"""The phase control of docs/plans/18 section 3.5 (chunk N2), from cached and
scored runs: deletions, gaps and cpWER per session and pooled for a base
config and its shifted-input configs, the signed relative change of the
pooled deletions per offset, the band (the largest magnitude), and the gaps of
5 s or more that are new or gone against the base. Never touches the GPU.

  uv run python -I phase.py --base rs_en_shipped --configs shipped_en_ph70,shipped_en_ph20,shipped_en_ph40

Every config must have been scored (`run.py --metrics-only`), so that
metrics.json exists; the gap lists are recomputed from transcript.json with the
run's audio_shift_ms moved back, exactly as run.py's score() reads it.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import EVAL_DIR, interpolate_words, load_sessions  # noqa: E402
import metrics as M  # noqa: E402
import run as R  # noqa: E402

GATE = "aug2,aug9,sep7,sep5,seplog,aug14,ami"


def long_gaps(s, config: str) -> list[dict]:
    d = R.run_dir(config, s.id)
    meta = json.loads((d / "run.json").read_text(encoding="utf-8"))
    ref_segs, _, _ = R.load_reference(s)
    _, hyp_words = M.hyp_from_export(json.loads((d / "transcript.json").read_text(encoding="utf-8")), s.id)
    shift = int(meta.get("audio_shift_ms") or 0) / 1000.0
    for w in hyp_words:
        w["start_time"] -= shift
        w["end_time"] -= shift
    duration = meta["audio_s"] - shift
    region = R.scored_region(s, ref_segs, duration)
    if region:
        a, b = region
        hyp_words = [w for w in hyp_words if a <= (w["start_time"] + w["end_time"]) / 2 <= b]
    ref_w, hyp_w = interpolate_words(ref_segs), interpolate_words(hyp_words)
    t0, t1 = region or (0.0, duration)
    return [g for g in M.word_gaps(ref_w, hyp_w, t0, t1) if g["len"] >= M.GAP_LONG_S]


def overlaps(g: dict, gs: list[dict]) -> bool:
    return any(g["start"] < h["end"] and h["start"] < g["end"] for h in gs)


def main() -> None:
    sys.stdout.reconfigure(encoding="utf-8")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", required=True)
    ap.add_argument("--configs", required=True)
    ap.add_argument("--sessions", default=GATE)
    ap.add_argument("--json", type=Path, help="also write the numbers here")
    a = ap.parse_args()
    sessions = load_sessions(EVAL_DIR / "sessions.toml")
    ids = [x.strip() for x in a.sessions.split(",") if x.strip()]
    configs = [a.base] + [x.strip() for x in a.configs.split(",") if x.strip()]
    rows: dict[str, dict[str, dict]] = {c: {} for c in configs}
    for c in configs:
        for sid in ids:
            m = json.loads((R.run_dir(c, sid) / "metrics.json").read_text(encoding="utf-8"))
            rows[c][sid] = {k: m[k] for k in ("w_del", "w_ins", "w_sub", "wer", "cpwer", "gaps", "gaps_5s", "ref_words")}
    gaps = {c: {sid: long_gaps(sessions[sid], c) for sid in ids} for c in configs}
    out = {"base": a.base, "sessions": ids, "configs": {}}
    print("| Session | " + " | ".join(configs) + " |")
    print("|---|" + "---|" * len(configs))
    for sid in ids:
        cells = [f"{rows[c][sid]['w_del']} / {rows[c][sid]['gaps']} / {rows[c][sid]['gaps_5s']} / {100 * rows[c][sid]['cpwer']:.2f}"
                 for c in configs]
        print(f"| {sessions[sid].label} | " + " | ".join(cells) + " |")
    base_del = sum(rows[a.base][sid]["w_del"] for sid in ids)
    band = 0.0
    print("\n| Config | Deletions (pooled) | Δ vs base | Relative change | Gaps ≥ 2 s | Gaps ≥ 5 s | New ≥ 5 s | Gone ≥ 5 s | AMI cpWER |")
    print("|---|---|---|---|---|---|---|---|---|")
    for c in configs:
        dele = sum(rows[c][sid]["w_del"] for sid in ids)
        g2 = sum(rows[c][sid]["gaps"] for sid in ids)
        g5 = sum(rows[c][sid]["gaps_5s"] for sid in ids)
        new = sum(1 for sid in ids for g in gaps[c][sid] if not overlaps(g, gaps[a.base][sid]))
        gone = sum(1 for sid in ids for g in gaps[a.base][sid] if not overlaps(g, gaps[c][sid]))
        rel = (dele - base_del) / base_del
        if c != a.base:
            band = max(band, abs(rel))
        ami = rows[c].get("ami", {}).get("cpwer")
        print(f"| {c} | {dele} | {dele - base_del:+d} | {100 * rel:+.2f} % | {g2} | {g5} | {new} | {gone} | "
              f"{'-' if ami is None else f'{100 * ami:.2f}'} |")
        out["configs"][c] = {"deletions": dele, "relative": rel, "gaps": g2, "gaps_5s": g5, "new_5s": new,
                             "gone_5s": gone, "ami_cpwer": ami, "per_session": rows[c],
                             "new_5s_by_session": {sid: [g for g in gaps[c][sid] if not overlaps(g, gaps[a.base][sid])]
                                                   for sid in ids}}
    print(f"\nband: {100 * band:.2f} % (largest |relative change| of pooled deletions over {', '.join(configs[1:])})")
    out["band"] = band
    if a.json:
        a.json.write_text(json.dumps(out, indent=1) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
