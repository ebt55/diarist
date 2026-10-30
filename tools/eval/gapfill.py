"""Gap fill from cached runs (docs/plans/18-beta6.md section 3.3, chunk N1):
fill the stretches the primary model leaves without a word with the second
model's words, then score the fused words through the app's own file import,
without the GPU.

  # build the fused words JSON and replay it through the file import
  uv run python -I gapfill.py --sessions-file <sessions.toml> run --config en_fill_n35_g10 \
      --sessions all --source en=<runs>/rs_en_shipped --source n35=<runs>/rs_n35_shipped
  # then score it with the harness, as every eval table is scored
  uv run python -I run.py --config en_fill_n35_g10 --sessions-file <sessions.toml> --metrics-only --no-report
  # the decision rule of plan 18 section 3.3 over the scored configs
  uv run python -I gapfill.py --sessions-file <sessions.toml> rule --base en_replay --g-configs ...

The fill (plan 18 section 3.3, as measured and as it would ship), with the
points the plan leaves open fixed here before any run:

- Times are the import's: seconds from the runtime's words JSON, rounded to
  integer microseconds as `engine_words` does (`secs_to_us`), so a Rust port
  compares integers exactly as this file does.
- A stretch is a maximal interval of [0, audio end] that no primary word span
  [start, end] touches: from the start of the file to the first word, between
  words (from the latest end so far to the next start), and from the last
  word to the end of the audio (the words JSON `duration`, or the last word's
  end if later). A stretch qualifies when its length is at least G.
- A fill word is inserted when its whole span lies inside a qualifying stretch
  shrunk by 0.25 s at both ends: start >= stretch start + 0.25 s and
  end <= stretch end - 0.25 s.
- Speaker labels: every (primary word, fill word) pair whose spans overlap by
  at least 60 percent of each span (overlap >= 0.6 x the longer span, and > 0)
  adds its overlap in microseconds to the weight of (fill label, primary
  label). A one-to-one assignment (Hungarian, maximal total weight) maps fill
  labels to primary labels; an assigned pair with zero weight is not a
  mapping. Fill words inside the stretches have no primary word to overlap, so
  only co-timed words outside the gaps vote.
- A filled word takes its fill label's mapped primary label. A fill label
  with no mapping takes the label of the nearer of the two primary words that
  bound its stretch (the word whose end starts the stretch, distance from its
  end to the filled word's start; the word that ends the stretch, distance
  from the filled word's end to its start; a tie goes to the earlier word; a
  leading or trailing stretch has one bound), and the word is counted as
  `unmapped`.
- Nothing else changes: every primary word stays, with its text, times and
  label; the fused list is sorted by start (stable, primary words first on a
  tie, which the 0.25 s margin rules out).
- Report per session (run.json `gap_fill_report`, the fields of plan 18 3.4's
  `engine_info.gap_fill` that the measurement can know): `stretches` (those of
  at least G), `gaps` (stretches that received at least one word),
  `words_filled`, `unmapped` (filled words whose label came from the nearest
  primary word), the label `mapping`.
- The oracle (for the record only): per qualifying stretch, the fill words are
  inserted only when they score better against the reference than nothing
  does: the Levenshtein distance between the reference words whose midpoint
  lies in the stretch (unshrunk) and the fill words' normalised tokens is
  lower than the number of those reference words (a tie keeps the stretch
  empty).

Scoring: `run` writes, per session, the fused words JSON as
`.work/runs/<config>/<session>/engine.words.json`, imports it into the
config's scratch data root `.work/data/<config>` through
`pipeline::import_file` with a replay engine (the ignored test
`punct::onnx::tests::cli_check_root` of diarist-core, which chunk K used:
its words JSON in `DIARIST_PUNCT_CHECK_WORDS`), so the unknown-token check,
the loop collapse, punctuation and segmentation run as the app runs them;
then `diarist-cli export --format json` and `--format rttm`, and a run.json
with the sources' and the fused file's SHA-256. `run.py --metrics-only`
scores it like any cached run (configs with `replay` never transcribe).

Environment: DIARIST_EVAL_CLI (diarist-cli built from this tree),
DIARIST_EVAL_MODELS_DIR (a models folder holding the punctuation entry),
DIARIST_GAPFILL_TEST_BIN (the diarist-core test binary; default the newest
target/release/deps/diarist_core-*.exe of this tree).
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import EVAL_DIR, REPO, WORK, load_sessions, normalize_token, sha256_file  # noqa: E402

SHRINK_S = 0.25
PAIR_OVERLAP = 0.6
TEST_NAME = "punct::onnx::tests::cli_check_root"


# --------------------------------------------------------------------------
# The fill (pure; unit-tested in test_gapfill.py)


def to_us(s: float) -> int:
    """Seconds to microseconds as diarist-core's `secs_to_us`: negative
    clamped to 0, rounded half away from zero."""
    return int(math.floor(max(float(s), 0.0) * 1_000_000.0 + 0.5))


def spans(words: list[dict]) -> list[tuple[int, int]]:
    """(t0, t1) in microseconds per word, the end never before the start."""
    out = []
    for w in words:
        t0 = to_us(w["start"])
        out.append((t0, max(to_us(max(float(w["end"]), float(w["start"]))), t0)))
    return out


def stretches(sp: list[tuple[int, int]], end_us: int) -> list[tuple[int, int, int | None, int | None]]:
    """Maximal intervals of [0, end_us] that no span touches, as (start, end,
    index of the bounding word before, index of the bounding word after); the
    word before is the one whose end is the latest so far (the later one on a
    tie), None for a leading stretch; the word after is None for a trailing
    one."""
    order = sorted(range(len(sp)), key=lambda i: (sp[i][0], sp[i][1]))
    out = []
    cur, before = 0, None
    for i in order:
        a, b = sp[i]
        if a > cur:
            out.append((cur, a, before, i))
        if b >= cur:
            cur, before = b, i
    end = max(end_us, cur)
    if end > cur:
        out.append((cur, end, before, None))
    return out


def label_mapping(p_sp, p_lab, f_sp, f_lab, overlap: float = PAIR_OVERLAP) -> tuple[dict, dict]:
    """Fill label -> primary label, by maximal summed overlap of co-timed word
    pairs (each span overlapped by at least `overlap` of the longer span), one
    to one. Returns (mapping, weights {(fill label, primary label): us})."""
    import bisect

    import numpy as np
    from scipy.optimize import linear_sum_assignment

    f_order = sorted(range(len(f_sp)), key=lambda j: f_sp[j][0])
    f_starts = [f_sp[j][0] for j in f_order]
    longest_f = max((b - a for a, b in f_sp), default=0)
    weights: dict = {}
    for i, (a, b) in enumerate(p_sp):
        k = bisect.bisect_left(f_starts, a - longest_f)
        while k < len(f_order) and f_starts[k] < b:
            j = f_order[k]
            c, d = f_sp[j]
            ov = min(b, d) - max(a, c)
            if ov > 0 and ov >= overlap * max(b - a, d - c):
                key = (f_lab[j], p_lab[i])
                weights[key] = weights.get(key, 0) + ov
            k += 1
    fl = sorted({k[0] for k in weights}, key=repr)
    pl = sorted({k[1] for k in weights}, key=repr)
    mapping: dict = {}
    if fl and pl:
        m = np.zeros((len(fl), len(pl)), dtype=np.int64)
        for (x, y), v in weights.items():
            m[fl.index(x), pl.index(y)] = v
        rows, cols = linear_sum_assignment(-m)
        mapping = {fl[r]: pl[c] for r, c in zip(rows, cols) if m[r, c] > 0}
    return mapping, weights


def _lev(a: list[str], b: list[str]) -> int:
    from rapidfuzz.distance import Levenshtein

    return Levenshtein.distance(a, b)


def fill(
    primary: dict, second: dict, g_s: float, *, shrink_s: float = SHRINK_S, overlap: float = PAIR_OVERLAP,
    oracle_ref: list[tuple[str, float]] | None = None,
) -> tuple[dict, dict]:
    """Fuses two runtime words JSONs ({duration, words: [{word, start, end,
    speaker}]}): the primary's words, plus the second's words inside the
    primary's stretches of at least g_s, relabelled (module docstring).
    oracle_ref: (normalised reference word, midpoint s) pairs; when given, a
    stretch is filled only where its words beat leaving it empty.
    Returns (fused words JSON, report)."""
    pw, fw = primary.get("words", []), second.get("words", [])
    p_sp, f_sp = spans(pw), spans(fw)
    p_lab = [w.get("speaker") for w in pw]
    f_lab = [w.get("speaker") for w in fw]
    end_us = max([to_us(primary.get("duration") or 0.0)] + [b for _, b in p_sp])
    g_us, shrink = to_us(g_s), to_us(shrink_s)
    mapping, weights = label_mapping(p_sp, p_lab, f_sp, f_lab, overlap)
    qual = [s for s in stretches(p_sp, end_us) if s[1] - s[0] >= g_us]
    f_order = sorted(range(len(fw)), key=lambda j: (f_sp[j][0], f_sp[j][1], j))
    f_starts = [f_sp[j][0] for j in f_order]
    import bisect

    if oracle_ref is not None:
        ref_mid = sorted(oracle_ref, key=lambda x: x[1])
        ref_t = [m for _, m in ref_mid]
    inserted: list[dict] = []
    gaps = unmapped = declined = 0
    for a, b, before, after in qual:
        lo, hi = a + shrink, b - shrink
        k = bisect.bisect_left(f_starts, lo)
        cand = []
        while k < len(f_order) and f_starts[k] <= hi:
            j = f_order[k]
            if f_sp[j][1] <= hi:
                cand.append(j)
            k += 1
        if not cand:
            continue
        if oracle_ref is not None:
            r0, r1 = bisect.bisect_left(ref_t, a / 1e6), bisect.bisect_right(ref_t, b / 1e6)
            ref_in = [w for w, _ in ref_mid[r0:r1]]
            hyp_in = [t for j in cand for t in normalize_token(str(fw[j]["word"]))]
            if _lev(ref_in, hyp_in) >= len(ref_in):
                declined += 1
                continue
        gaps += 1
        for j in cand:
            lab = f_lab[j]
            if lab in mapping:
                new = mapping[lab]
            else:
                unmapped += 1
                c, d = f_sp[j]
                if before is None and after is None:
                    new = lab
                elif after is None or (before is not None and (c - a) <= (b - d)):
                    new = p_lab[before]
                else:
                    new = p_lab[after]
            w = dict(fw[j])
            w["speaker"] = new
            inserted.append(w)
    fused = sorted(list(pw) + inserted, key=lambda w: to_us(w["start"]))
    doc = {k: v for k, v in primary.items() if k != "words"}
    doc["text"] = " ".join(str(w["word"]) for w in fused)
    doc["words"] = fused
    report = {
        "g_s": g_s,
        "shrink_s": shrink_s,
        "pair_overlap": overlap,
        "oracle": oracle_ref is not None,
        "primary_words": len(pw),
        "fill_words": len(fw),
        "stretches": len(qual),
        "gaps": gaps,
        "words_filled": len(inserted),
        "unmapped": unmapped,
        "declined": declined if oracle_ref is not None else None,
        "mapping": {str(k): v for k, v in sorted(mapping.items(), key=lambda kv: repr(kv[0]))},
        "unmapped_labels": sorted({str(x) for x in f_lab if x not in mapping}),
        "pair_weight_us": sum(weights.values()),
    }
    return doc, report


# --------------------------------------------------------------------------
# Building, replaying and recording one config


def _src(sources: dict[str, Path], name: str, sid: str) -> tuple[dict, dict, Path]:
    d = sources[name] / sid
    words = d / "engine.words.json"
    meta = json.loads((d / "run.json").read_text(encoding="utf-8"))
    return json.loads(words.read_text(encoding="utf-8")), meta, words


def oracle_reference(s, meta: dict) -> list[tuple[str, float]]:
    import run as R
    from common import interpolate_words

    ref_segs, _, _ = R.load_reference(s)
    return [(w.word, (w.start + w.end) / 2) for w in interpolate_words(ref_segs, meta.get("language") or s.language)]


def build(config: str, cfg: dict, s, sources: dict[str, Path]) -> tuple[Path, dict]:
    """Writes runs/<config>/<session>/engine.words.json (the fused words, or
    the primary's own words for a config without gap_fill) and returns its
    path and the build record for run.json."""
    import run as R

    prim_doc, prim_meta, prim_path = _src(sources, cfg["replay"], s.id)
    rec: dict = {"primary": {"model": cfg["replay"], "words": str(prim_path), "sha256": sha256_file(prim_path),
                             "run_cli_sha256": prim_meta.get("cli_sha256"), "config": prim_meta.get("config")}}
    gf = cfg.get("gap_fill")
    if gf:
        sec_doc, sec_meta, sec_path = _src(sources, gf["fill"], s.id)
        rec["fill"] = {"model": gf["fill"], "words": str(sec_path), "sha256": sha256_file(sec_path),
                       "run_cli_sha256": sec_meta.get("cli_sha256"), "config": sec_meta.get("config")}
        ref = oracle_reference(s, prim_meta) if gf.get("oracle") else None
        doc, report = fill(prim_doc, sec_doc, float(gf["g_s"]), oracle_ref=ref)
        rec["gap_fill_report"] = report
    else:
        doc = prim_doc
    d = R.run_dir(config, s.id)
    d.mkdir(parents=True, exist_ok=True)
    out = d / "engine.words.json"
    out.write_text(json.dumps(doc, ensure_ascii=False) + "\n", encoding="utf-8")
    rec["words_sha256"] = sha256_file(out)
    rec["audio_s"] = prim_meta.get("audio_s")
    rec["source_meta"] = {k: prim_meta.get(k) for k in ("engine_version", "asr_file", "diarizer_file", "device",
                                                         "right_context", "diar_preset", "language")}
    return out, rec


def test_binary() -> Path:
    env = os.environ.get("DIARIST_GAPFILL_TEST_BIN")
    if env:
        return Path(env)
    cands = sorted((REPO / "target" / "release" / "deps").glob("diarist_core-*.exe"), key=lambda p: p.stat().st_mtime)
    if not cands:
        raise SystemExit("no diarist-core test binary: cargo test --release -p diarist-core --lib --no-run")
    return cands[-1]


def prepare_root(config: str, cfg: dict) -> Path:
    """The config's scratch data root: models scanned from the eval models
    folder, the punctuation model selected when the config punctuates, and
    the two transcript settings written as run.py writes them."""
    import run as R

    data = WORK / "data" / config
    data.mkdir(parents=True, exist_ok=True)
    if not (data / "diarist.db").is_file():
        R.cli(data, "models", "scan")
    if R.punctuation_of(cfg):
        R.cli(data, "models", "select", "punctuation", R.punctuation_model(cfg))
    R.set_setting(data, "transcript.punctuation", "auto" if R.punctuation_of(cfg) else "off")
    R.set_setting(data, "transcript.collapse_loops", "on" if R.collapse_loops_of(cfg) else "off")
    return data


def engine_info(data: Path, meeting_id: int) -> dict:
    import sqlite3

    con = sqlite3.connect(data / "diarist.db")
    try:
        row = con.execute(
            "SELECT r.engine_info FROM meetings m JOIN transcript_runs r ON r.id = m.current_run_id WHERE m.id = ?",
            (meeting_id,),
        ).fetchone()
    finally:
        con.close()
    return json.loads(row[0]) if row and row[0] else {}


def replay(config: str, cfg: dict, s, words: Path, rec: dict, data: Path, binary: Path) -> dict:
    """Imports the words JSON through the file import (replay engine), exports
    the meeting and writes run.json."""
    import run as R

    d = R.run_dir(config, s.id)
    env = {**os.environ, "DIARIST_PUNCT_CHECK_ROOT": str(data), "DIARIST_PUNCT_CHECK_WORDS": str(words)}
    p = subprocess.run(
        [str(binary), TEST_NAME, "--ignored", "--exact", "--nocapture", "--test-threads=1"],
        capture_output=True, text=True, encoding="utf-8", errors="replace", env=env,
    )
    m = re.search(r"^meeting (\d+) \((\d+) words from ", p.stdout, re.M)
    if p.returncode != 0 or not m:
        raise RuntimeError(f"[{s.id}] replay import failed ({p.returncode}):\n{p.stdout[-1500:]}\n{p.stderr[-1500:]}")
    mid = int(m.group(1))
    R.cli(data, "export", str(mid), "--format", "json", "--out", str(d / "transcript.json"))
    R.cli(data, "export", str(mid), "--format", "rttm", "--out", str(d / "hyp.rttm"))
    info = engine_info(data, mid)
    loops = info.get("loops") if isinstance(info.get("loops"), dict) else None
    punct = info.get("punctuation") if isinstance(info.get("punctuation"), dict) else None
    meta = {
        "schema": R.RUN_SCHEMA,
        "status": "ok",
        "session": s.id,
        "config": config,
        "meeting_id": mid,
        "stored_words": int(m.group(2)),
        "audio_s": rec["audio_s"],
        "language": rec["source_meta"].get("language") or cfg["language"],
        "asr": cfg["asr"],
        "diarizer": cfg["diarizer"],
        "engine_args": list(cfg.get("engine_args") or []),
        "punctuation": R.punctuation_of(cfg),
        "collapse_loops": R.collapse_loops_of(cfg),
        "loops": {k: loops.get(k) for k in ("applied", "runs", "words_removed")} if loops else None,
        "punctuation_report": {k: punct.get(k) for k in ("applied", "reason", "unit", "rejected") if k in punct}
        if punct else None,
        "replay": cfg["replay"],
        "gap_fill": cfg.get("gap_fill"),
        "diar_pass": False,
        **{k: v for k, v in rec["source_meta"].items() if k != "language"},
        "sources": {k: rec[k] for k in ("primary", "fill") if k in rec},
        "words_sha256": rec["words_sha256"],
        "gap_fill_report": rec.get("gap_fill_report"),
        "test_bin": binary.name,
        "test_bin_sha256": sha256_file(binary),
        "cli_sha256": sha256_file(R.CLI),
        "finished": dt.datetime.now().isoformat(timespec="seconds"),
        # No speed or VRAM: nothing was transcribed.
        "wall_s": None,
        "asr_wall_s": None,
    }
    (d / "run.json").write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
    return meta


def cmd_run(a) -> None:
    import run as R

    configs = tomllib.loads(a.configs_file.read_text(encoding="utf-8"))
    cfg = configs.get(a.config)
    if not cfg or not cfg.get("replay"):
        raise SystemExit(f"{a.config} is not a replay config in {a.configs_file}")
    sources = {}
    for spec in a.source:
        k, _, v = spec.partition("=")
        sources[k] = Path(v)
    sessions = load_sessions(a.sessions_file)
    ids = list(sessions) if a.sessions == "all" else [x.strip() for x in a.sessions.split(",") if x.strip()]
    if a.groups:
        groups = {g.strip() for g in a.groups.split(",")}
        ids = [i for i in ids if sessions[i].group in groups]
    binary = test_binary()
    data = prepare_root(a.config, cfg)
    for sid in ids:
        s = sessions[sid]
        lang = R.session_language(s, cfg)
        if not R.asr_covers(cfg, lang):
            continue
        if not (sources[cfg["replay"]] / sid / "engine.words.json").is_file():
            print(f"[{sid}] no cached words for {cfg['replay']}; skipped", file=sys.stderr)
            continue
        words, rec = build(a.config, cfg, s, sources)
        old = R.run_dir(a.config, sid) / "run.json"
        if not a.force and old.is_file():
            try:
                prev = json.loads(old.read_text(encoding="utf-8"))
            except ValueError:
                prev = {}
            if prev.get("status") == "ok" and prev.get("words_sha256") == rec["words_sha256"]:
                print(f"[{sid}] {a.config}: replay cached", file=sys.stderr)
                continue
        meta = replay(a.config, cfg, s, words, rec, data, binary)
        r = meta.get("gap_fill_report") or {}
        print(
            f"[{sid}] {a.config}: meeting {meta['meeting_id']}, {meta['stored_words']} words stored"
            + (f", {r['words_filled']} filled in {r['gaps']} gaps ({r['unmapped']} unmapped)" if r else "")
            + f", loops {(meta.get('loops') or {}).get('words_removed')}",
            file=sys.stderr,
        )


# --------------------------------------------------------------------------
# The decision rule (plan 18 section 3.3), over metrics.json of scored configs

GATE_GROUPS = {"tuning", "heldout", "control", "stress", "overlap"}
ENGLISH_PUBLIC = {"pub-meeting", "pub-far", "pub-phone"}


def _metrics(config: str, sid: str) -> dict | None:
    try:
        return json.loads((WORK / "runs" / config / sid / "metrics.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def pooled(ms: list[dict]) -> dict:
    s = lambda k: sum((m.get(k) or 0) for m in ms)  # noqa: E731
    return {
        "n": len(ms),
        "min": s("audio_s") / 60,
        "cp_errors": s("cp_errors"),
        "cp_length": s("cp_length"),
        "cpwer": s("cp_errors") / s("cp_length") if s("cp_length") else float("nan"),
        "tcpwer": s("tcp_errors") / s("tcp_length") if s("tcp_length") else float("nan"),
        "wer": (s("w_sub") + s("w_del") + s("w_ins")) / s("ref_words") if s("ref_words") else float("nan"),
        "w_del": s("w_del"), "w_ins": s("w_ins"), "w_sub": s("w_sub"),
        "wder": sum(m["wder"] * m["wder_pairs"] for m in ms if m.get("wder_pairs")) / s("wder_pairs")
        if s("wder_pairs") else float("nan"),
        "gaps": s("gaps"), "gaps_5s": s("gaps_5s"),
    }


def _pc(x: float) -> str:
    return "–" if x is None or x != x else f"{100 * x:.2f}"


def _rows(config: str, sessions: dict, ids: list[str]) -> dict[str, dict]:
    out = {}
    for sid in ids:
        m = _metrics(config, sid)
        if m:
            out[sid] = m
    return out


def cmd_rule(a) -> None:
    """Prints the tables and the verdict of plan 18 section 3.3's rule. Reads
    only metrics.json files; the bars are the plan's, quoted in the output."""
    sessions = load_sessions(a.sessions_file)
    gate = [i for i, s in sessions.items() if s.group in GATE_GROUPS]
    allz = [i for i, s in sessions.items() if s.group in GATE_GROUPS | {"broad", "talks"}]
    talks = [i for i, s in sessions.items() if s.group == "talks"]
    base = a.base
    out: dict = {"base": base}

    def need(config, ids):
        rows = _rows(config, sessions, ids)
        missing = [i for i in ids if i not in rows]
        if missing:
            raise SystemExit(f"{config}: no metrics.json for {', '.join(missing)}")
        return rows

    # Rule 1: G on the gate set only.
    b_gate = need(base, gate)
    pick = None
    print("## Rule 1: G on the gate set (7 sessions), EN primary\n")
    print("| Config | G s | Sessions | cp errors | cpWER | Δ vs EN alone | WER | del | gaps ≥ 2 s | gaps ≥ 5 s |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    pb = pooled(list(b_gate.values()))
    print(f"| {base} | – | {pb['n']} | {pb['cp_errors']} | {_pc(pb['cpwer'])} | – | {_pc(pb['wer'])} | {pb['w_del']} | "
          f"{pb['gaps']} | {pb['gaps_5s']} |")
    cand = []
    for spec in a.g_configs.split(","):
        c, _, g = spec.partition("=")
        p = pooled(list(need(c, gate).values()))
        cand.append((p["cp_errors"], float(g), c))
        print(f"| {c} | {g} | {p['n']} | {p['cp_errors']} | {_pc(p['cpwer'])} | {_pc(p['cpwer'] - pb['cpwer'])} | "
              f"{_pc(p['wer'])} | {p['w_del']} | {p['gaps']} | {p['gaps_5s']} |")
    # Lowest pooled cpWER (the reference length is the same for every G, so
    # the error counts decide), ties to the smaller G.
    cand.sort(key=lambda x: (x[0], x[1]))
    pick = cand[0][2]
    out["pick"] = {"config": pick, "g_s": cand[0][1]}
    print(f"\nPick: {pick} (G = {cand[0][1]} s)\n")
    if a.stop_after_pick:
        print(json.dumps(out, indent=2))
        return

    # Rule 2: the 36 sessions with that G.
    b_all, f_all = need(base, allz), need(pick, allz)
    print("## Rule 2: the 36 Zoom and AMI sessions\n")
    print("| Session | Group | cpWER EN | cpWER fill | Δ | WDER EN | WDER fill | Δ | del EN | del fill | gaps ≥ 5 s EN | fill |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|")
    worst_cp = worst_wder = -1.0
    worst_cp_s = worst_wder_s = None
    for sid in allz:
        b, f = b_all[sid], f_all[sid]
        dcp, dw = f["cpwer"] - b["cpwer"], f["wder"] - b["wder"]
        if dcp > worst_cp:
            worst_cp, worst_cp_s = dcp, sid
        if dw > worst_wder:
            worst_wder, worst_wder_s = dw, sid
        print(f"| {sessions[sid].label} | {sessions[sid].group} | {_pc(b['cpwer'])} | {_pc(f['cpwer'])} | {_pc(dcp)} | "
              f"{_pc(b['wder'])} | {_pc(f['wder'])} | {_pc(dw)} | {b['w_del']} | {f['w_del']} | {b['gaps_5s']} | {f['gaps_5s']} |")
    pb, pf = pooled(list(b_all.values())), pooled(list(f_all.values()))
    gain = pb["cpwer"] - pf["cpwer"]
    ami = "ami"
    g5_drop = (pb["gaps_5s"] - pf["gaps_5s"]) / pb["gaps_5s"] if pb["gaps_5s"] else float("nan")
    bars = [
        ("pooled cpWER lower than EN alone by at least 0.2 points", gain >= 0.002,
         f"{_pc(pb['cpwer'])} → {_pc(pf['cpwer'])}, gain {_pc(gain)} points"),
        ("no session worse by more than 0.3", worst_cp <= 0.003,
         f"largest change {_pc(worst_cp)} points ({sessions[worst_cp_s].label})"),
        ("AMI ES2002a's cpWER is lower", f_all[ami]["cpwer"] < b_all[ami]["cpwer"],
         f"{_pc(b_all[ami]['cpwer'])} → {_pc(f_all[ami]['cpwer'])}"),
        ("gaps of 5 s or more at least 30 percent fewer", g5_drop >= 0.30,
         f"{pb['gaps_5s']} → {pf['gaps_5s']} ({100 * g5_drop:.1f} percent fewer)"),
        ("WDER on no session worse by more than 0.5 points", worst_wder <= 0.005,
         f"largest change {_pc(worst_wder)} points ({sessions[worst_wder_s].label})"),
    ]
    print("\n| Set | Config | Sessions | Min | cpWER | tcpWER | WER | del | ins | sub | WDER | gaps ≥ 2 s | gaps ≥ 5 s |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for name, ids in (("gate", gate), ("broad", [i for i in allz if sessions[i].group == "broad"]), ("talks", talks),
                      ("all 36", allz)):
        for c, rows in ((base, b_all), (pick, f_all)):
            p = pooled([rows[i] for i in ids])
            print(f"| {name} | {c} | {p['n']} | {p['min']:.0f} | {_pc(p['cpwer'])} | {_pc(p['tcpwer'])} | {_pc(p['wer'])} | "
                  f"{p['w_del']} | {p['w_ins']} | {p['w_sub']} | {_pc(p['wder'])} | {p['gaps']} | {p['gaps_5s']} |")
    print("\n| Bar (rule 2) | Result | |\n|---|---|---|")
    for name, ok, txt in bars:
        print(f"| {name} | {txt} | {'met' if ok else 'NOT met'} |")
    rule2 = all(ok for _, ok, _ in bars)
    out["rule2"] = {"pass": rule2, "gain_points": 100 * gain, "bars": [(n, ok, t) for n, ok, t in bars]}

    # Rule 3: the held-out public English groups, read once.
    if a.public_sessions_file:
        pub_sessions = load_sessions(a.public_sessions_file)
        pub = [i for i, s in pub_sessions.items() if s.group in ENGLISH_PUBLIC]
        pb_rows, pf_rows = _rows(base, pub_sessions, pub), _rows(pick, pub_sessions, pub)
        missing = [i for i in pub if i not in pb_rows or i not in pf_rows]
        if missing:
            raise SystemExit(f"public: no metrics.json for {', '.join(missing)}")
        print("\n## Rule 3: held-out public English groups (52 sessions)\n")
        print("| Set | Config | Sessions | Min | cpWER | tcpWER | WER | del | ins | sub | WDER | gaps ≥ 2 s | gaps ≥ 5 s |")
        print("|---|---|---|---|---|---|---|---|---|---|---|---|---|")
        for g in ("pub-meeting", "pub-far", "pub-phone", "English"):
            ids = [i for i in pub if pub_sessions[i].group == g] if g != "English" else pub
            for c, rows in ((base, pb_rows), (pick, pf_rows)):
                p = pooled([rows[i] for i in ids])
                print(f"| {g} | {c} | {p['n']} | {p['min']:.0f} | {_pc(p['cpwer'])} | {_pc(p['tcpwer'])} | {_pc(p['wer'])} | "
                      f"{p['w_del']} | {p['w_ins']} | {p['w_sub']} | {_pc(p['wder'])} | {p['gaps']} | {p['gaps_5s']} |")
        qb, qf = pooled(list(pb_rows.values())), pooled(list(pf_rows.values()))
        r3 = [
            ("pooled cpWER not higher than EN alone", qf["cpwer"] <= qb["cpwer"],
             f"{_pc(qb['cpwer'])} → {_pc(qf['cpwer'])}"),
            ("deletions fewer", qf["w_del"] < qb["w_del"], f"{qb['w_del']} → {qf['w_del']}"),
        ]
        print("\n| Bar (rule 3) | Result | |\n|---|---|---|")
        for name, ok, txt in r3:
            print(f"| {name} | {txt} | {'met' if ok else 'NOT met'} |")
        out["rule3"] = {"pass": all(ok for _, ok, _ in r3), "bars": [(n, ok, t) for n, ok, t in r3]}
    passes = out["rule2"]["pass"] and out.get("rule3", {}).get("pass", False)
    if not passes:
        verdict = "closed: the fill does not ship (chunk N3 does not start)"
    elif out["rule2"]["gain_points"] >= 0.3:
        verdict = "passes: N3, transcript.gap_fill = auto by default (rule 4, gain at least 0.3 points)"
    else:
        verdict = "passes: N3, transcript.gap_fill = off by default and offered in Settings (rule 4, gain 0.2 to 0.3)"
    out["verdict"] = verdict
    print(f"\nVerdict: {verdict}")
    if a.json_out:
        Path(a.json_out).write_text(json.dumps(out, indent=2, default=str) + "\n", encoding="utf-8")


def main() -> None:
    sys.stdout.reconfigure(encoding="utf-8")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--sessions-file", type=Path, default=EVAL_DIR / "sessions.toml")
    ap.add_argument("--configs-file", type=Path, default=EVAL_DIR / "configs.toml")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("run", help="build the words JSON of a replay config and import it (no GPU)")
    p.add_argument("--config", required=True)
    p.add_argument("--sessions", default="all")
    p.add_argument("--groups")
    p.add_argument("--source", action="append", required=True,
                   help="NAME=<runs dir of a cached config>, e.g. en=.work/runs/rs_en_shipped (read only)")
    p.add_argument("--force", action="store_true")
    p = sub.add_parser("rule", help="plan 18 section 3.3's decision rule over scored configs")
    p.add_argument("--base", default="en_replay")
    p.add_argument("--g-configs", default="en_fill_n35_g10=1.0,en_fill_n35_g15=1.5,en_fill_n35_g20=2.0")
    p.add_argument("--public-sessions-file", type=Path)
    p.add_argument("--stop-after-pick", action="store_true")
    p.add_argument("--json-out")
    a = ap.parse_args()
    {"run": cmd_run, "rule": cmd_rule}[a.cmd](a)


if __name__ == "__main__":
    main()
