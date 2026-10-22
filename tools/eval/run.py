"""Run diarist-cli over the eval sessions, score against references, write the
results table to docs/eval/<yyyy-mm-dd>-<config>.md.

  uv run python -I run.py --config nemotron35 --sessions aug2,aug9,seplog,ami
  uv run python -I run.py --config nemotron35 --sessions all
  uv run python -I run.py --config nemotron35 --sessions all --metrics-only
  uv run python -I run.py --config nemotron35 --export-for-correction aug2

Per session: the audio is converted once to 16 kHz mono WAV (.work/wav), then
`diarist-cli transcribe --rttm` runs into the config's scratch data root
(.work/data/<config>); the transcript is exported as JSON and RTTM, and the
standalone diarizer RTTM is copied next to it (.work/runs/<config>/<session>).
A finished run is cached by (session, config): re-runs only recompute metrics
unless --force is given.

References: data/eval/refs/<session>/ref.seglst.jsonl (hand-corrected, plus
optional ref.rttm) takes precedence; otherwise Zoom sessions are converted
from transcript.vtt and AMI uses its word-level manual annotation and RTTM.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
import re
import shutil
import subprocess
import sys
import time
import tomllib
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import (  # noqa: E402
    BIN,
    CUE_TIMING_NOTE,
    DOCS_EVAL,
    EVAL_DIR,
    GROUP_ORDER,
    NORMALIZER_NAME,
    REFS_OVERRIDE,
    REPO,
    WORK,
    Session,
    load_sessions,
    read_rttm,
    read_seglst,
    seglst_to_rttm,
    sha256_file,
    write_jsonl,
)
import gpu  # noqa: E402
import metrics as M  # noqa: E402
import zoom_vtt_to_ref  # noqa: E402

CLI = Path(os.environ.get("DIARIST_EVAL_CLI", BIN / "diarist-cli.exe"))
ENGINE_DIR = Path(
    os.environ.get(
        "DIARIST_EVAL_ENGINE_DIR", REPO / "engine" / "nemo-speech" / "nemo-speech-0.2.0-windows-x86_64-cuda"
    )
)
MODELS_DIR = Path(os.environ.get("DIARIST_EVAL_MODELS_DIR", REPO / "models"))
RUN_SCHEMA = 1
GPU_IDLE_MIB = int(os.environ.get("DIARIST_EVAL_GPU_IDLE_MIB", gpu.DEFAULT_IDLE_MIB))


def log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


# --------------------------------------------------------------------------
# Audio and diarist-cli


def probe(path: Path) -> dict:
    out = subprocess.run(
        [
            "ffprobe", "-v", "error", "-select_streams", "a:0",
            "-show_entries", "stream=codec_name,sample_rate,channels:format=duration",
            "-of", "json", str(path),
        ],
        check=True, capture_output=True, text=True,
    ).stdout
    j = json.loads(out)
    st = (j.get("streams") or [{}])[0]
    return {
        "codec": st.get("codec_name"),
        "rate": int(st.get("sample_rate", 0) or 0),
        "channels": int(st.get("channels", 0) or 0),
        "duration": float(j.get("format", {}).get("duration", 0) or 0),
    }


def wav_for(s: Session) -> tuple[Path, float]:
    info = probe(s.audio)
    if info["codec"] == "pcm_s16le" and info["rate"] == 16000 and info["channels"] == 1:
        return s.audio, info["duration"]
    wav = WORK / "wav" / f"{s.id}.wav"
    if not wav.is_file() or wav.stat().st_mtime < s.audio.stat().st_mtime:
        wav.parent.mkdir(parents=True, exist_ok=True)
        tmp = wav.with_suffix(".tmp.wav")
        log(f"[{s.id}] ffmpeg -> 16 kHz mono WAV")
        subprocess.run(
            ["ffmpeg", "-nostdin", "-v", "error", "-y", "-i", str(s.audio), "-vn",
             "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", str(tmp)],
            check=True,
        )
        tmp.replace(wav)
    return wav, probe(wav)["duration"]


def cli(data: Path, *args: str, capture: bool = True) -> subprocess.CompletedProcess:
    cmd = [str(CLI), "--data-dir", str(data), "--models-dir", str(MODELS_DIR), "--engine-dir", str(ENGINE_DIR), *args]
    p = subprocess.run(cmd, capture_output=capture, text=True, encoding="utf-8", errors="replace")
    if p.returncode != 0:
        raise RuntimeError(f"diarist-cli {' '.join(args[:2])} failed ({p.returncode}): {(p.stderr or '')[-2000:]}")
    return p


def prepare_data_root(config: str, cfg: dict) -> Path:
    data = WORK / "data" / config
    data.mkdir(parents=True, exist_ok=True)
    if not (data / "diarist.db").is_file():
        cli(data, "models", "scan")
    cli(data, "models", "select", "asr", cfg["asr"])
    cli(data, "models", "select", "diarizer", cfg["diarizer"])
    return data


def run_dir(config: str, sid: str) -> Path:
    return WORK / "runs" / config / sid


def cached(config: str, sid: str, cfg: dict) -> dict | None:
    """The finished run of (session, config), if its settings still match."""
    d = run_dir(config, sid)
    try:
        meta = json.loads((d / "run.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    if meta.get("status") != "ok" or meta.get("schema") != RUN_SCHEMA or not (d / "transcript.json").is_file():
        return None
    if any(meta.get(k) != cfg.get(k) for k in ("asr", "diarizer", "language")) or list(
        meta.get("engine_args") or []
    ) != list(cfg.get("engine_args") or []):
        log(f"[{sid}] cached run used different settings; transcribing again")
        return None
    if CLI.is_file() and meta.get("cli_sha256") != sha256_file(CLI):
        log(f"[{sid}] note: cached run was made by a different diarist-cli build (use --force to redo)")
    return meta


def engine_arg_flags(cfg: dict) -> list[str]:
    """configs.toml `engine_args` -> `--engine-arg=<flag>` per item (diarist-cli
    appends them verbatim to `nemo-speech transcribe`; the standalone
    `diarize` pass of `--rttm` does not get them)."""
    return [f"--engine-arg={a}" for a in cfg.get("engine_args") or []]


def birth_time(path: Path) -> float | None:
    try:
        st = path.stat()
    except OSError:
        return None
    return getattr(st, "st_birthtime", None) or st.st_ctime


def phase_times(audio_dir: Path) -> dict:
    """Unix times of the engine steps, from the files diarist-core leaves in
    the meeting's audio folder: engine.log is created when the first engine
    command starts, source.words.json is written when `transcribe` ends and
    diarization.rttm when the standalone `diarize` pass ends."""
    out = {"engine_start": birth_time(audio_dir / "engine.log")}
    for key, name in (("transcribe_end", "source.words.json"), ("diarize_end", "diarization.rttm")):
        try:
            out[key] = (audio_dir / name).stat().st_mtime
        except OSError:
            out[key] = None
    return out


def transcribe(
    s: Session, config: str, cfg: dict, data: Path, *, language: str | None = None, run_id: str | None = None,
    diarize: bool = True, wav: Path | None = None, duration: float | None = None,
) -> dict:
    """One diarist-cli transcribe of session s (or of `wav`, for FLEURS) into
    runs/<config>/<run_id or session id>, with wall time per engine step and
    peak VRAM (nvidia-smi delta) per step."""
    rid = run_id or s.id
    d = run_dir(config, rid)
    d.mkdir(parents=True, exist_ok=True)
    if wav is None:
        wav, duration = wav_for(s)
    lang = language or cfg["language"]
    log(f"[{rid}] diarist-cli transcribe ({duration / 60:.1f} min, config {config}, language {lang})")
    gpu.wait_idle(GPU_IDLE_MIB)
    args = ["transcribe", str(wav), "--language", lang, "--title", rid]
    args += ["--rttm"] if diarize else ["--no-diarize"]
    args += ["--out", str(d / "transcript.json"), "--format", "json", *engine_arg_flags(cfg)]
    t_start = time.time()
    t0 = time.perf_counter()
    with gpu.Sampler() as mon:
        p = cli(data, *args)
    wall = time.perf_counter() - t0
    m = re.search(r"^meeting (\d+) \((\w+)\)", p.stdout, re.M)
    if not m:
        raise RuntimeError(f"cannot find the meeting id in diarist-cli output:\n{p.stdout}")
    mid, uid = m.group(1), m.group(2)
    if diarize:
        cli(data, "export", mid, "--format", "rttm", "--out", str(d / "hyp.rttm"))
    audio_dir = data / "audio" / uid
    for name, dest in (
        ("diarization.rttm", "diarizer.rttm"),
        ("diarization.json", "diarizer.json"),
        ("engine.log", "engine.log"),
        ("source.words.json", "engine.words.json"),
    ):
        if (audio_dir / name).is_file():
            shutil.copyfile(audio_dir / name, d / dest)
    # diarist-cli's stderr line: `engine <v> | asr <file> | diarizer <file>`; since
    # beta.2 also `| device <d>` after the engine (`<d> (--device)` when overridden)
    # and `| lookahead <n> | diar preset <name>`, then `| engine args ...`.
    eng = re.search(
        r"^engine (?P<engine>\S+)(?: \| device (?P<device>.+?))? \| asr (?P<asr>.+?) \| diarizer (?P<diar>.+?)"
        r"(?: \| lookahead (?P<lookahead>.+?))?(?: \| diar preset (?P<preset>.+?))?"
        r"(?: \| engine args .*)?\s*$",
        p.stderr,
        re.M,
    )
    ph = phase_times(audio_dir)
    t_asr0, t_asr1, t_dia1 = ph["engine_start"] or t_start, ph["transcribe_end"], ph["diarize_end"]
    meta = {
        "schema": RUN_SCHEMA,
        "status": "ok",
        "session": s.id,
        "config": config,
        "meeting_id": int(mid),
        "meeting_uid": uid,
        "wall_s": round(wall, 1),
        # nemo-speech transcribe alone (model load, warmup and decoding), from
        # engine.log creation to source.words.json; the rest of wall_s is the
        # audio decode, the standalone diarize pass and the database import.
        "asr_wall_s": round(t_asr1 - t_asr0, 1) if t_asr1 else None,
        "diarize_wall_s": round(t_dia1 - t_asr1, 1) if (t_dia1 and t_asr1) else None,
        "vram_base_mib": mon.base,
        "vram_peak_delta_mib": mon.peak_delta(),
        "vram_asr_peak_delta_mib": mon.peak_delta(t_asr0, t_asr1) if t_asr1 else None,
        "vram_diarize_peak_delta_mib": mon.peak_delta(t_asr1, t_dia1) if (t_asr1 and t_dia1) else None,
        "audio_s": round(duration, 3),
        "language": lang,
        "asr": cfg["asr"],
        "diarizer": cfg["diarizer"],
        "engine_args": list(cfg.get("engine_args") or []),
        "engine_version": eng["engine"] if eng else None,
        "asr_file": eng["asr"] if eng else None,
        "diarizer_file": eng["diar"] if eng else None,
        # What the CLI says it passed (its run's EngineInfo); None from older builds.
        "device": eng["device"].removesuffix(" (--device)") if eng and eng["device"] else None,
        "right_context": eng["lookahead"] if eng else None,
        "diar_preset": eng["preset"] if eng else None,
        "cli_sha256": sha256_file(CLI),
        "finished": dt.datetime.now().isoformat(timespec="seconds"),
        "cli_stdout": [ln for ln in p.stdout.splitlines() if not ln.strip().startswith("title")],
    }
    (d / "run.json").write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
    asr_rt = f", transcribe step {meta['asr_wall_s']} s" if meta["asr_wall_s"] else ""
    log(
        f"[{rid}] done in {wall:.0f} s ({duration / max(wall, 1e-9):.1f}x real time{asr_rt}; "
        f"peak VRAM delta {meta['vram_peak_delta_mib']} MiB)"
    )
    return meta


# --------------------------------------------------------------------------
# References


def load_reference(s: Session) -> tuple[list[dict], list, str]:
    """Returns (SegLST rows, RTTM rows, source description)."""
    over = REFS_OVERRIDE / s.id
    if (over / "ref.seglst.jsonl").is_file():
        segs = read_seglst(over / "ref.seglst.jsonl")
        rttm = read_rttm(over / "ref.rttm") if (over / "ref.rttm").is_file() else seglst_to_rttm(segs, s.id)
        return segs, rttm, "hand-corrected"
    if s.kind == "zoom":
        out = WORK / "refs"
        zoom_vtt_to_ref.convert(s.vtt, s.id, out)
        return read_seglst(out / f"{s.id}.ref.seglst.jsonl"), read_rttm(out / f"{s.id}.ref.rttm"), "zoom-cue"
    if s.kind == "ami":
        if not (s.ref_seglst and s.ref_seglst.is_file()):
            raise SystemExit(f"{s.ref_seglst} missing: run `uv run python -I ami_fetch.py`")
        return read_seglst(s.ref_seglst), read_rttm(s.ref_rttm), "ami-word"
    raise SystemExit(f"session {s.id}: unknown kind {s.kind!r}")


# --------------------------------------------------------------------------
# Scoring


def scored_region(s: Session, ref_segs: list[dict], duration: float | None) -> tuple[float, float] | None:
    """sessions.toml `uem`: "ref" (annotated span, 1 s margin) or [start, end] seconds."""
    u = s.extra.get("uem")
    if u == "ref":
        a = min(float(r["start_time"]) for r in ref_segs)
        b = max(float(r["end_time"]) for r in ref_segs)
        return max(0.0, a - 1.0), min(duration or b + 1.0, b + 1.0)
    if isinstance(u, list) and len(u) == 2:
        return float(u[0]), float(u[1])
    return None


def score(s: Session, config: str, meta: dict) -> dict:
    d = run_dir(config, s.id)
    ref_segs, ref_rttm, ref_src = load_reference(s)
    doc = json.loads((d / "transcript.json").read_text(encoding="utf-8"))
    hyp_segs, hyp_words = M.hyp_from_export(doc, s.id)
    write_jsonl(d / "hyp.seglst.jsonl", hyp_segs)
    hyp_rttm = read_rttm(d / "hyp.rttm")
    duration = meta.get("audio_s")
    region = scored_region(s, ref_segs, duration)
    t0 = time.perf_counter()
    res = M.meeting_metrics(ref_segs, ref_rttm, hyp_segs, hyp_words, hyp_rttm, duration, s.id, region)
    if (d / "diarizer.rttm").is_file():
        raw = M.diarization_metrics(ref_rttm, read_rttm(d / "diarizer.rttm"), duration, region)
        res.update({f"diar_{k}": v for k, v in raw.items()})
    res.update(
        session=s.id,
        label=s.label,
        group=s.group,
        ref_source=ref_src,
        region=region,
        audio_s=duration,
        wall_s=meta.get("wall_s"),
        asr_wall_s=meta.get("asr_wall_s"),
        vram_peak_delta_mib=meta.get("vram_peak_delta_mib"),
        vram_asr_peak_delta_mib=meta.get("vram_asr_peak_delta_mib"),
        score_s=round(time.perf_counter() - t0, 1),
    )
    (d / "metrics.json").write_text(json.dumps(res, indent=2, default=str) + "\n", encoding="utf-8")
    return res


# --------------------------------------------------------------------------
# Report


def pct(x) -> str:
    return "–" if x is None or (isinstance(x, float) and math.isnan(x)) else f"{100 * x:.1f}"


def signed(x) -> str:
    return "–" if x is None else f"{x:+d}"


def model_info(name: str) -> dict:
    manifest = json.loads((REPO / "manifests" / "models.json").read_text(encoding="utf-8"))
    m = next((x for x in manifest if x["name"] == name), None)
    if not m:
        return {"name": name}
    info = {"name": name, "repo": m["repo"], "revision": m["revision"], "file": m["file"], "sha256": m["sha256"]}
    ver = MODELS_DIR / Path(*m["repo"].split("/")) / m["revision"] / (m["file"] + ".verified")
    try:
        kv = dict(ln.split("=", 1) for ln in ver.read_text().splitlines() if "=" in ln)
        info["verified"] = kv.get("sha256") == m["sha256"]
    except OSError:
        info["verified"] = None
    return info


def engine_commands(config: str, metas: dict[str, dict]) -> list[str]:
    """The nemo-speech command lines diarist-core ran, from a run's engine.log,
    with paths reduced to placeholders."""
    for sid, meta in metas.items():
        log_path = run_dir(config, sid) / "engine.log"
        if not log_path.is_file():
            src = WORK / "data" / config / "audio" / str(meta.get("meeting_uid")) / "engine.log"
            if src.is_file():
                shutil.copyfile(src, log_path)
        if not log_path.is_file():
            continue
        cmds = []
        for ln in log_path.read_text(encoding="utf-8", errors="replace").splitlines():
            if not ln.startswith("$ "):
                continue
            args = [a.replace("\\\\", "\\") for a in re.findall(r'"((?:[^"\\]|\\.)*)"', ln)]
            out = []
            for a in args[1:]:
                low = a.lower()
                if low.endswith(".wav"):
                    a = "<wav>"
                elif low.endswith(".gguf"):
                    a = "<" + re.split(r"[\\/]", a)[-1] + ">"
                elif low.endswith((".json", ".rttm")):
                    a = "<out>"
                out.append(a)
            cmd = "nemo-speech " + " ".join(out)
            if cmd not in cmds:
                cmds.append(cmd)
        if cmds:
            return cmds
    return []


def mean_max(rows: list[dict], key: str) -> tuple[float | None, float | None]:
    vals = [r[key] for r in rows if isinstance(r.get(key), (int, float)) and not math.isnan(r[key])]
    if not vals:
        return None, None
    return sum(vals) / len(vals), max(vals)


ATTR_COLS = [
    ("DER c0.25", "der_c025"),
    ("DER c0", "der_c0"),
    ("JER", "jer"),
    ("WDER", "wder"),
    ("WDER near", "wder_near"),
    ("cpWER", "cpwer"),
    ("DI-cpWER", "dicpwer"),
    ("cp−DI", "attribution_loss"),
    ("tcpWER", "tcpwer"),
]


def table_rows(rows: list[dict]) -> list[str]:
    head = "| Session | Group | Min | Ref spk | Hyp spk | Count err | " + " | ".join(c for c, _ in ATTR_COLS) + " |"
    sep = "|" + "---|" * (6 + len(ATTR_COLS))
    out = [head, sep]
    for r in rows:
        out.append(
            f"| {r['label']} | {r['group']} | {r['audio_s'] / 60:.0f} | {r['ref_speakers']} | {r['hyp_speakers']} | "
            f"{signed(r['count_error'])} | " + " | ".join(pct(r.get(k)) for _, k in ATTR_COLS) + " |"
        )
    return out


def section(rows: list[dict]) -> list[str]:
    return [*table_rows(rows), ""] if rows else ["Not run in this report.", ""]


def quality_bar(results: list[dict]) -> list[str]:
    attr = [r for r in results if r["group"] in ("tuning", "heldout")]
    out = []
    if attr:
        mn, mx = mean_max(attr, "wder")
        ok = mn is not None and mn < 0.10 and mx < 0.20
        out.append(
            f"- WDER over the user meetings ({len(attr)}): mean {pct(mn)} %, max {pct(mx)} % "
            f"(bar: mean < 10, max < 20): {'met' if ok else 'NOT met'}"
        )
        far = [r["label"] for r in attr if abs(r["count_error"]) > 1]
        out.append(
            "- Speaker count within one on the user meetings: "
            + ("met" if not far else "NOT met (" + ", ".join(far) + ")")
        )
    for r in results:
        if r["group"] == "control":
            out.append(
                f"- Single-speaker control ({r['label']}): transcript has {r['hyp_speakers']} speaker(s), "
                f"diarizer pass {r.get('diar_hyp_speakers', '–')}: {'met' if r['hyp_speakers'] == 1 else 'NOT met'}"
            )
    out.append(
        "- References are Zoom cue-level, not hand-labelled yet (docs/00 asks for hand-labelled meetings); "
        "treat the verdicts as provisional until data/eval/refs holds corrected references."
    )
    return out


def aggregate_rows(name: str, rows: list[dict]) -> list[str]:
    if not rows:
        return []
    out = []
    for stat in ("mean", "max"):
        cells = []
        for _, k in ATTR_COLS:
            mn, mx = mean_max(rows, k)
            cells.append(pct(mn if stat == "mean" else mx))
        ce = [abs(r["count_error"]) for r in rows]
        cnt = f"{sum(ce) / len(ce):.1f}" if stat == "mean" else f"{max(ce)}"
        out.append(f"| {name} ({len(rows)}) | {stat} | | | | abs {cnt} | " + " | ".join(cells) + " |")
    return out


def _engine_arg(cfg: dict, flag: str) -> str | None:
    args = list(cfg.get("engine_args") or [])
    return args[args.index(flag) + 1] if flag in args[:-1] else None


def diar_preset_of(cfg: dict, meta: dict) -> str:
    """The fused pass's diarizer preset: engine_args, else what the CLI reported, else the runtime default."""
    arg = _engine_arg(cfg, "--asr.diar.preset")
    if arg:
        return f"{arg} (engine_args)"
    rep = meta.get("diar_preset")
    if rep and rep != "runtime default":
        return f"{rep} (diarist-cli's own choice)"
    return "v3-streaming (runtime default)"


