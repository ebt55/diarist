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
  uv run python -I levers.py loops --configs shipped_en --sessions all
  uv run python -I levers.py loops --db <copy of diarist.db> --meetings 12-21 [--analysis <loops json>]
  uv run python -I levers.py loops --configs pn_shipped_n35 --sessions present --runs-dir <.work/runs> --fleurs-root <.work/fleurs>

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
compare: session by config table of speaker count, DER, WDER, near and cpWER.
fleurs-utts: per-utterance errors of FLEURS runs, with empty and silent
  recordings counted.
boost: phrase hits, changes against the unboosted run, clip WER and the
  runtime's boosting messages, per nemo_direct run.
table: the lookahead summary, one row per config (quality on the short
  sessions, speed, and on the 77-minute session VRAM, tag agreement and the
  word-start offset against the same model's R = 1 config).
loops: repeated-token runs and what the loop collapse of
  crates/core/src/transcript/loops.rs (docs/plans/16 section 12.3) removes,
  over the cached engine words JSON of eval runs (as the file import builds
  its words) or over the current runs of a copy of a data root's database
  (each channel's words in time order, as the post-pass and a rebuild see
  them). Runs by length, median start gap and split; the loops by the
  plan's rule (a loop goes whole) and the bursts the rule as built
  collapses (only the frame-level bursts of a loop go); on sessions with a
  reference (the sessions file's, --extra-sessions TOMLs with ref_seglst,
  FLEURS refs.jsonl), how many copies of the token the reference has around
  each; with --analysis, the share of removed tokens that a word-error
  analysis's loop records (Whisper lacks the repeat, Nemotron 3.5 has no
  loop there) confirm. Numbers only, per session label or meeting number;
  no text is printed. The tokens are the Rust norm's (loop_norm).
"""

from __future__ import annotations

import argparse
import bisect
import json
import os
import statistics
import sys
import unicodedata
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

    print(f"| Session | Words {a.a} | Words {a.b} | Raw words differing (edit ops) | Identical | Speaker tags identical |")
    print("|---|---|---|---|---|---|")
    for sid in [x.strip() for x in a.sessions.split(",") if x.strip()]:
        pa, pb = RUNS / a.a / sid / "engine.words.json", RUNS / a.b / sid / "engine.words.json"
        if not (pa.is_file() and pb.is_file()):
            print(f"| {sid} | missing | | | | |")
            continue
        wa = [w["word"] for w in raw_words(pa)]
        wb = [w["word"] for w in raw_words(pb)]
        n = Levenshtein.distance(wa, wb)
        ta = [(w["word"], w["start"]) for w in raw_words(pa)]
        tb = [(w["word"], w["start"]) for w in raw_words(pb)]
        ka = [(w["word"], w["start"], w.get("speaker")) for w in raw_words(pa)]
        kb = [(w["word"], w["start"], w.get("speaker")) for w in raw_words(pb)]
        tags = "yes" if ka == kb else f"no ({sum(x != y for x, y in zip(ka, kb))} of {min(len(ka), len(kb))} differ)"
        print(f"| {sid} | {len(wa)} | {len(wb)} | {n} | {'yes, words and times' if ta == tb else 'no'} | {tags} |")


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


def cmd_fleurs_utts(a) -> None:
    """Per FLEURS utterance of cached runs: hypothesis words inside the
    utterance's slot (start - 0.4 s to the next start - 0.4 s), error rate,
    and the recording's peak level, listing the utterances above --bad."""
    import wave

    import numpy as np
    from rapidfuzz.distance import Levenshtein

    import run as R

    for lang in [x.strip() for x in a.langs.split(",") if x.strip()]:
        root = WORK / "fleurs" / lang
        refs = [json.loads(ln) for ln in (root / "refs.jsonl").read_text(encoding="utf-8").splitlines() if ln.strip()]
        with wave.open(str(root / "audio.wav")) as wv:
            audio = np.frombuffer(wv.readframes(wv.getnframes()), dtype=np.int16).astype(np.float32) / 32768
        for c in [x.strip() for x in a.configs.split(",") if x.strip()]:
            d = RUNS / c / f"fleurs-{lang}-{a.mode}"
            if not (d / "engine.words.json").is_file():
                print(f"{lang} {c} {a.mode}: missing")
                continue
            ws = raw_words(d / "engine.words.json")
            bad, empty, silent = [], 0, 0
            for i, r in enumerate(refs):
                lo = r["start"] - 0.4
                hi = refs[i + 1]["start"] - 0.4 if i + 1 < len(refs) else 1e9
                hyp = " ".join(w["word"] for w in ws if lo <= float(w["start"]) < hi)
                rt, ht = R.fleurs_units(lang, r["raw_transcription"]), R.fleurs_units(lang, hyp)
                er = Levenshtein.distance(rt, ht) / max(len(rt), 1)
                seg = audio[int(r["start"] * 16000):int(r["end"] * 16000)]
                peak = 20 * np.log10(float(np.abs(seg).max()) + 1e-9) if seg.size else -120.0
                silent += peak < -30
                empty += not ht
                if er >= a.bad:
                    bad.append(f"#{i} at {r['start']:.0f} s: {len(ht)}/{len(rt)} units, error {100 * er:.0f} %, peak {peak:.0f} dBFS")
            print(f"{lang} {c} {a.mode}: {len(refs)} utterances, {empty} with no words, {silent} with peak below -30 dBFS")
            for b in bad:
                print("    " + b)


def cmd_compare(a) -> None:
    """Session by config table from cached metrics.json: speaker count
    error, DER c0.25, WDER, WDER near and cpWER (percent)."""
    sessions = load_sessions(a.sessions_file)
    configs = [x.strip() for x in a.configs.split(",") if x.strip()]
    sids = list(sessions) if a.sessions == "all" else [x.strip() for x in a.sessions.split(",") if x.strip()]

    def pc(x):
        return "–" if x is None or x != x else f"{100 * x:.1f}"

    print("| Session | Config | Ref / hyp speakers | DER c0.25 | WDER | WDER near | cpWER |")
    print("|---|---|---|---|---|---|---|")
    for sid in sids:
        for c in configs:
            m = _load(RUNS / c / sid / "metrics.json")
            if not m:
                print(f"| {sessions[sid].label} | {c} | missing | | | | |")
                continue
            print(
                f"| {sessions[sid].label} | {c} | {m['ref_speakers']} / {m['hyp_speakers']} | {pc(m.get('der_c025'))} | "
                f"{pc(m.get('wder'))} | {pc(m.get('wder_near'))} | {pc(m.get('cpwer'))} |"
            )


