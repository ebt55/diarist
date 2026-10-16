"""Analyses for the accuracy-lever spike (docs/research/12-accuracy-levers.md),
over runs that run.py or nemo_direct.py already cached. Never touches the GPU.

  uv run python -I levers.py offsets --base nemotron35 --configs n35_rc_3,n35_rc_6 --sessions ami,aug2,sep5
  uv run python -I levers.py tags --configs nemotron35,n35_rc_3 --session aug14
  uv run python -I levers.py diff --a n35_rc_3 --b n35_rc_13 --sessions ami,aug2,sep5
  uv run python -I levers.py ami-misses --words .work/spikes/boost/n35_base/words.json --offset 50 --dur 300
  uv run python -I levers.py clip-score --words a/words.json b/words.json --offset 50 --dur 300
  uv run python -I levers.py punct --words <words.json> [...]
  uv run python -I levers.py table --configs nemotron35,n35_rc_3,nemotron_en,en_rc_13
  uv run python -I levers.py boost --base .work/spikes/boost/n35_base --runs .work/spikes/boost/n35_b3 --phrases Craig beagle

offsets: word-start offset of each config against the base config on the
  same session: normalised words aligned by Levenshtein, starts compared on
  the equal pairs (positive = later than the base). For AMI also against the
  manual word times of the reference.
tags: agreement of the runtime's word speaker tags (engine.words.json) with
  the standalone diarizer RTTM of the same run (research/08 section 7): each
  word takes the diarizer speaker with the most overlap with its span, labels
  are matched one to one (Hungarian), agreement = matching words / tagged
  words that the diarizer covers.
diff: word counts and edit operations between two configs' transcripts.
ami-misses: reference words of the AMI clip that the hypothesis gets wrong,
  most frequent first (candidates for --speech-context).
clip-score: WER and cpWER of runtime words JSONs on an AMI clip.
punct: share of words carrying punctuation or a capital, per words JSON.
boost: phrase hits, changes against the unboosted run, clip WER and the
  runtime's boosting messages, per nemo_direct run.
table: the lookahead summary, one row per config (quality on the short
  sessions, speed, and on the 77-minute session VRAM, tag agreement and the
  word-start offset against the same model's R = 1 config).
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import EVAL_DIR, WORK, load_sessions, normalize_token, read_rttm, read_seglst  # noqa: E402

RUNS = WORK / "runs"


def raw_words(path: Path) -> list[dict]:
    """Runtime words JSON (engine.words.json / nemo_direct words.json):
    [{word, start, end, speaker?}]."""
    return json.loads(path.read_text(encoding="utf-8")).get("words", [])


def norm_seq(words: list[dict], key: str = "word") -> list[tuple[str, float, float, object]]:
    """(normalised word, start, end, speaker) per normalised token; a raw word
    that normalises to several tokens shares its start."""
    out = []
    for w in words:
        for t in normalize_token(str(w[key])):
            out.append((t, float(w["start"]), float(w["end"]), w.get("speaker")))
    return out


def equal_pairs(a: list, b: list) -> list[tuple[int, int]]:
    from rapidfuzz.distance import Levenshtein

    pairs = []
    for op in Levenshtein.opcodes([x[0] for x in a], [x[0] for x in b]):
        if op.tag == "equal":
            pairs.extend((op.src_start + k, op.dest_start + k) for k in range(op.src_end - op.src_start))
    return pairs


def offset_stats(base: list, other: list) -> dict:
    pairs = equal_pairs(base, other)
    d = [other[j][1] - base[i][1] for i, j in pairs]
    if not d:
        return {"pairs": 0}
    return {
        "pairs": len(d),
        "mean_ms": round(1000 * statistics.fmean(d)),
        "median_ms": round(1000 * statistics.median(d)),
        "p90_abs_ms": round(1000 * sorted(abs(x) for x in d)[int(0.9 * (len(d) - 1))]),
    }


def ami_ref_words(sessions_file: Path) -> list[tuple[str, float, float, object]]:
    s = load_sessions(sessions_file)["ami"]
    rows = read_seglst(s.ref_seglst)
    words = [{"word": r["words"], "start": r["start_time"], "end": r["end_time"], "speaker": r["speaker"]} for r in rows]
    words.sort(key=lambda w: w["start"])
    return norm_seq(words)


def cmd_offsets(a) -> None:
    sessions = [x.strip() for x in a.sessions.split(",") if x.strip()]
    configs = [x.strip() for x in a.configs.split(",") if x.strip()]
    print("| Session | Config | Aligned words | Mean Δ start vs base, ms | Median, ms | p90 abs, ms | Flag |")
    print("|---|---|---|---|---|---|---|")
    for sid in sessions:
        bpath = RUNS / a.base / sid / "engine.words.json"
        if not bpath.is_file():
            print(f"| {sid} | {a.base} | missing | | | | |")
            continue
        base = norm_seq(raw_words(bpath))
        for c in configs:
            p = RUNS / c / sid / "engine.words.json"
            if not p.is_file():
                print(f"| {sid} | {c} | missing | | | | |")
                continue
            st = offset_stats(base, norm_seq(raw_words(p)))
            flag = "> 150 ms" if abs(st.get("mean_ms", 0)) > 150 else ""
            print(f"| {sid} | {c} | {st['pairs']} | {st.get('mean_ms')} | {st.get('median_ms')} | {st.get('p90_abs_ms')} | {flag} |")
    if "ami" in sessions:
        ref = ami_ref_words(a.sessions_file)
        print("\nAMI against the manual word times of the reference (positive = hypothesis later):\n")
        print("| Config | Aligned words | Mean Δ start, ms | Median, ms | p90 abs, ms |")
        print("|---|---|---|---|---|")
        for c in [a.base, *configs]:
            p = RUNS / c / "ami" / "engine.words.json"
            if p.is_file():
                st = offset_stats(ref, norm_seq(raw_words(p)))
                print(f"| {c} | {st['pairs']} | {st.get('mean_ms')} | {st.get('median_ms')} | {st.get('p90_abs_ms')} |")


def tag_agreement(words: list[dict], rttm) -> dict:
    import numpy as np
    from scipy.optimize import linear_sum_assignment

    rows = sorted(rttm, key=lambda r: r.start)
    pairs = []
    untagged = uncovered = 0
    for w in words:
        if w.get("speaker") is None:
            untagged += 1
            continue
        s, e = float(w["start"]), max(float(w["end"]), float(w["start"]) + 0.01)
        best, best_ov = None, 0.0
        for r in rows:
            if r.start > e:
                break
            ov = min(e, r.end) - max(s, r.start)
            if ov > best_ov:
                best, best_ov = r.speaker, ov
        if best is None:
            uncovered += 1
            continue
        pairs.append((w["speaker"], best))
    hs = sorted({h for h, _ in pairs}, key=str)
    ds = sorted({d for _, d in pairs})
    m = np.zeros((len(hs), len(ds)), dtype=np.int64)
    for h, d in pairs:
        m[hs.index(h), ds.index(d)] += 1
    r, c = linear_sum_assignment(-m) if m.size else ([], [])
    agree = int(sum(m[i, j] for i, j in zip(r, c)))
    return {
        "words": len(words),
        "compared": len(pairs),
        "untagged": untagged,
        "uncovered": uncovered,
        "agreement": agree / len(pairs) if pairs else float("nan"),
        "tag_speakers": len(hs),
        "diarizer_speakers": len(ds),
    }


def cmd_tags(a) -> None:
    print("| Config | Session | Words | Compared | Not covered by diarizer | Tag speakers | Diarizer speakers | Agreement % |")
    print("|---|---|---|---|---|---|---|---|")
    for c in [x.strip() for x in a.configs.split(",") if x.strip()]:
        d = RUNS / c / a.session
        if not (d / "engine.words.json").is_file() or not (d / "diarizer.rttm").is_file():
            print(f"| {c} | {a.session} | missing | | | | | |")
            continue
        st = tag_agreement(raw_words(d / "engine.words.json"), read_rttm(d / "diarizer.rttm"))
        print(
            f"| {c} | {a.session} | {st['words']} | {st['compared']} | {st['uncovered']} | {st['tag_speakers']} | "
            f"{st['diarizer_speakers']} | {100 * st['agreement']:.1f} |"
        )


def cmd_diff(a) -> None:
    from rapidfuzz.distance import Levenshtein

    print(f"| Session | Words {a.a} | Words {a.b} | Raw words differing (edit ops) | Identical |")
    print("|---|---|---|---|---|")
    for sid in [x.strip() for x in a.sessions.split(",") if x.strip()]:
        pa, pb = RUNS / a.a / sid / "engine.words.json", RUNS / a.b / sid / "engine.words.json"
        if not (pa.is_file() and pb.is_file()):
            print(f"| {sid} | missing | | | |")
            continue
        wa = [w["word"] for w in raw_words(pa)]
        wb = [w["word"] for w in raw_words(pb)]
        n = Levenshtein.distance(wa, wb)
        ta = [(w["word"], w["start"]) for w in raw_words(pa)]
        tb = [(w["word"], w["start"]) for w in raw_words(pb)]
        print(f"| {sid} | {len(wa)} | {len(wb)} | {n} | {'yes, words and times' if ta == tb else 'no'} |")


def cmd_ami_misses(a) -> None:
    """Reference words (normalised) in [offset, offset + dur] that are not
    matched by an equal hypothesis word."""
    ref = [w for w in ami_ref_words(a.sessions_file) if a.offset <= w[1] <= a.offset + a.dur]
    hyp = norm_seq(raw_words(Path(a.words)))
    hyp = [(t, s + a.offset, e + a.offset, sp) for t, s, e, sp in hyp]
    matched = {i for i, _ in equal_pairs(ref, hyp)}
    missed = Counter(ref[i][0] for i in range(len(ref)) if i not in matched)
    total = Counter(w[0] for w in ref)
    print(f"reference words in the clip: {len(ref)}, missed: {sum(missed.values())}")
    for w, n in missed.most_common(a.top):
        print(f"{w:<18} missed {n} of {total[w]}")


def cmd_clip_score(a) -> None:
    """WER (speaker-agnostic, time-ordered words) and cpWER of runtime words
    JSONs for an AMI clip cut at --offset, against the manual reference
    words inside the clip."""
    import metrics as M
    from rapidfuzz.distance import Levenshtein

    s = load_sessions(a.sessions_file)["ami"]
    rows = [r for r in read_seglst(s.ref_seglst) if a.offset <= float(r["start_time"]) <= a.offset + a.dur]
    ref_segs = [
        {"session_id": "clip", "speaker": r["speaker"], "start_time": float(r["start_time"]) - a.offset,
         "end_time": float(r["end_time"]) - a.offset, "words": r["words"]}
        for r in rows
    ]
    ref_tok = [t for r in sorted(ref_segs, key=lambda r: r["start_time"]) for t in normalize_token(r["words"])]
    print("| Words JSON | Words | WER % | cpWER % |")
    print("|---|---|---|---|")
    for p in a.words:
        ws = raw_words(Path(p))
        hyp_tok = [t for w in ws for t in normalize_token(w["word"])]
        wer = Levenshtein.distance(ref_tok, hyp_tok) / len(ref_tok)
        hyp_segs = [
            {"session_id": "clip", "speaker": str(w.get("speaker", 0)), "start_time": float(w["start"]),
             "end_time": max(float(w["end"]), float(w["start"])), "words": w["word"]}
            for w in ws
        ]
        from common import normalize_seglst

        cp = M.wer_metrics(normalize_seglst(ref_segs), normalize_seglst(hyp_segs), normalize_seglst(hyp_segs))["cpwer"]
        print(f"| {p} | {len(ws)} | {100 * wer:.1f} | {100 * cp:.1f} |")


def _load(path: Path) -> dict | None:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def cmd_table(a) -> None:
    """One row per config: AMI and Zoom quality on the short sessions, speed of
    the transcribe step over them, and on the long session (77 min) peak
    VRAM, tag agreement with the standalone diarizer and the word-start
    offset against the model's R = 1 baseline."""
    import tomllib

    def pc(x):
        return "–" if x is None else f"{100 * x:.1f}"

    configs = tomllib.loads((EVAL_DIR / "configs.toml").read_text(encoding="utf-8"))
    base_of = {"nemotron-3.5": a.base_n35, "nemotron-en": a.base_en}
    short = [x.strip() for x in a.sessions.split(",") if x.strip()]
    head = ["Config", "ASR", "Engine args"]
    for sid in short:
        head += [f"{sid} cpWER", f"{sid} WDER", f"{sid} near"] if sid != "ami" else ["ami cpWER", "ami WDER near"]
    head += ["Transcribe s (short set)", "x real time", f"{a.long} VRAM Δ MiB", f"{a.long} tag agreement %",
             f"{a.long} words", "Δ start vs R=1, median ms (short set)"]
    print("| " + " | ".join(head) + " |")
    print("|" + "---|" * len(head))
    for c in [x.strip() for x in a.configs.split(",") if x.strip()]:
        cfg = configs.get(c, {})
        cells = [c, cfg.get("asr", "?"), " ".join(cfg.get("engine_args") or []) or "–"]
        asr_s = audio_s = 0.0
        offs = []
        for sid in short:
            m = _load(RUNS / c / sid / "metrics.json") or {}
            r = _load(RUNS / c / sid / "run.json") or {}
            if sid == "ami":
                cells += [pc(m.get("cpwer")), pc(m.get("wder_near"))]
            else:
                cells += [pc(m.get("cpwer")), pc(m.get("wder")), pc(m.get("wder_near"))]
            if r.get("asr_wall_s"):
                asr_s += r["asr_wall_s"]
                audio_s += r.get("audio_s") or 0
            base = base_of.get(cfg.get("asr"))
            bp, cp = RUNS / str(base) / sid / "engine.words.json", RUNS / c / sid / "engine.words.json"
            if base and base != c and bp.is_file() and cp.is_file():
                b, o = norm_seq(raw_words(bp)), norm_seq(raw_words(cp))
                offs += [o[j][1] - b[i][1] for i, j in equal_pairs(b, o)]
        cells += [f"{asr_s:.0f}" if asr_s else "–", f"{audio_s / asr_s:.0f}" if asr_s else "–"]
        rl = _load(RUNS / c / a.long / "run.json") or {}
        cells.append(str(rl.get("vram_asr_peak_delta_mib") or "–"))
        d = RUNS / c / a.long
        if (d / "engine.words.json").is_file() and (d / "diarizer.rttm").is_file():
            st = tag_agreement(raw_words(d / "engine.words.json"), read_rttm(d / "diarizer.rttm"))
            cells += [f"{100 * st['agreement']:.1f}", str(st["words"])]
        else:
            cells += ["–", "–"]
        cells.append(f"{round(1000 * statistics.median(offs))}" if offs else ("0 (base)" if c in base_of.values() else "–"))
        print("| " + " | ".join(cells) + " |")