def lookahead_of(cfg: dict, meta: dict) -> str:
    arg = _engine_arg(cfg, "--asr.streaming.rnnt_right_context")
    if arg:
        return f"{arg} (engine_args)"
    rep = meta.get("right_context")
    if rep and rep != "runtime default":
        return f"{rep} (diarist-cli's own choice)"
    if rep == "runtime default":
        return "runtime default (1)"
    return "not recorded (a diarist-cli build from before 0.1.0-beta.2 passes the runtime default, 1)"


def write_report(config: str, cfg: dict, results: list[dict], metas: dict[str, dict], path: Path) -> None:
    today = dt.date.today().isoformat()
    any_meta = next(iter(metas.values()), {})
    asr, diar = model_info(cfg["asr"]), model_info(cfg["diarizer"])
    order = {g: i for i, g in enumerate(GROUP_ORDER)}
    results = sorted(results, key=lambda r: (order.get(r["group"], 99), r["label"]))
    attr = [r for r in results if r["group"] in ("tuning", "heldout")]
    total_wall = sum(m.get("wall_s") or 0 for m in metas.values())
    total_audio = sum(m.get("audio_s") or 0 for m in metas.values())
    lines = [
        f"# Eval: {config} ({today})",
        "",
        f"{cfg.get('description', '')}. Generated by `tools/eval/run.py --config {config}`; numbers are percentages unless a column says otherwise.",
        "",
        "## Setup",
        "",
        f"- Date: {today}",
        f"- diarist-cli: `{CLI.name}` sha256 `{(any_meta.get('cli_sha256') or sha256_file(CLI))[:16]}` (built from this repo; see tools/eval/.bin)",
        f"- Engine: nemo-speech {any_meta.get('engine_version') or '?'} (`{ENGINE_DIR.name}`)",
        f"- ASR: {asr.get('name')} `{asr.get('repo')}` rev `{asr.get('revision', '')[:12]}` sha256 `{asr.get('sha256', '')[:16]}`"
        f" (local .verified matches: {asr.get('verified')})",
        f"- Diarizer: {diar.get('name')} `{diar.get('repo')}` rev `{diar.get('revision', '')[:12]}` sha256 `{diar.get('sha256', '')[:16]}`"
        f" (local .verified matches: {diar.get('verified')}); preset {diar_preset_of(cfg, any_meta)}",
        f"- Decoder lookahead (`rnnt_right_context`): {lookahead_of(cfg, any_meta)}",
        f"- diarist-cli flags: `transcribe <16 kHz wav> --language {cfg['language']} --rttm --format json --out ..."
        + "".join(f" {a}" for a in engine_arg_flags(cfg))
        + "`, then `export --format rttm`",
        "- Engine command lines (from diarist-core's engine.log): "
        + "; ".join(f"`{c}`" for c in engine_commands(config, metas)),
        "- Word-to-speaker fusion: the runtime's own tags (start-anchored argmax); no app-side smoothing yet",
        f"- Text normaliser: {NORMALIZER_NAME}",
        f"- GPU wall time for these runs: {total_wall / 60:.1f} min for {total_audio / 60:.0f} min of audio"
        f" ({total_audio / max(total_wall, 1e-9):.1f}x real time; includes the standalone diarize pass)",
        "",
        "## Quality bar (docs/00)",
        "",
        *quality_bar(results),
        "",
        "## Transcript attribution (app output)",
        "",
        "DER/JER from the exported transcript RTTM (one span per transcript segment). WDER near = words within 1 s of a reference speaker change.",
        "",
        "### User meetings (tuning and held-out)",
        "",
        *section(attr),
        "### Controls",
        "",
        *section([r for r in results if r["group"] in ("control", "overlap")]),
        "### Stress: in-room multi-speaker (reported separately)",
        "",
        "Shared room devices carry several people under one Zoom label, so the reference undercounts speakers. "
        "A diarizer that correctly splits those people is charged for it by DER, JER and WDER (the speaker map is one to one); "
        "read this row for the speaker count and for regressions between configs, not against the bar.",
        "",
        *section([r for r in results if r["group"] == "stress"]),
        "## Aggregates",
        "",
        "| Set | Stat | | | | Count err | " + " | ".join(c for c, _ in ATTR_COLS) + " |",
        "|" + "---|" * (6 + len(ATTR_COLS)),
        *aggregate_rows("Tuning", [r for r in results if r["group"] == "tuning"]),
        *aggregate_rows("Held-out", [r for r in results if r["group"] == "heldout"]),
        *aggregate_rows("Tuning + held-out", attr),
        "",
        "## Standalone diarizer pass (`--rttm`)",
        "",
        "| Session | Hyp spk | Count err | DER c0.25 | miss | FA | conf | DER c0 | JER |",
        "|---|---|---|---|---|---|---|---|---|",
    ]
    for r in results:
        if "diar_der_c025" in r:
            lines.append(
                f"| {r['label']} | {r['diar_hyp_speakers']} | {signed(r['diar_count_error'])} | {pct(r['diar_der_c025'])} | "
                f"{pct(r['diar_der_c025_miss'])} | {pct(r['diar_der_c025_fa'])} | {pct(r['diar_der_c025_conf'])} | "
                f"{pct(r['diar_der_c0'])} | {pct(r['diar_jer'])} |"
            )
    lines += [
        "",
        "## Words and speed",
        "",
        "Wall s covers the whole diarist-cli run (audio decode, transcribe, the standalone diarize pass of `--rttm`, import). "
        "Transcribe s is the `nemo-speech transcribe` step alone (engine.log creation to its words JSON). "
        "VRAM is the peak nvidia-smi delta during the transcribe step over the reading before the run (research/08 pitfall 10); – when not sampled.",
        "",
        "| Session | Ref source | Ref words | Hyp words | Aligned | Near-change words | Ref changes | ORC-WER | Wall s | × real time | Transcribe s | VRAM Δ MiB |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for r in results:
        rt = r["audio_s"] / r["wall_s"] if r.get("wall_s") else None
        lines.append(
            f"| {r['label']} | {r['ref_source']} | {r['ref_words']} | {r['hyp_words']} | {r['wder_pairs']} | "
            f"{r['wder_near_pairs']} | {r['ref_changes']} | {pct(r.get('orcwer'))} | {r.get('wall_s') or '–'} | "
            f"{f'{rt:.1f}' if rt else '–'} | {r.get('asr_wall_s') or '–'} | {r.get('vram_asr_peak_delta_mib') or '–'} |"
        )
    lines += [
        "",
        "## Notes",
        "",
        f"- Zoom references ({sum(r['ref_source'] == 'zoom-cue' for r in results)} sessions): {CUE_TIMING_NOTE}. Overlapping speech is serialised and short backchannels are often missing, so DER is approximate there; speaker identity per cue is near ground truth, which is what WDER uses.",
        "- The Zoom transcript is machine output: cpWER, tcpWER and ORC-WER against it mix ASR disagreement with attribution. Word accuracy is judged on AMI only; cpWER − DI-cpWER is the attribution share.",
        "- AMI ES2002a: word-level manual annotation (ami_public_manual_1.6.2) for the SegLST reference, pyannote AMI-diarization-setup only_words RTTM for DER.",
        "- DER collar 0.25 = ±0.25 s around reference boundaries (pyannote `collar=0.5`). Overlap is scored. The scored region is the whole recording, except AMI: its annotated span ±1 s, because the first 50 s of pre-meeting chatter is not transcribed (hypothesis words outside the span are dropped for the transcript metrics too).",
        "- AMI WER here is on the Mix-Headset (far-field mix) channel with this harness's normaliser, so it is not directly comparable with published AMI figures (usually IHM). Use it to compare configs.",
        "- Count err = hypothesis speakers − reference speakers (transcript speakers for the first table, diarizer labels for the second). Zoom labels count people with their own audio stream; the stress session's room devices hide several people behind one label, so its reference count is a lower bound.",
        f"- tcpWER collar {M.TCP_COLLAR_S:.0f} s. DI-cpWER treats each hypothesis word as its own segment. Nemotron word times run about 0.3 s late (research/08 pitfall 11); nothing here shifts them.",
        "- Quality bar (docs/00): WDER average < 10 % and max < 20 % over the user's meetings, exact speaker count on the single-speaker control, within one elsewhere.",
        "",
    ]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines), encoding="utf-8", newline="\n")
    log(f"wrote {path}")