def cmd_punct(a) -> None:
    for p in a.words:
        ws = raw_words(Path(p))
        punct = sum(1 for w in ws if w["word"] and not w["word"][-1].isalnum())
        caps = sum(1 for w in ws if w["word"][:1].isupper())
        print(f"{p}: {len(ws)} words, {punct} end in punctuation ({100 * punct / max(len(ws), 1):.1f} %), "
              f"{caps} capitalised ({100 * caps / max(len(ws), 1):.1f} %)")


# --------------------------------------------------------------------------
# loops: the repeated-token loop collapse (docs/plans/16 section 12.3), the
# same rule as crates/core/src/transcript/loops.rs.

LOOP_MIN_RUN = 4  # loops.rs LOOP_MIN_RUN
LOOP_FRAME_MS = 80  # loops.rs LOOP_FRAME_MS: one encoder frame
LOOP_SPLIT_MS = 400  # loops.rs LOOP_SPLIT_MS

# Rust's `char` predicates where Python's unicodedata differs, so that
# loop_norm is the Rust norm character for character (str.isalnum drops the
# Other_Alphabetic combining vowel signs of Devanagari, Thai and other
# Indic and Southeast Asian scripts, which char::is_alphabetic keeps).
# Generated from Rust's std tables (char::is_alphabetic, is_numeric,
# is_whitespace over every scalar value) against Python 3.12's unicodedata:
# inclusive code point ranges. RUST_UNICODE is the std tables' version.
RUST_UNICODE = "17.0.0"  # Python 3.12: 15.0.0
# Alphabetic in Rust, not a letter or Nl in Python: Other_Alphabetic, and
# letters newer than Python's tables.
_RS_ALPHA_EXTRA = (
    (0x0345, 0x0345), (0x0363, 0x036F), (0x05B0, 0x05BD), (0x05BF, 0x05BF), (0x05C1, 0x05C2), (0x05C4, 0x05C5),
    (0x05C7, 0x05C7), (0x0610, 0x061A), (0x064B, 0x0657), (0x0659, 0x065F), (0x0670, 0x0670), (0x06D6, 0x06DC),
    (0x06E1, 0x06E4), (0x06E7, 0x06E8), (0x06ED, 0x06ED), (0x0711, 0x0711), (0x0730, 0x073F), (0x07A6, 0x07B0),
    (0x0816, 0x0817), (0x081B, 0x0823), (0x0825, 0x0827), (0x0829, 0x082C), (0x088F, 0x088F), (0x0897, 0x0897),
    (0x08D4, 0x08DF), (0x08E3, 0x08E9), (0x08F0, 0x0903), (0x093A, 0x093B), (0x093E, 0x094C), (0x094E, 0x094F),
    (0x0955, 0x0957), (0x0962, 0x0963), (0x0981, 0x0983), (0x09BE, 0x09C4), (0x09C7, 0x09C8), (0x09CB, 0x09CC),
    (0x09D7, 0x09D7), (0x09E2, 0x09E3), (0x0A01, 0x0A03), (0x0A3E, 0x0A42), (0x0A47, 0x0A48), (0x0A4B, 0x0A4C),
    (0x0A51, 0x0A51), (0x0A70, 0x0A71), (0x0A75, 0x0A75), (0x0A81, 0x0A83), (0x0ABE, 0x0AC5), (0x0AC7, 0x0AC9),
    (0x0ACB, 0x0ACC), (0x0AE2, 0x0AE3), (0x0AFA, 0x0AFC), (0x0B01, 0x0B03), (0x0B3E, 0x0B44), (0x0B47, 0x0B48),
    (0x0B4B, 0x0B4C), (0x0B56, 0x0B57), (0x0B62, 0x0B63), (0x0B82, 0x0B82), (0x0BBE, 0x0BC2), (0x0BC6, 0x0BC8),
    (0x0BCA, 0x0BCC), (0x0BD7, 0x0BD7), (0x0C00, 0x0C04), (0x0C3E, 0x0C44), (0x0C46, 0x0C48), (0x0C4A, 0x0C4C),
    (0x0C55, 0x0C56), (0x0C5C, 0x0C5C), (0x0C62, 0x0C63), (0x0C81, 0x0C83), (0x0CBE, 0x0CC4), (0x0CC6, 0x0CC8),
    (0x0CCA, 0x0CCC), (0x0CD5, 0x0CD6), (0x0CDC, 0x0CDC), (0x0CE2, 0x0CE3), (0x0CF3, 0x0CF3), (0x0D00, 0x0D03),
    (0x0D3E, 0x0D44), (0x0D46, 0x0D48), (0x0D4A, 0x0D4C), (0x0D57, 0x0D57), (0x0D62, 0x0D63), (0x0D81, 0x0D83),
    (0x0DCF, 0x0DD4), (0x0DD6, 0x0DD6), (0x0DD8, 0x0DDF), (0x0DF2, 0x0DF3), (0x0E31, 0x0E31), (0x0E34, 0x0E3A),
    (0x0E4D, 0x0E4D), (0x0EB1, 0x0EB1), (0x0EB4, 0x0EB9), (0x0EBB, 0x0EBC), (0x0ECD, 0x0ECD), (0x0F71, 0x0F83),
    (0x0F8D, 0x0F97), (0x0F99, 0x0FBC), (0x102B, 0x1036), (0x1038, 0x1038), (0x103B, 0x103E), (0x1056, 0x1059),
    (0x105E, 0x1060), (0x1062, 0x1064), (0x1067, 0x106D), (0x1071, 0x1074), (0x1082, 0x108D), (0x108F, 0x108F),
    (0x109A, 0x109D), (0x1712, 0x1713), (0x1732, 0x1733), (0x1752, 0x1753), (0x1772, 0x1773), (0x17B6, 0x17C8),
    (0x1885, 0x1886), (0x18A9, 0x18A9), (0x1920, 0x192B), (0x1930, 0x1938), (0x1A17, 0x1A1B), (0x1A55, 0x1A5E),
    (0x1A61, 0x1A74), (0x1ABF, 0x1AC0), (0x1ACC, 0x1ACE), (0x1B00, 0x1B04), (0x1B35, 0x1B43), (0x1B80, 0x1B82),
    (0x1BA1, 0x1BA9), (0x1BAC, 0x1BAD), (0x1BE7, 0x1BF1), (0x1C24, 0x1C36), (0x1C89, 0x1C8A), (0x1DD3, 0x1DF4),
    (0x24B6, 0x24E9), (0x2DE0, 0x2DFF), (0xA674, 0xA67B), (0xA69E, 0xA69F), (0xA7CB, 0xA7CF), (0xA7D2, 0xA7D2),
    (0xA7D4, 0xA7D4), (0xA7DA, 0xA7DC), (0xA7F1, 0xA7F1), (0xA802, 0xA802), (0xA80B, 0xA80B), (0xA823, 0xA827),
    (0xA880, 0xA881), (0xA8B4, 0xA8C3), (0xA8C5, 0xA8C5), (0xA8FF, 0xA8FF), (0xA926, 0xA92A), (0xA947, 0xA952),
    (0xA980, 0xA983), (0xA9B4, 0xA9BF), (0xA9E5, 0xA9E5), (0xAA29, 0xAA36), (0xAA43, 0xAA43), (0xAA4C, 0xAA4D),
    (0xAA7B, 0xAA7D), (0xAAB0, 0xAAB0), (0xAAB2, 0xAAB4), (0xAAB7, 0xAAB8), (0xAABE, 0xAABE), (0xAAEB, 0xAAEF),
    (0xAAF5, 0xAAF5), (0xABE3, 0xABEA), (0xFB1E, 0xFB1E), (0x10376, 0x1037A), (0x105C0, 0x105F3), (0x10940, 0x10959),
    (0x10A01, 0x10A03), (0x10A05, 0x10A06), (0x10A0C, 0x10A0F), (0x10D24, 0x10D27), (0x10D4A, 0x10D65),
    (0x10D69, 0x10D69), (0x10D6F, 0x10D85), (0x10EAB, 0x10EAC), (0x10EC2, 0x10EC7), (0x10EFA, 0x10EFC),
    (0x11000, 0x11002), (0x11038, 0x11045), (0x11073, 0x11074), (0x11080, 0x11082), (0x110B0, 0x110B8),
    (0x110C2, 0x110C2), (0x11100, 0x11102), (0x11127, 0x11132), (0x11145, 0x11146), (0x11180, 0x11182),
    (0x111B3, 0x111BF), (0x111CE, 0x111CF), (0x1122C, 0x11234), (0x11237, 0x11237), (0x1123E, 0x1123E),
    (0x11241, 0x11241), (0x112DF, 0x112E8), (0x11300, 0x11303), (0x1133E, 0x11344), (0x11347, 0x11348),
    (0x1134B, 0x1134C), (0x11357, 0x11357), (0x11362, 0x11363), (0x11380, 0x11389), (0x1138B, 0x1138B),
    (0x1138E, 0x1138E), (0x11390, 0x113B5), (0x113B7, 0x113C0), (0x113C2, 0x113C2), (0x113C5, 0x113C5),
    (0x113C7, 0x113CA), (0x113CC, 0x113CD), (0x113D1, 0x113D1), (0x113D3, 0x113D3), (0x11435, 0x11441),
    (0x11443, 0x11445), (0x114B0, 0x114C1), (0x115AF, 0x115B5), (0x115B8, 0x115BE), (0x115DC, 0x115DD),
    (0x11630, 0x1163E), (0x11640, 0x11640), (0x116AB, 0x116B5), (0x1171D, 0x1172A), (0x1182C, 0x11838),
    (0x11930, 0x11935), (0x11937, 0x11938), (0x1193B, 0x1193C), (0x11940, 0x11940), (0x11942, 0x11942),
    (0x119D1, 0x119D7), (0x119DA, 0x119DF), (0x119E4, 0x119E4), (0x11A01, 0x11A0A), (0x11A35, 0x11A39),
    (0x11A3B, 0x11A3E), (0x11A51, 0x11A5B), (0x11A8A, 0x11A97), (0x11B60, 0x11B67), (0x11BC0, 0x11BE0),
    (0x11C2F, 0x11C36), (0x11C38, 0x11C3E), (0x11C92, 0x11CA7), (0x11CA9, 0x11CB6), (0x11D31, 0x11D36),
    (0x11D3A, 0x11D3A), (0x11D3C, 0x11D3D), (0x11D3F, 0x11D41), (0x11D43, 0x11D43), (0x11D47, 0x11D47),
    (0x11D8A, 0x11D8E), (0x11D90, 0x11D91), (0x11D93, 0x11D96), (0x11DB0, 0x11DDB), (0x11EF3, 0x11EF6),
    (0x11F00, 0x11F01), (0x11F03, 0x11F03), (0x11F34, 0x11F3A), (0x11F3E, 0x11F40), (0x13460, 0x143FA),
    (0x16100, 0x1612E), (0x16D40, 0x16D6C), (0x16EA0, 0x16EB8), (0x16EBB, 0x16ED3), (0x16F4F, 0x16F4F),
    (0x16F51, 0x16F87), (0x16F8F, 0x16F92), (0x16FF0, 0x16FF6), (0x187F8, 0x187FF), (0x18CFF, 0x18CFF),
    (0x18D09, 0x18D1E), (0x18D80, 0x18DF2), (0x1BC9E, 0x1BC9E), (0x1E000, 0x1E006), (0x1E008, 0x1E018),
    (0x1E01B, 0x1E021), (0x1E023, 0x1E024), (0x1E026, 0x1E02A), (0x1E08F, 0x1E08F), (0x1E5D0, 0x1E5ED),
    (0x1E5F0, 0x1E5F0), (0x1E6C0, 0x1E6DE), (0x1E6E0, 0x1E6F5), (0x1E6FE, 0x1E6FF), (0x1E947, 0x1E947),
    (0x1F130, 0x1F149), (0x1F150, 0x1F169), (0x1F170, 0x1F189), (0x2B73A, 0x2B73F), (0x2CEA2, 0x2CEAD),
    (0x2EBF0, 0x2EE5D), (0x323B0, 0x33479),
)
_RS_ALPHA_MISSING = ()
# Numeric (Nd, Nl, No) in Rust, unassigned in Python's tables.
_RS_NUM_EXTRA = (
    (0x10D40, 0x10D49), (0x116D0, 0x116E3), (0x11BF0, 0x11BF9), (0x11DE0, 0x11DE9), (0x16130, 0x16139),
    (0x16D70, 0x16D79), (0x16FF4, 0x16FF6), (0x1CCF0, 0x1CCF9), (0x1E5F1, 0x1E5FA),
)
_RS_NUM_MISSING = ()
# White_Space (str.isspace also takes U+001C to U+001F).
_RS_WHITESPACE = (
    (0x0009, 0x000D), (0x0020, 0x0020), (0x0085, 0x0085), (0x00A0, 0x00A0), (0x1680, 0x1680), (0x2000, 0x200A),
    (0x2028, 0x2029), (0x202F, 0x202F), (0x205F, 0x205F), (0x3000, 0x3000),
)