def cmd_boost(a) -> None:
    """Per nemo_direct run directory: phrase hits (normalised), words changed
    against the base run, clip WER, and what the runtime said about boosting
    on stderr."""
    from rapidfuzz.distance import Levenshtein

    phrases = [tuple(normalize_token(t)[0] for t in ph.split() if normalize_token(t)) for ph in a.phrases]

    def hits(tokens: list[str], ph: tuple) -> int:
        n = len(ph)
        return sum(1 for i in range(len(tokens) - n + 1) if tuple(tokens[i:i + n]) == ph)

    s = load_sessions(a.sessions_file)["ami"]
    ref_rows = [r for r in read_seglst(s.ref_seglst) if a.offset <= float(r["start_time"]) <= a.offset + a.dur]
    ref_tok = [t for r in sorted(ref_rows, key=lambda r: float(r["start_time"])) for t in normalize_token(r["words"])]
    base = [t for w in raw_words(Path(a.base) / "words.json") for t in normalize_token(w["word"])]
    head = ["Run", "Normalised words"] + [f"'{' '.join(ph)}' hits (ref {hits(ref_tok, ph)})" for ph in phrases]
    head += ["Edit ops vs base", "Clip WER %", "Runtime message"]
    print("| " + " | ".join(head) + " |")
    print("|" + "---|" * len(head))
    for d in [Path(a.base), *map(Path, a.runs)]:
        if not (d / "words.json").is_file():
            print(f"| {d.name} | missing |")
            continue
        toks = [t for w in raw_words(d / "words.json") for t in normalize_token(w["word"])]
        err = (d / "stderr.txt").read_text(encoding="utf-8", errors="replace") if (d / "stderr.txt").is_file() else ""
        msgs = [ln.strip() for ln in err.splitlines()
                if (" -> " not in ln) and ("boost" in ln.lower() or "sentencepiece" in ln.lower())]
        cells = [d.name, str(len(toks))] + [str(hits(toks, ph)) for ph in phrases]
        cells += [str(Levenshtein.distance(base, toks)), f"{100 * Levenshtein.distance(ref_tok, toks) / len(ref_tok):.1f}",
                  ("; ".join(dict.fromkeys(msgs)) or "none").replace("|", "/")]
        print("| " + " | ".join(cells) + " |")