def print_summary(results: list[dict]) -> None:
    print(f"{'session':<14}{'count':>6}{'DER25':>7}{'JER':>7}{'WDER':>7}{'near':>7}{'cpWER':>7}{'DIcp':>7}")
    for r in results:
        line = (
            f"{r['label']:<14}{signed(r['count_error']):>6}{pct(r['der_c025']):>7}{pct(r['jer']):>7}"
            f"{pct(r['wder']):>7}{pct(r['wder_near']):>7}{pct(r['cpwer']):>7}{pct(r['dicpwer']):>7}"
        )
        print(line.replace("–", "-"))  # the Windows console code page has no en dash


# --------------------------------------------------------------------------


def export_for_correction(s: Session, config: str) -> None:
    d = run_dir(config, s.id)
    if not (d / "transcript.json").is_file():
        raise SystemExit(f"no cached run for {s.id} under config {config}; run it first")
    doc = json.loads((d / "transcript.json").read_text(encoding="utf-8"))
    segs, _ = M.hyp_from_export(doc, s.id)
    out = REFS_OVERRIDE / s.id / "draft.seglst.jsonl"
    write_jsonl(out, segs)
    print(
        f"wrote {out}\nEdit speaker labels, boundaries and words, then save it as "
        f"{out.with_name('ref.seglst.jsonl')} (it then takes precedence over the Zoom reference)."
    )