def _in(ranges: tuple, o: int) -> bool:
    k = bisect.bisect_right(ranges, (o, 0x10FFFF)) - 1
    return k >= 0 and ranges[k][0] <= o <= ranges[k][1]


def rs_alphabetic(c: str) -> bool:
    """Rust char::is_alphabetic (the Unicode Alphabetic property)."""
    o = ord(c)
    if _in(_RS_ALPHA_EXTRA, o):
        return True
    if _in(_RS_ALPHA_MISSING, o):
        return False
    return unicodedata.category(c) in ("Lu", "Ll", "Lt", "Lm", "Lo", "Nl")


def rs_numeric(c: str) -> bool:
    """Rust char::is_numeric (general category Nd, Nl or No)."""
    o = ord(c)
    if _in(_RS_NUM_EXTRA, o):
        return True
    if _in(_RS_NUM_MISSING, o):
        return False
    return unicodedata.category(c) in ("Nd", "Nl", "No")


def rs_whitespace(c: str) -> bool:
    """Rust char::is_whitespace (the White_Space property)."""
    return _in(_RS_WHITESPACE, ord(c))


_RS_WHITESPACE_CHARS = "".join(chr(o) for a, b in _RS_WHITESPACE for o in range(a, b + 1))


def loop_norm(text: str) -> str:
    """crates/core/src/audio/echo/text.rs `norm`: lower case, then the
    characters Rust takes for letters (is_alphabetic), digits (is_numeric)
    and spaces (is_whitespace) kept, trimmed of Rust's white space."""
    return "".join(c for c in text.lower() if rs_alphabetic(c) or rs_numeric(c) or rs_whitespace(c)).strip(
        _RS_WHITESPACE_CHARS)


