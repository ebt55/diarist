"""Run meet-cli over the eval sessions, score against references, write the
results table to docs/eval/<yyyy-mm-dd>-<config>.md.

  uv run python -I run.py --config nemotron35 --sessions aug2,aug9,seplog,ami
  uv run python -I run.py --config nemotron35 --sessions all
  uv run python -I run.py --config nemotron35 --sessions all --metrics-only
  uv run python -I run.py --config nemotron35 --export-for-correction aug2

Per session: the audio is converted once to 16 kHz mono WAV (.work/wav), then
`meet-cli transcribe --rttm` runs into the config's scratch data root
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
import metrics as M  # noqa: E402
import zoom_vtt_to_ref  # noqa: E402

CLI = Path(os.environ.get("MEET_EVAL_CLI", BIN / "meet-cli.exe"))
ENGINE_DIR = Path(
    os.environ.get(
        "MEET_EVAL_ENGINE_DIR", REPO / "engine" / "nemo-speech" / "nemo-speech-0.2.0-windows-x86_64-cuda"
    )
)
MODELS_DIR = Path(os.environ.get("MEET_EVAL_MODELS_DIR", REPO / "models"))
RUN_SCHEMA = 1


def log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


# --------------------------------------------------------------------------
# Audio and meet-cli


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
        raise RuntimeError(f"meet-cli {' '.join(args[:2])} failed ({p.returncode}): {(p.stderr or '')[-2000:]}")
    return p


def prepare_data_root(config: str, cfg: dict) -> Path:
    data = WORK / "data" / config
    data.mkdir(parents=True, exist_ok=True)
    if not (data / "meet-assist.db").is_file():
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
    if any(meta.get(k) != cfg.get(k) for k in ("asr", "diarizer", "language")):
        log(f"[{sid}] cached run used different settings; transcribing again")
        return None
    if CLI.is_file() and meta.get("cli_sha256") != sha256_file(CLI):
        log(f"[{sid}] note: cached run was made by a different meet-cli build (use --force to redo)")
    return meta


def transcribe(s: Session, config: str, cfg: dict, data: Path) -> dict:
    d = run_dir(config, s.id)
    d.mkdir(parents=True, exist_ok=True)
    wav, duration = wav_for(s)
    log(f"[{s.id}] meet-cli transcribe ({duration / 60:.1f} min, config {config})")
    t0 = time.perf_counter()
    p = cli(
        data, "transcribe", str(wav), "--language", cfg["language"], "--title", s.id, "--rttm",
        "--out", str(d / "transcript.json"), "--format", "json",
    )
    wall = time.perf_counter() - t0
    m = re.search(r"^meeting (\d+) \((\w+)\)", p.stdout, re.M)
    if not m:
        raise RuntimeError(f"cannot find the meeting id in meet-cli output:\n{p.stdout}")
    mid, uid = m.group(1), m.group(2)
    cli(data, "export", mid, "--format", "rttm", "--out", str(d / "hyp.rttm"))
    audio_dir = data / "audio" / uid
    for name, dest in (
        ("diarization.rttm", "diarizer.rttm"),
        ("diarization.json", "diarizer.json"),
        ("engine.log", "engine.log"),
    ):
        if (audio_dir / name).is_file():
            shutil.copyfile(audio_dir / name, d / dest)
    eng = re.search(r"^engine (\S+) \| asr (.+?) \| diarizer (.+?)\s*$", p.stderr, re.M)
    meta = {
        "schema": RUN_SCHEMA,
        "status": "ok",
        "session": s.id,
        "config": config,
        "meeting_id": int(mid),
        "meeting_uid": uid,
        "wall_s": round(wall, 1),
        "audio_s": round(duration, 3),
        "language": cfg["language"],
        "asr": cfg["asr"],
        "diarizer": cfg["diarizer"],
        "engine_version": eng.group(1) if eng else None,
        "asr_file": eng.group(2) if eng else None,
        "diarizer_file": eng.group(3) if eng else None,
        "cli_sha256": sha256_file(CLI),
        "finished": dt.datetime.now().isoformat(timespec="seconds"),
        "cli_stdout": [ln for ln in p.stdout.splitlines() if not ln.strip().startswith("title")],
    }
    (d / "run.json").write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
    log(f"[{s.id}] done in {wall:.0f} s ({duration / max(wall, 1e-9):.1f}x real time)")
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
    """The nemo-speech command lines meet-core ran, from a run's engine.log,
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
        f"- meet-cli: `{CLI.name}` sha256 `{(any_meta.get('cli_sha256') or sha256_file(CLI))[:16]}` (built once from this repo at the start of Phase 4; see tools/eval/.bin)",
        f"- Engine: nemo-speech {any_meta.get('engine_version') or '?'} (`{ENGINE_DIR.name}`)",
        f"- ASR: {asr.get('name')} `{asr.get('repo')}` rev `{asr.get('revision', '')[:12]}` sha256 `{asr.get('sha256', '')[:16]}`"
        f" (local .verified matches: {asr.get('verified')})",
        f"- Diarizer: {diar.get('name')} `{diar.get('repo')}` rev `{diar.get('revision', '')[:12]}` sha256 `{diar.get('sha256', '')[:16]}`"
        f" (local .verified matches: {diar.get('verified')}); preset v3-streaming (runtime default)",
        f"- meet-cli flags: `transcribe <16 kHz wav> --language {cfg['language']} --rttm --format json --out ...`, then `export --format rttm`",
        "- Engine command lines (from meet-core's engine.log): "
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
        "| Session | Ref source | Ref words | Hyp words | Aligned | Near-change words | Ref changes | ORC-WER | Wall s | × real time |",
        "|---|---|---|---|---|---|---|---|---|---|",
    ]
    for r in results:
        rt = r["audio_s"] / r["wall_s"] if r.get("wall_s") else None
        lines.append(
            f"| {r['label']} | {r['ref_source']} | {r['ref_words']} | {r['hyp_words']} | {r['wder_pairs']} | "
            f"{r['wder_near_pairs']} | {r['ref_changes']} | {pct(r.get('orcwer'))} | {r.get('wall_s') or '–'} | "
            f"{f'{rt:.1f}' if rt else '–'} |"
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


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--config", required=True)
    ap.add_argument("--sessions", default="all", help="comma-separated session ids, or all")
    ap.add_argument("--sessions-file", type=Path, default=EVAL_DIR / "sessions.toml")
    ap.add_argument("--configs-file", type=Path, default=EVAL_DIR / "configs.toml")
    ap.add_argument("--force", action="store_true", help="re-transcribe even when cached")
    ap.add_argument("--transcribe-only", action="store_true")
    ap.add_argument("--metrics-only", action="store_true", help="never run the GPU; skip sessions without a cached run")
    ap.add_argument("--export-for-correction", metavar="SESSION")
    ap.add_argument("--out", type=Path, help="report path (default docs/eval/<date>-<config>.md)")
    ap.add_argument("--no-report", action="store_true")
    a = ap.parse_args()

    configs = tomllib.loads(a.configs_file.read_text(encoding="utf-8"))
    if a.config not in configs:
        raise SystemExit(f"unknown config {a.config!r}; known: {', '.join(configs)}")
    cfg = configs[a.config]
    if cfg.get("skip"):
        raise SystemExit(f"config {a.config} is not runnable: {cfg['skip']}")
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
                raise SystemExit(f"{CLI} missing: build meet-cli once and copy it there (README)")
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