# --------------------------------------------------------------------------
# WER-only mode: FLEURS slices from fleurs_fetch.py (no diarization reference)

FLEURS = WORK / "fleurs"
# The language tag the runtime honours per app code. The Nemotron 3.5 prompt
# dictionary (GGUF asr.rnnt.prompt_dictionary) has bare de, fr, it, hi, en, es
# (es-US) and pt (pt-PT), but no bare ja or zh: those run as auto. FLEURS es_419
# is Latin American and pt_br Brazilian, hence es-US and pt-BR.
FLEURS_TAGS = {"de": "de", "fr": "fr", "it": "it", "hi": "hi", "en": "en", "ja": "ja-JP", "es": "es-US", "pt": "pt-BR"}
# Model card FLEURS WER (CER for ja), Nemotron 3.5, by lookahead in ms:
# (language given, auto-detect). Right context R gives (R + 1) x 80 ms.
CARD = {
    "es": ({80: 4.87, 160: 4.64, 320: 4.39, 560: 4.26, 1120: 4.11}, {80: 5.04, 160: 4.82, 320: 4.48, 560: 4.34, 1120: 4.13}),
    "it": ({80: 5.23, 160: 4.85, 320: 4.83, 560: 4.41, 1120: 4.25}, {80: 5.28, 160: 4.89, 320: 4.84, 560: 4.47, 1120: 4.32}),
    "pt": ({80: 6.29, 160: 6.10, 320: 5.81, 560: 5.65, 1120: 5.48}, {80: 6.41, 160: 6.19, 320: 5.82, 560: 5.57, 1120: 5.47}),
    "hi": ({80: 8.13, 160: 7.97, 320: 7.41, 560: 7.05, 1120: 6.81}, {80: 11.47, 160: 10.83, 320: 9.88, 560: 9.26, 1120: 8.23}),
    "en": ({80: 9.43, 160: 8.88, 320: 8.27, 560: 7.99, 1120: 7.91}, {80: 9.72, 160: 9.34, 320: 8.84, 560: 8.80, 1120: 8.84}),
    "de": ({80: 9.81, 160: 9.21, 320: 8.83, 560: 8.42, 1120: 8.31}, {80: 9.90, 160: 9.37, 320: 8.87, 560: 8.58, 1120: 8.22}),
    "fr": ({80: 10.97, 160: 10.60, 320: 9.79, 560: 9.45, 1120: 9.03}, {80: 11.03, 160: 10.60, 320: 9.84, 560: 9.46, 1120: 9.02}),
    "ja": ({80: 13.87, 160: 12.90, 320: 12.22, 560: 11.91, 1120: 11.48}, {80: 14.97, 160: 13.85, 320: 13.00, 560: 12.38, 1120: 11.66}),
}
CER_LANGS = {"ja", "zh", "ko"}