def _continuous(c: str) -> bool:
    """Han, Hiragana, Katakana or Thai (`punct::Script::of_char` and
    `is_continuous`: only letters have a script)."""
    o = ord(c)
    return rs_alphabetic(c) and (
        0x0E00 <= o <= 0x0E7F or 0x3040 <= o <= 0x30FF or 0x31F0 <= o <= 0x31FF or 0xFF66 <= o <= 0xFF9F
        or o in (0x3005, 0x3007) or 0x3400 <= o <= 0x4DBF or 0x4E00 <= o <= 0x9FFF or 0xF900 <= o <= 0xFAFF
        or 0x20000 <= o <= 0x3134F
    )


def loop_eligible(n: str) -> bool:
    """A token the collapse may touch: a non-empty norm that is not digits
    only and has no Han, Kana or Thai character."""
    return bool(n) and not all(rs_numeric(c) or rs_whitespace(c) for c in n) and not any(_continuous(c) for c in n)


def loop_runs(words: list[dict], key=lambda w: (w["norm"], w["speaker"])) -> list[list[tuple[int, int]]]:
    """Maximal runs of consecutive words (in time order) with the same key
    (the norm and the speaker label), each split into pieces wherever two
    consecutive starts are more than LOOP_SPLIT_MS apart: per run its
    pieces as inclusive (first, last) indices."""
    out = []
    i = 0
    while i < len(words):
        j = i
        while j + 1 < len(words) and key(words[j + 1]) == key(words[i]):
            j += 1
        pieces, a = [], i
        for k in range(i + 1, j + 1):
            if words[k]["start"] - words[k - 1]["start"] > LOOP_SPLIT_MS:
                pieces.append((a, k - 1))
                a = k
        pieces.append((a, j))
        out.append(pieces)
        i = j + 1
    return out


def piece_gaps(words: list[dict], piece: tuple[int, int]) -> list[int]:
    i, j = piece
    return [words[k + 1]["start"] - words[k]["start"] for k in range(i, j)]


def loop_kind(words: list[dict], piece: tuple[int, int]) -> str | None:
    """The plan's rule (docs/plans/16 section 12.3), which decides whether a
    piece is a loop: `loop`, LOOP_MIN_RUN or more words at a median start
    gap of at most one frame; `frame3`, exactly three words starting in one
    frame (within half a frame of the first, i.e. equal on the 80 ms grid);
    else None (the piece is kept)."""
    i, j = piece
    n = j - i + 1
    if not loop_eligible(words[i]["norm"]):
        return None
    if n >= LOOP_MIN_RUN:
        return "loop" if statistics.median(piece_gaps(words, piece)) <= LOOP_FRAME_MS else None
    if n == 3 and words[j]["start"] - words[i]["start"] < LOOP_FRAME_MS / 2:
        return "frame3"
    return None


def loop_bursts(words: list[dict], piece: tuple[int, int]) -> list[tuple[int, int, str]]:
    """What the rule as built (loops.rs) removes from a piece: nothing
    unless the plan's rule takes it for a loop; then its frame-level
    sub-runs (consecutive starts at most LOOP_FRAME_MS apart) that meet the
    rule on their own, LOOP_MIN_RUN or more words (`loop`) or exactly three
    in one frame (`frame3`), as (first, last, kind). Never more than the
    plan's rule removes."""
    if loop_kind(words, piece) is None:
        return []
    i, j = piece
    out, s = [], i
    for t in range(i + 1, j + 2):
        if t > j or words[t]["start"] - words[t - 1]["start"] > LOOP_FRAME_MS:
            n = t - s
            if n >= LOOP_MIN_RUN:
                out.append((s, t - 1, "loop"))
            elif n == 3 and words[t - 1]["start"] - words[s]["start"] < LOOP_FRAME_MS / 2:
                out.append((s, t - 1, "frame3"))
            s = t
    return out