def cmd_punct(a) -> None:
    for p in a.words:
        ws = raw_words(Path(p))
        punct = sum(1 for w in ws if w["word"] and not w["word"][-1].isalnum())
        caps = sum(1 for w in ws if w["word"][:1].isupper())
        print(f"{p}: {len(ws)} words, {punct} end in punctuation ({100 * punct / max(len(ws), 1):.1f} %), "
              f"{caps} capitalised ({100 * caps / max(len(ws), 1):.1f} %)")


def main() -> None:
    sys.stdout.reconfigure(encoding="utf-8")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--sessions-file", type=Path, default=EVAL_DIR / "sessions.toml")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("offsets")
    p.add_argument("--base", required=True)
    p.add_argument("--configs", required=True)
    p.add_argument("--sessions", default="ami,aug2,sep5")
    p = sub.add_parser("tags")
    p.add_argument("--configs", required=True)
    p.add_argument("--session", default="aug14")
    p = sub.add_parser("diff")
    p.add_argument("--a", required=True)
    p.add_argument("--b", required=True)
    p.add_argument("--sessions", default="ami,aug2,sep5")
    p = sub.add_parser("ami-misses")
    p.add_argument("--words", required=True)
    p.add_argument("--offset", type=float, default=0.0, help="clip start in the full AMI recording (s)")
    p.add_argument("--dur", type=float, default=1e9)
    p.add_argument("--top", type=int, default=40)
    p = sub.add_parser("clip-score")
    p.add_argument("--words", nargs="+", required=True)
    p.add_argument("--offset", type=float, default=50.0)
    p.add_argument("--dur", type=float, default=300.0)
    p = sub.add_parser("boost")
    p.add_argument("--base", required=True, help="nemo_direct run directory without boosting")
    p.add_argument("--runs", nargs="+", required=True)
    p.add_argument("--phrases", nargs="+", required=True)
    p.add_argument("--offset", type=float, default=50.0)
    p.add_argument("--dur", type=float, default=300.0)
    p = sub.add_parser("table")
    p.add_argument("--configs", required=True)
    p.add_argument("--sessions", default="ami,aug2,sep5")
    p.add_argument("--long", default="aug14")
    p.add_argument("--base-n35", default="nemotron35")
    p.add_argument("--base-en", default="nemotron_en")
    p = sub.add_parser("punct")
    p.add_argument("--words", nargs="+", required=True)
    a = ap.parse_args()
    {"offsets": cmd_offsets, "tags": cmd_tags, "diff": cmd_diff, "ami-misses": cmd_ami_misses, "clip-score": cmd_clip_score, "table": cmd_table, "boost": cmd_boost,
     "punct": cmd_punct}[a.cmd](a)


if __name__ == "__main__":
    main()