def lookahead_ms(cfg: dict) -> int:
    args = list(cfg.get("engine_args") or [])
    r = 1
    if "--asr.streaming.rnnt_right_context" in args:
        r = int(args[args.index("--asr.streaming.rnnt_right_context") + 1])
    if r < 0:  # the GGUF's trained maximum
        r = 13 if cfg["asr"] == "nemotron-en" else 3
    return (r + 1) * 80


def multilingual_tokens(text: str) -> list[str]:
    """Non-English normaliser: NFKC, casefold, every punctuation or symbol
    character (Unicode P*, S*) becomes a space. Combining marks (Devanagari
    vowel signs, Japanese voicing marks) are kept, unlike whisper's
    BasicTextNormalizer, which splits Hindi words at every vowel sign."""
    import unicodedata

    s = unicodedata.normalize("NFKC", text).casefold()
    s = "".join(" " if unicodedata.category(c)[0] in "PS" else c for c in s)
    return s.split()


def fleurs_units(lang: str, text: str) -> list[str]:
    from common import normalize_text

    if lang == "en":
        return normalize_text(text)  # the harness normaliser, as for AMI and Zoom
    toks = multilingual_tokens(text)
    if lang in CER_LANGS:
        return [c for t in toks for c in t]
    return toks


def edit_counts(ref: list[str], hyp: list[str]) -> dict:
    from rapidfuzz.distance import Levenshtein

    sub = dele = ins = 0
    for op in Levenshtein.opcodes(ref, hyp):
        n_ref, n_hyp = op.src_end - op.src_start, op.dest_end - op.dest_start
        if op.tag == "replace":
            sub += min(n_ref, n_hyp)
            dele += max(0, n_ref - n_hyp)
            ins += max(0, n_hyp - n_ref)
        elif op.tag == "delete":
            dele += n_ref
        elif op.tag == "insert":
            ins += n_hyp
    n = len(ref)
    return {"ref_units": n, "hyp_units": len(hyp), "sub": sub, "del": dele, "ins": ins,
            "er": (sub + dele + ins) / n if n else float("nan")}