def loop_removed(words: list[dict], key=lambda w: (w["norm"], w["speaker"]), split: bool = True,
                 triple_median: bool = False, rule: str = "built") -> int:
    """Words the rule removes from `words`: `rule="built"`, the rule as
    built (loop_bursts); `rule="plan"`, the plan's rule (a loop goes whole);
    with `split=False` and `triple_median=True`, the draft rule of plan 16
    section 12.3 before its review (no 400 ms split; three words at a
    median gap of one frame; a loop goes whole)."""
    removed = 0
    for pieces in loop_runs(words, key):
        if not split:
            pieces = [(pieces[0][0], pieces[-1][1])]
        for (i, j) in pieces:
            n = j - i + 1
            if triple_median and n == 3:
                hit = loop_eligible(words[i]["norm"]) and statistics.median(piece_gaps(words, (i, j))) <= LOOP_FRAME_MS
                removed += (n - 1) if hit else 0
            elif rule == "built" and split and not triple_median:
                removed += sum(b - a for (a, b, _) in loop_bursts(words, (i, j)))
            else:
                removed += (n - 1) if loop_kind(words, (i, j)) is not None else 0
    return removed


def engine_stream(path: Path) -> list[dict]:
    """A runtime words JSON as the file import stores it: microseconds
    rounded, then milliseconds rounded (words_from_engine), clamped to the
    audio, speakers numbered by first appearance (spk_N, else spk_unknown),
    sorted by start (stable)."""
    doc = json.loads(path.read_text(encoding="utf-8"))
    labels: dict = {}
    out = []
    for w in doc.get("words", []):
        s_us = round(max(float(w["start"]), 0.0) * 1e6)
        e_us = round(max(float(w["end"]), float(w["start"]), 0.0) * 1e6)
        spk = w.get("speaker")
        label = "spk_unknown" if spk is None else f"spk_{labels.setdefault(spk, len(labels))}"
        out.append({"start": (s_us + 500) // 1000, "end": (e_us + 500) // 1000, "text": str(w["word"]),
                    "speaker": label, "seg": None})
    dur = max(int(round(float(doc.get("duration") or 0) * 1000)), max((w["end"] for w in out), default=0))
    for w in out:
        w["start"] = min(w["start"], dur)
        w["end"] = min(max(w["end"], w["start"]), dur)
        w["norm"] = loop_norm(w["text"])
    out.sort(key=lambda w: w["start"])
    return out


def db_streams(con, run_id: int) -> dict[str, list[dict]]:
    """The words of a stored run by channel, in (start, id) order, each with
    the engine label of its own segment's speaker row (as a rebuild reads
    them: punct::rebuild::source_run) and its segment."""
    cols = {r[1] for r in con.execute("PRAGMA table_info(words)")}
    asr = "coalesce(w.raw_text, w.text)" if "raw_text" in cols else "w.text"
    q = f"""select w.start_ms, w.end_ms, {asr}, sp.channel, sp.engine_label, s.id
            from words w join segments s on s.id = w.segment_id join speakers sp on sp.id = s.speaker_id
            where s.run_id = ? order by w.start_ms, w.id"""
    out: dict[str, list[dict]] = {}
    for s, e, t, ch, lab, seg in con.execute(q, (run_id,)):
        out.setdefault(ch, []).append({"start": s, "end": e, "text": t, "norm": loop_norm(t), "speaker": lab, "seg": seg})
    return out


def _q(v: list[float], p: float) -> str:
    if not v:
        return "–"
    v = sorted(v)
    return f"{v[min(len(v) - 1, int(p * (len(v) - 1)))]:.0f}"


class LoopTally:
    """Counters over one or more channel streams."""

    def __init__(self) -> None:
        self.c: Counter = Counter()
        self.gaps: dict[str, list[float]] = {"3": [], "4-5": [], "6+": []}

    def merge(self, other: "LoopTally") -> None:
        self.c.update(other.c)
        for k, v in other.gaps.items():
            self.gaps[k] += v

    def add(self, words: list[dict], refs=None, margin: float = 1.0, ref_norm=None) -> tuple[list[dict], list[dict]]:
        """Counts `words` (one channel) and returns the bursts the rule as
        built collapses and the pieces the plan's rule collapses whole, each
        as {start, end, n, kind, norm, i, j}. With `refs` (reference words
        with times and their starts), the reference's copies of the token
        around each collapsed burst, each kept 4+ piece and each loop whose
        human-pace words the rule as built keeps are counted too; tokens are
        compared through `ref_norm` (default: the harness's normalize_token
        of the hypothesis word against the normalised reference word)."""
        c = self.c
        c["words"] += len(words)
        bursts, plan = [], []
        for pieces in loop_runs(words):
            if len(pieces) > 1:
                c["runs_split"] += 1
                c["runs_split_collapsed"] += any(loop_kind(words, p) for p in pieces)
            for (i, j) in pieces:
                n = j - i + 1
                if n < 2:
                    continue
                g = piece_gaps(words, (i, j))
                med = statistics.median(g)
                kind = loop_kind(words, (i, j))
                if n == 2:
                    c["pairs"] += 1
                    c["pairs_one_frame"] += g[0] <= LOOP_FRAME_MS
                elif n == 3:
                    self.gaps["3"].append(med)
                    same = words[j]["start"] - words[i]["start"] < LOOP_FRAME_MS / 2
                    c["triples"] += 1
                    c["triples_same_frame"] += same
                    c["triples_one_frame_spacing"] += (not same) and max(g) <= LOOP_FRAME_MS
                else:
                    self.gaps["4-5" if n <= 5 else "6+"].append(med)
                    c["runs4"] += 1
                    c["runs4_fast"] += med <= LOOP_FRAME_MS
                    c["runs4_81_160"] += LOOP_FRAME_MS < med <= 2 * LOOP_FRAME_MS
                    c["runs4_slow"] += med > 2 * LOOP_FRAME_MS
                    c["runs4_jitter"] += LOOP_FRAME_MS < med < 1.5 * LOOP_FRAME_MS
                if n >= 4 and kind is None:
                    c["kept4_pieces"] += 1
                    c["kept4_words"] += n
                    if not loop_eligible(words[i]["norm"]):
                        c["kept4_ineligible"] += 1
                built = loop_bursts(words, (i, j))
                gone = sum(b - a for (a, b, _) in built)
                if kind:
                    c[f"plan_{kind}"] += 1
                    c["removed_plan"] += n - 1
                    plan.append({"start": words[i]["start"], "end": words[j]["end"], "n": n, "kind": kind,
                                 "norm": words[i]["norm"], "i": i, "j": j})
                    if (n - 1) - gone:
                        c["refined_pieces"] += 1
                        c["refined_kept"] += (n - 1) - gone
                for (a, b, bk) in built:
                    m = b - a + 1
                    c[f"collapsed_{bk}"] += 1
                    c["removed"] += m - 1
                    c[f"removed_{bk}"] += m - 1
                    if words[a]["seg"] is not None and words[a]["seg"] != words[b]["seg"]:
                        c["crossing_segment"] += 1
                    nxt = words[b + 1]["norm"] if b + 1 < len(words) else ""
                    if nxt.startswith(words[a]["norm"]) and len(nxt) > len(words[a]["norm"]):
                        c["false_start"] += 1
                    bursts.append({"start": words[a]["start"], "end": words[b]["end"], "n": m, "kind": bk,
                                   "norm": words[a]["norm"], "i": a, "j": b})
                if refs is None:
                    continue
                # Reference copies of the token around: each collapsed burst
                # ("c"), each 4+ piece the timing rule keeps ("k"), and each
                # loop whose human-pace words the rule as built keeps ("r",
                # against the copies that remain).
                checks = [("c", a, b, b - a + 1) for (a, b, _) in built]
                if n >= 4 and kind is None:
                    checks.append(("k", i, j, n))
                if kind and (n - 1) - gone:
                    checks.append(("r", i, j, n - gone))
                for side, a, b, want in checks:
                    if ref_norm is None:
                        toks = normalize_token(words[a]["text"])
                        tok = toks[0] if toks else None
                    else:
                        tok = ref_norm(words[a]["text"]) or None
                    if tok is None:
                        c[f"ref_{side}_filler"] += 1
                        continue
                    lo_t, hi_t = words[a]["start"] / 1000 - margin, words[b]["end"] / 1000 + margin
                    lo = bisect.bisect_left(refs[1], lo_t)
                    have = sum(1 for k in range(lo, len(refs[0])) if refs[1][k] <= hi_t and refs[0][k].word == tok)
                    c[f"ref_{side}_pieces"] += 1
                    c[f"ref_{side}_le1"] += have <= 1
                    c[f"ref_{side}_some"] += 1 < have < want
                    c[f"ref_{side}_all"] += have >= want
        # What the speaker condition kept (expected 0: plan 16 section 12.3).
        no_spk = sum(b - a for pieces in loop_runs(words, key=lambda w: w["norm"]) for p in pieces
                     for (a, b, _) in loop_bursts(words, p))
        c["removed_without_speaker_rule"] += no_spk
        return bursts, plan

    def row(self) -> list[str]:
        c = self.c
        return [
            str(c["words"]), f"{c['pairs']} ({c['pairs_one_frame']})",
            f"{c['triples']} ({c['triples_same_frame']} / {c['triples_one_frame_spacing']})",
            f"{c['runs4']} ({c['runs4_fast']} / {c['runs4_81_160']} / {c['runs4_slow']})",
            f"{c['runs_split']} ({c['runs_split_collapsed']})",
            f"{c['plan_loop']} / {c['plan_frame3']}", str(c["removed_plan"]),
            f"{c['collapsed_loop']} / {c['collapsed_frame3']}", str(c["removed"]),
            f"{c['kept4_pieces']} ({c['kept4_words']})",
        ]


LOOP_HEAD = (
    "| {src} | Channel | Words | Pairs (one frame) | Runs of 3 (same frame / one-frame spacing) | "
    "Pieces of 4+ (median ≤ 80 / 81–160 / > 160 ms) | Runs split at 400 ms (with a loop) | "
    "Loops by the plan's rule (4+ / 3 in a frame) | Words removed, plan's rule | "
    "Bursts collapsed as built (4+ / 3 in a frame) | Words removed as built | 4+ kept (words) |"
)
LOOP_COLS = 12


def _summary(total: LoopTally, crossing: bool) -> None:
    c = total.c
    print(f"\nPieces by length, median start gap p10/p50/p90 (ms): 3: {_q(total.gaps['3'], .1)}/{_q(total.gaps['3'], .5)}/"
          f"{_q(total.gaps['3'], .9)} ({len(total.gaps['3'])}); 4-5: {_q(total.gaps['4-5'], .1)}/{_q(total.gaps['4-5'], .5)}/"
          f"{_q(total.gaps['4-5'], .9)} ({len(total.gaps['4-5'])}); 6+: {_q(total.gaps['6+'], .1)}/"
          f"{_q(total.gaps['6+'], .5)}/{_q(total.gaps['6+'], .9)} ({len(total.gaps['6+'])})")
    print(f"Loops by the plan's rule whose human-pace words the rule as built keeps: {c['refined_pieces']} "
          f"({c['refined_kept']} words kept that the plan's rule removes)")
    print(f"Collapsed bursts crossing a segment boundary: {c['crossing_segment'] if crossing else 'n/a (import words)'}; "
          f"followed by a word that starts with the token (false start): {c['false_start']}; 4+ pieces with a median "
          f"gap of 81 to 119 ms (clock rounding): {c['runs4_jitter']}; words a speaker-label change kept from the "
          f"collapse: {c['removed_without_speaker_rule'] - c['removed']}")


def _ref_table(c: Counter, margin: float) -> None:
    print("\nReference copies of the token within "
          f"{margin:.1f} s of each piece (fillers have no reference token):\n")
    print("| Pieces | Count | Filler | Reference has at most 1 | 2 to n − 1 | n or more |")
    print("|---|---|---|---|---|---|")
    for side, name in (("c", "bursts collapsed as built"), ("k", "4+ kept by the timing rule"),
                       ("r", "loops whose human-pace words the rule as built keeps (n: the copies kept)")):
        print(f"| {name} | {c[f'ref_{side}_pieces'] + c[f'ref_{side}_filler']} | {c[f'ref_{side}_filler']} | "
              f"{c[f'ref_{side}_le1']} | {c[f'ref_{side}_some']} | {c[f'ref_{side}_all']} |")


def _ref_words(session):
    """The session's reference as normalised words with times (cue-level
    Zoom times interpolated by length), and their starts."""
    import run as R
    from common import interpolate_words

    segs, _, _ = R.load_reference(session)
    words = interpolate_words(segs)
    return words, [w.start for w in words]


class _RefWord:
    __slots__ = ("word", "start")

    def __init__(self, word: str, start: float) -> None:
        self.word, self.start = word, start


def _spread(rows: list[tuple[float, float, str]]) -> tuple[list, list]:
    """Reference words with times from (start, end, text) rows: each row's
    whitespace tokens spread over its span by length, as loop_norm tokens."""
    out = []
    for a, b, text in rows:
        toks = [t for t in (loop_norm(x) for x in text.split()) if t]
        total = sum(len(t) for t in toks) or 1
        t = a
        for tok in toks:
            out.append(_RefWord(tok, t))
            t += (b - a) * len(tok) / total
    out.sort(key=lambda w: w.start)
    return out, [w.start for w in out]