def fleurs_language(lang: str, mode: str) -> str:
    return {"given": FLEURS_TAGS[lang], "short": lang, "auto": "auto"}[mode]


def run_fleurs(config: str, cfg: dict, langs: list[str], mode: str, force: bool, metrics_only: bool) -> list[dict]:
    data = None
    out = []
    for lang in langs:
        root = FLEURS / lang
        if not (root / "meta.json").is_file():
            raise SystemExit(f"{root} missing: run `uv run python -I fleurs_fetch.py --langs {lang}`")
        fmeta = json.loads((root / "meta.json").read_text(encoding="utf-8"))
        tag = fleurs_language(lang, mode)
        rid = f"fleurs-{lang}-{mode}"
        d = run_dir(config, rid)
        meta = None
        try:
            meta = json.loads((d / "run.json").read_text(encoding="utf-8"))
            if (
                meta.get("status") != "ok" or meta.get("language") != tag or meta.get("asr") != cfg["asr"]
                or list(meta.get("engine_args") or []) != list(cfg.get("engine_args") or [])
            ):
                meta = None
        except (OSError, ValueError):
            meta = None
        if (meta is None or force) and not metrics_only:
            data = data or prepare_data_root(config, cfg)
            s = Session(id=rid, label=rid, kind="fleurs", group="fleurs", audio=root / "audio.wav")
            meta = transcribe(
                s, config, cfg, data, language=tag, run_id=rid, diarize=False,
                wav=root / "audio.wav", duration=fmeta["duration_s"],
            )
        if meta is None:
            log(f"[{rid}] no cached run for {config}; skipped")
            continue
        refs = [json.loads(ln) for ln in (root / "refs.jsonl").read_text(encoding="utf-8").splitlines() if ln.strip()]
        doc = json.loads((d / "transcript.json").read_text(encoding="utf-8"))
        hyp_text = " ".join(s["text"] for s in sorted(doc.get("segments", []), key=lambda s: s["start_ms"]))
        ref_text = " ".join(r["raw_transcription"] for r in refs)
        res = edit_counts(fleurs_units(lang, ref_text), fleurs_units(lang, hyp_text))
        detected = None
        try:
            detected = json.loads((d / "engine.words.json").read_text(encoding="utf-8")).get("languages")
        except (OSError, ValueError):
            pass
        ms = lookahead_ms(cfg)
        card = CARD.get(lang)
        res.update(
            config=config, lang=lang, mode=mode, tag=tag, metric="CER" if lang in CER_LANGS else "WER",
            utterances=len(refs), audio_s=fmeta["duration_s"], wall_s=meta.get("wall_s"),
            asr_wall_s=meta.get("asr_wall_s"), detected=detected, lookahead_ms=ms,
            card=(card[1 if mode == "auto" else 0].get(ms) if card and cfg["asr"] == "nemotron-3.5" else None),
        )
        (d / "metrics.json").write_text(json.dumps(res, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        log(f"[{rid}] {res['metric']} {100 * res['er']:.1f} % ({res['ref_units']} ref units), detected {detected}")
        out.append(res)
    return out


def fleurs_report(path: Path, configs: dict) -> None:
    """One table over every cached FLEURS run (all configs and modes)."""
    rows = []
    for mj in sorted((WORK / "runs").glob("*/fleurs-*/metrics.json")):
        r = json.loads(mj.read_text(encoding="utf-8"))
        if r.get("config") in configs:
            rows.append(r)
    order = list(FLEURS_TAGS)
    rows.sort(key=lambda r: (order.index(r["lang"]) if r["lang"] in order else 99, r["config"], r["mode"]))
    today = dt.date.today().isoformat()
    lines = [
        f"# Eval: FLEURS slices, WER only ({today})",
        "",
        "Generated by `tools/eval/run.py --fleurs-report`. About 10 minutes of the FLEURS test split per language "
        "(`fleurs_fetch.py`: utterances in archive order, one recording per sentence, joined with 1.5 s of silence) "
        "through `diarist-cli transcribe --no-diarize --language <tag>`. WER in percent; CER for ja. "
        "Normaliser: the harness's for en (" + NORMALIZER_NAME + "); for the others NFKC, casefold, "
        "punctuation and symbols removed (combining marks kept). Card = the model card's FLEURS number at the same "
        "lookahead and mode (full test split, NVIDIA's normaliser), for Nemotron 3.5 only.",
        "",
        "| Lang | Config | ASR | Lookahead | Mode | Tag passed | Detected | Utt | Ref units | WER/CER | Sub | Del | Ins | Card | Transcribe s |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for r in rows:
        cfg = configs.get(r["config"], {})
        lines.append(
            f"| {r['lang']} | {r['config']} | {cfg.get('asr', '?')} | {r['lookahead_ms']} ms | {r['mode']} | `{r['tag']}` | "
            f"{', '.join(r.get('detected') or []) or '–'} | {r['utterances']} | {r['ref_units']} | {pct(r['er'])} ({r['metric']}) | "
            f"{r['sub']} | {r['del']} | {r['ins']} | {r['card'] if r.get('card') is not None else '–'} | {r.get('asr_wall_s') or '–'} |"
        )
    lines += [
        "",
        "Notes: a 10-minute slice is 37 to 55 utterances, so one or two points either way is noise. FLEURS references "
        "write numbers as digits and the runtime has no ITN grammars configured, so spoken numbers count as errors on "
        "both sides of the comparison. The EN model ignores `--language` (diarist passes it only to multilingual models).",
        "",
    ]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines), encoding="utf-8", newline="\n")
    log(f"wrote {path}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--config", help="required except with --fleurs-report")
    ap.add_argument("--sessions", default="all", help="comma-separated session ids, or all")
    ap.add_argument("--sessions-file", type=Path, default=EVAL_DIR / "sessions.toml")
    ap.add_argument("--configs-file", type=Path, default=EVAL_DIR / "configs.toml")
    ap.add_argument("--force", action="store_true", help="re-transcribe even when cached")
    ap.add_argument("--transcribe-only", action="store_true")
    ap.add_argument("--metrics-only", action="store_true", help="never run the GPU; skip sessions without a cached run")
    ap.add_argument("--export-for-correction", metavar="SESSION")
    ap.add_argument("--out", type=Path, help="report path (default docs/eval/<date>-<config>.md)")
    ap.add_argument("--no-report", action="store_true")
    ap.add_argument(
        "--fleurs", metavar="LANGS",
        help="WER-only mode on the FLEURS slices of fleurs_fetch.py: comma-separated app codes, or all",
    )
    ap.add_argument(
        "--fleurs-language", choices=("given", "auto", "short"), default="given",
        help="given: the tag the runtime honours (ja-JP, es-US, pt-BR, bare de fr it hi en); "
        "short: the app's bare code; auto: detection",
    )
    ap.add_argument("--fleurs-report", action="store_true", help="write docs/eval/<date>-fleurs.md from cached FLEURS runs")
    a = ap.parse_args()

    configs = tomllib.loads(a.configs_file.read_text(encoding="utf-8"))
    if a.fleurs_report:
        fleurs_report(a.out or DOCS_EVAL / f"{dt.date.today().isoformat()}-fleurs.md", configs)
        return
    if not a.config:
        raise SystemExit("--config is required")
    if a.config not in configs:
        raise SystemExit(f"unknown config {a.config!r}; known: {', '.join(configs)}")
    cfg = configs[a.config]
    if cfg.get("skip"):
        raise SystemExit(f"config {a.config} is not runnable: {cfg['skip']}")
    if a.fleurs:
        langs = list(FLEURS_TAGS) if a.fleurs == "all" else [x.strip() for x in a.fleurs.split(",") if x.strip()]
        for lang in langs:
            if lang not in FLEURS_TAGS:
                raise SystemExit(f"unknown language {lang!r}; known: {', '.join(FLEURS_TAGS)}")
        res = run_fleurs(a.config, cfg, langs, a.fleurs_language, a.force, a.metrics_only)
        for r in res:
            print(f"{r['lang']:<4}{r['mode']:<7}{r['tag']:<8}{r['metric']} {100 * r['er']:5.1f}  card {r['card']}")
        return
    sessions = load_sessions(a.sessions_file)
    if a.export_for_correction:
        export_for_correction(sessions[a.export_for_correction], a.config)
        return
    ids = list(sessions) if a.sessions == "all" else [x.strip() for x in a.sessions.split(",") if x.strip()]
    for sid in ids:
        if sid not in sessions:
            raise SystemExit(f"unknown session {sid!r}; known: {', '.join(sessions)}")

    data = None
    metas: dict[str, dict] = {}
    for sid in ids:
        s = sessions[sid]
        meta = None if a.force else cached(a.config, sid, cfg)
        if meta is None and not a.metrics_only:
            if not CLI.is_file():
                raise SystemExit(f"{CLI} missing: build diarist-cli once and copy it there (README)")
            data = data or prepare_data_root(a.config, cfg)
            meta = transcribe(s, a.config, cfg, data)
        if meta is None:
            log(f"[{sid}] no cached run for {a.config}; skipped")
            continue
        metas[sid] = meta
    if a.transcribe_only:
        return
    results = []
    for sid, meta in metas.items():
        log(f"[{sid}] scoring")
        results.append(score(sessions[sid], a.config, meta))
    if not results:
        raise SystemExit("nothing to score")
    print_summary(results)
    if not a.no_report:
        out = a.out or DOCS_EVAL / f"{dt.date.today().isoformat()}-{a.config}.md"
        write_report(a.config, cfg, results, metas, out)


if __name__ == "__main__":
    main()