def _fleurs_refs(root: Path, sid: str):
    """`fleurs-<lang>-<mode>`: the utterances of <root>/<lang>/refs.jsonl
    (FLEURS's normalised transcription over each utterance's span)."""
    lang = sid.split("-")[1]
    path = root / lang / "refs.jsonl"
    rows = [json.loads(x) for x in path.read_text(encoding="utf-8").splitlines() if x.strip()]
    return _spread([(float(r["start"]), float(r["end"]), r.get("transcription") or r["raw_transcription"]) for r in rows])


def _extra_sessions(paths: list[str], root: Path | None) -> dict[str, dict]:
    """Sessions of further sessions TOMLs (for example a public set's), by
    id: their label, group and `ref_seglst` (relative to `root`, else to
    the TOML's folder)."""
    import tomllib

    out = {}
    for p in paths:
        p = Path(p)
        for sid, s in tomllib.loads(p.read_text(encoding="utf-8")).get("sessions", {}).items():
            ref = s.get("ref_seglst")
            out[sid] = {"label": s.get("label", sid), "group": s.get("group", "extra"),
                        "ref": (root or p.parent) / ref if ref else None}
    return out


def _loops_eval(a) -> None:
    sessions = load_sessions(a.sessions_file) if Path(a.sessions_file).is_file() else {}
    extra = _extra_sessions(a.extra_sessions or [], Path(a.extra_root) if a.extra_root else None)
    fleurs_root = Path(a.fleurs_root) if a.fleurs_root else WORK / "fleurs"
    runs = Path(a.runs_dir) if a.runs_dir else RUNS
    for cfg in [x.strip() for x in a.configs.split(",") if x.strip()]:
        if a.sessions == "all":
            sids = list(sessions)
        elif a.sessions.split(":")[0] == "present":
            prefix = a.sessions.partition(":")[2]
            sids = sorted(d.name for d in (runs / cfg).iterdir()
                          if d.name.startswith(prefix) and (d / "engine.words.json").is_file()) \
                if (runs / cfg).is_dir() else []
        else:
            sids = [x.strip() for x in a.sessions.split(",") if x.strip()]
        print(f"\n### {cfg}\n")
        print(LOOP_HEAD.format(src="Session"))
        print("|" + "---|" * LOOP_COLS)
        total = LoopTally()
        groups: dict[str, LoopTally] = {}
        for sid in sids:
            p = runs / cfg / sid / "engine.words.json"
            if not p.is_file():
                continue
            words = engine_stream(p)
            refs, ref_norm = None, None
            if sid in sessions:
                label, group = sessions[sid].label, sessions[sid].group
            elif sid in extra:
                label, group = extra[sid]["label"], extra[sid]["group"]
            else:
                label, group = sid, sid.split("-")[0]
            if not a.no_ref:
                try:
                    if sid in sessions:
                        refs = _ref_words(sessions[sid])
                    elif sid in extra and extra[sid]["ref"]:
                        segs = read_seglst(extra[sid]["ref"])
                        refs, ref_norm = _spread([(float(r["start_time"]), float(r["end_time"]), str(r["words"]))
                                                  for r in segs]), loop_norm
                    elif sid.startswith("fleurs-"):
                        refs, ref_norm = _fleurs_refs(fleurs_root, sid), loop_norm
                except (SystemExit, OSError, ValueError, KeyError) as e:
                    print(f"{sid}: no reference ({type(e).__name__})", file=sys.stderr)
            t = LoopTally()
            t.add(words, refs, a.margin, ref_norm)
            total.merge(t)
            groups.setdefault(group, LoopTally()).merge(t)
            print(f"| {label} | source | " + " | ".join(t.row()) + " |")
        for g, t in groups.items():
            print(f"| group {g} | source | " + " | ".join(t.row()) + " |")
        print("| all | source | " + " | ".join(total.row()) + " |")
        _summary(total, crossing=False)
        if not a.no_ref:
            _ref_table(total.c, a.margin)


def _parse_meetings(spec: str) -> list[int]:
    out = []
    for part in spec.split(","):
        part = part.strip()
        if "-" in part:
            lo, hi = part.split("-")
            out += list(range(int(lo), int(hi) + 1))
        elif part:
            out.append(int(part))
    return out


def _analysis_spans(rec_list: list[dict], words: list[dict]) -> list[tuple[float, float]]:
    """A loop record's span (s): from its start to the last start of the
    same-norm words that follow it in `words` (speaker and splits ignored)."""
    spans = []
    starts = [w["start"] / 1000 for w in words]
    for r in rec_list:
        k = bisect.bisect_left(starts, r["t"] - 0.15)
        if k >= len(words) or starts[k] > r["t"] + 0.15:
            spans.append((r["t"], r["t"]))
            continue
        e = k
        while e + 1 < len(words) and words[e + 1]["norm"] == words[k]["norm"]:
            e += 1
        spans.append((starts[k], starts[e]))
    return spans


def _loops_db(a) -> None:
    import sqlite3

    con = sqlite3.connect(a.db)
    analysis = json.loads(Path(a.analysis).read_text(encoding="utf-8")) if a.analysis else None
    print(LOOP_HEAD.format(src="Meeting"))
    print("|" + "---|" * LOOP_COLS)
    total = LoopTally()
    per_segment: Counter = Counter()
    share: Counter = Counter()
    for mid in _parse_meetings(a.meetings):
        row = con.execute("select current_run_id from meetings where id = ?", (mid,)).fetchone()
        if not row or row[0] is None:
            continue
        streams = db_streams(con, row[0])
        for ch in sorted(streams, key=lambda c: ("loopback", "mic", "source").index(c) if c in ("loopback", "mic", "source") else 9):
            words = streams[ch]
            t = LoopTally()
            bursts, plan = t.add(words)
            total.merge(t)
            print(f"| {mid} | {ch} | " + " | ".join(t.row()) + " |")
            # The rule as built and the plan's rule over the channel stream
            # and within one segment, and the draft rule before the review
            # (the reviewer's and the planner's counts of section 12.3).
            seg = lambda w: (w["norm"], w["speaker"], w["seg"])  # noqa: E731
            per_segment["built stream", mid] += t.c["removed"]
            per_segment["built segment", mid] += loop_removed(words, seg)
            per_segment["plan stream", mid] += t.c["removed_plan"]
            per_segment["plan segment", mid] += loop_removed(words, seg, rule="plan")
            per_segment["draft stream", mid] += loop_removed(words, split=False, triple_median=True)
            per_segment["draft segment", mid] += loop_removed(words, seg, split=False, triple_median=True)
            if analysis and str(mid) in analysis:
                recs = analysis[str(mid)]
                key = "POST_LB" if ch == "loopback" else "POST_ME"
                tie = "N35_LB" if ch == "loopback" else ("AEC_N35" if "STORED_AEC" in recs else "RAW_N35")
                spans = _analysis_spans(recs.get(key, {}).get("list", []), words)
                tie_spans = _analysis_spans(recs.get(tie, {}).get("list", []), words)
                wh2 = [r.get("whisper_n", 0) >= 2 for r in recs.get(key, {}).get("list", [])]
                for rule, pieces in (("built", bursts), ("plan", plan)):
                    for p in pieces:
                        s0, s1 = p["start"] / 1000, words[p["j"]]["start"] / 1000
                        hit = [k for k, (x, y) in enumerate(spans) if x - 0.15 <= s1 and s0 <= y + 0.15]
                        removed = p["n"] - 1
                        share[rule, "removed", ch] += removed
                        if hit:
                            share[rule, "labelled", ch] += removed
                            share[rule, "labelled_whisper2", ch] += removed if any(wh2[k] for k in hit) else 0
                            if not any(x - 0.5 <= s1 and s0 <= y + 0.5 for x, y in tie_spans):
                                share[rule, "en_only", ch] += removed
                        elif p["kind"] == "frame3":
                            share[rule, "frame3_unlabelled", ch] += removed
    print("| all | both | " + " | ".join(total.row()) + " |")
    _summary(total, crossing=True)
    ms = sorted({m for (_, m) in per_segment})
    print("\nWords removed per meeting:\n")
    print("| Count | " + " | ".join(str(m) for m in ms) + " | Total |")
    print("|---|" + "---|" * (len(ms) + 1))
    for k, name in (("built stream", "the rule as built (frame-level bursts), channel stream"),
                    ("built segment", "the rule as built, within one segment"),
                    ("plan stream", "the plan's rule (a loop goes whole), channel stream"),
                    ("plan segment", "the plan's rule, within one segment"),
                    ("draft stream", "draft rule (no split, 3 at a one-frame median), channel stream"),
                    ("draft segment", "draft rule, within one segment")):
        print(f"| {name} | " + " | ".join(str(per_segment[k, m]) for m in ms) + f" | {sum(per_segment[k, m] for m in ms)} |")
    if analysis:
        for rule in ("built", "plan"):
            print(f"\n{rule}:")
            for ch in ("loopback", "mic"):
                r = share[rule, "removed", ch]
                if not r:
                    continue
                print(f"{ch}: {r} removed; in a loop the analysis recorded (Whisper has fewer than 3 copies) "
                      f"{share[rule, 'labelled', ch]} ({100 * share[rule, 'labelled', ch] / r:.1f} %), of them with no "
                      f"Nemotron 3.5 loop there (EN only) {share[rule, 'en_only', ch]} "
                      f"({100 * share[rule, 'en_only', ch] / r:.1f} %); in records where Whisper has 2 copies "
                      f"{share[rule, 'labelled_whisper2', ch]}; unlabelled 3-in-a-frame {share[rule, 'frame3_unlabelled', ch]}")
            r = share[rule, "removed", "loopback"] + share[rule, "removed", "mic"]
            if r:
                lab = share[rule, "labelled", "loopback"] + share[rule, "labelled", "mic"]
                en = share[rule, "en_only", "loopback"] + share[rule, "en_only", "mic"]
                print(f"both: {r} removed; labelled {lab} ({100 * lab / r:.1f} %); EN only {en} ({100 * en / r:.1f} %)")


def cmd_loops(a) -> None:
    if a.db:
        _loops_db(a)
    else:
        _loops_eval(a)


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
    p = sub.add_parser("fleurs-utts")
    p.add_argument("--langs", default="de,es,fr,it,pt,hi,ja,en")
    p.add_argument("--configs", default="nemotron35,n35_rc_13")
    p.add_argument("--mode", default="given")
    p.add_argument("--bad", type=float, default=0.5, help="list utterances with at least this error rate")
    p = sub.add_parser("compare")
    p.add_argument("--configs", required=True)
    p.add_argument("--sessions", default="all")
    p = sub.add_parser("table")
    p.add_argument("--configs", required=True)
    p.add_argument("--sessions", default="ami,aug2,sep5")
    p.add_argument("--long", default="aug14")
    p.add_argument("--base-n35", default="nemotron35")
    p.add_argument("--base-en", default="nemotron_en")
    p = sub.add_parser("punct")
    p.add_argument("--words", nargs="+", required=True)
    p = sub.add_parser("loops")
    p.add_argument("--configs", default="shipped_en", help="eval run configs (their engine.words.json)")
    p.add_argument("--sessions", default="all",
                   help="comma-separated ids, all (the sessions file's), or present[:<prefix>] (every cached run of the config)")
    p.add_argument("--runs-dir", help="another .work/runs folder to read (read only)")
    p.add_argument("--extra-sessions", nargs="*",
                   help="further sessions TOMLs (labels, groups, ref_seglst), e.g. a public set's")
    p.add_argument("--extra-root", help="the folder their ref_seglst paths are relative to (default: each TOML's folder)")
    p.add_argument("--fleurs-root", help="FLEURS data for fleurs-<lang>-* sessions (default .work/fleurs)")
    p.add_argument("--no-ref", action="store_true", help="skip the reference check")
    p.add_argument("--margin", type=float, default=1.0, help="reference window around a piece (s)")
    p.add_argument("--db", help="a copy of a data root's diarist.db: its meetings' current runs instead of eval runs")
    p.add_argument("--meetings", default="1-1000", help="meeting ids for --db, e.g. 12-21")
    p.add_argument("--analysis", help="a word-error analysis's loops JSON, for the EN-only share (--db)")
    a = ap.parse_args()
    {"offsets": cmd_offsets, "tags": cmd_tags, "diff": cmd_diff, "ami-misses": cmd_ami_misses, "clip-score": cmd_clip_score, "table": cmd_table, "boost": cmd_boost, "fleurs-utts": cmd_fleurs_utts, "compare": cmd_compare,
     "punct": cmd_punct, "loops": cmd_loops}[a.cmd](a)


if __name__ == "__main__":
    main()
