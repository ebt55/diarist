"""Metrics for one meeting: diarization (DER, JER, speaker count) on RTTM
pairs, and transcript-level attribution (WDER, cpWER, tcpWER, DI-cpWER) on
SegLST pairs.

Conventions
- DER/JER: pyannote.metrics, overlap scored. "collar 0.25" means the NIST
  convention, +/-0.25 s around every reference boundary; pyannote's `collar`
  argument is the total width, so it gets 0.5. The scored region (UEM) is
  [0, audio duration] unless the session sets one (AMI: the annotated span,
  because the pre-meeting chatter in the first 50 s is not transcribed);
  hypothesis words whose midpoint falls outside it are dropped for the
  transcript metrics too.
- Text: reference and hypothesis go through the same per-token normaliser
  (common.normalize_token); fillers are dropped on both sides. Sessions in
  another language use the multilingual normaliser (common.token_normalizer:
  NFKC, casefold, punctuation removed); Mandarin and Japanese are scored per
  character, so cpWER there is a speaker-attributed CER.
- cpWER, tcpWER (collar 5 s), DI-cpWER and greedy ORC-WER (speaker-agnostic
  WER; exact ORC-WER does not fit in memory for a 20-minute meeting) via
  meeteval. DI-cpWER is computed with every hypothesis word as its own
  segment, so it forgives every attribution error: cpWER - DI-cpWER is the
  full attribution cost.
- WDER: hypothesis and reference words (time-ordered, speaker-agnostic) are
  aligned by Levenshtein distance on normalised words; aligned pairs (correct
  or substituted) vote for a hypothesis->reference speaker map (Hungarian, one
  to one); WDER = aligned pairs whose hypothesis speaker does not map to the
  reference speaker / aligned pairs. "near" restricts to reference words within
  1 s of a reference speaker change.
- WER (speaker-agnostic): the same Levenshtein alignment of time-ordered
  normalised words as WDER; w_sub, w_del and w_ins are its counts (research/19
  section 0).
- Gaps (research/19 section 0): stretches of at least GAP_MIN_S without a
  hypothesis word in which the reference has at least GAP_MIN_REF_WORDS words.
  A stretch runs from one hypothesis word's end to the next one's start (and
  from the scored region's start, to its end); a reference word counts when
  its midpoint lies inside the stretch shrunk by GAP_MARGIN_S at both ends
  (Nemotron word times run about 0.3 s late). gaps_5s counts those of at least
  GAP_LONG_S. Turn-level references spread a turn's span over its words, and
  per-character languages count characters, so gaps there are approximate.

CLI (one meeting, files from anywhere):
  uv run python -I metrics.py --ref-seglst r.jsonl --ref-rttm r.rttm \
      --hyp-json transcript.json [--hyp-rttm h.rttm] [--duration 2650]
"""

from __future__ import annotations

import argparse
import bisect
import json
import logging
import os
import sys
import time
import warnings
from collections import defaultdict
from pathlib import Path

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import (  # noqa: E402
    RttmRow,
    Word,
    interpolate_words,
    normalize_seglst,
    read_rttm,
    read_seglst,
)

# meeteval logs a warning when hypothesis words overlap by a few ms (Nemotron
# word spans do); it does not change the result.
logging.getLogger("error_rate").setLevel(logging.ERROR)

NEAR_CHANGE_S = 1.0
TCP_COLLAR_S = 5.0
GAP_MIN_S = 2.0
GAP_LONG_S = 5.0
GAP_MIN_REF_WORDS = 3
GAP_MARGIN_S = 0.5


# --------------------------------------------------------------------------
# Diarization


def _annotation(rows: list[RttmRow]):
    from pyannote.core import Annotation, Segment

    ann = Annotation()
    for i, r in enumerate(rows):
        if r.dur > 0:
            ann[Segment(r.start, r.end), i] = r.speaker
    return ann


def diarization_metrics(
    ref: list[RttmRow], hyp: list[RttmRow], duration: float | None = None, region: tuple[float, float] | None = None
) -> dict:
    from pyannote.core import Segment, Timeline
    from pyannote.metrics.diarization import DiarizationErrorRate, JaccardErrorRate

    r, h = _annotation(ref), _annotation(hyp)
    end = duration or max([x.end for x in ref] + [x.end for x in hyp] + [0.0])
    uem = Timeline([Segment(*region)]) if region else Timeline([Segment(0.0, end)])
    out: dict = {}
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        for name, collar in (("der_c025", 0.5), ("der_c0", 0.0)):
            m = DiarizationErrorRate(collar=collar, skip_overlap=False)
            d = m(r, h, uem=uem, detailed=True)
            total = d["total"] or 1e-9
            out[name] = d["diarization error rate"]
            out[name + "_total_s"] = d["total"]  # scored reference speech, for pooling
            out[name + "_miss"] = d["missed detection"] / total
            out[name + "_fa"] = d["false alarm"] / total
            out[name + "_conf"] = d["confusion"] / total
        out["jer"] = JaccardErrorRate(collar=0.0, skip_overlap=False)(r, h, uem=uem)
    out["ref_speakers"] = len({x.speaker for x in ref})
    out["hyp_speakers"] = len({x.speaker for x in hyp})
    out["count_error"] = out["hyp_speakers"] - out["ref_speakers"]
    return out


# --------------------------------------------------------------------------
# WDER


def change_points(ref_words: list[Word]) -> list[float]:
    pts = []
    for a, b in zip(ref_words, ref_words[1:]):
        if a.speaker != b.speaker:
            pts.append((a.end + b.start) / 2 if a.end <= b.start else b.start)
    return pts


def _near(w: Word, pts: list[float], radius: float) -> bool:
    i = bisect.bisect_left(pts, w.start - radius)
    return i < len(pts) and pts[i] <= w.end + radius


def wder(ref_words: list[Word], hyp_words: list[Word], radius: float = NEAR_CHANGE_S) -> dict:
    from rapidfuzz.distance import Levenshtein
    from scipy.optimize import linear_sum_assignment

    ref_tok = [w.word for w in ref_words]
    hyp_tok = [w.word for w in hyp_words]
    pairs: list[tuple[int, int]] = []
    sub = dele = ins = 0
    for op in Levenshtein.opcodes(ref_tok, hyp_tok):
        n_ref, n_hyp = op.src_end - op.src_start, op.dest_end - op.dest_start
        if op.tag in ("equal", "replace"):
            n = min(n_ref, n_hyp)
            pairs.extend((op.src_start + k, op.dest_start + k) for k in range(n))
        if op.tag == "replace":
            sub += min(n_ref, n_hyp)
            dele += max(0, n_ref - n_hyp)
            ins += max(0, n_hyp - n_ref)
        elif op.tag == "delete":
            dele += n_ref
        elif op.tag == "insert":
            ins += n_hyp
    ref_spk = sorted({w.speaker for w in ref_words})
    hyp_spk = sorted({w.speaker for w in hyp_words})
    ri = {s: i for i, s in enumerate(ref_spk)}
    hi = {s: i for i, s in enumerate(hyp_spk)}
    conf = np.zeros((len(hyp_spk), len(ref_spk)), dtype=np.int64)
    for i, j in pairs:
        conf[hi[hyp_words[j].speaker], ri[ref_words[i].speaker]] += 1
    mapping: dict[str, str] = {}
    if conf.size:
        rows, cols = linear_sum_assignment(-conf)
        mapping = {hyp_spk[r]: ref_spk[c] for r, c in zip(rows, cols)}
    pts = change_points(ref_words)
    wrong = near = near_wrong = 0
    for i, j in pairs:
        bad = mapping.get(hyp_words[j].speaker) != ref_words[i].speaker
        wrong += bad
        if _near(ref_words[i], pts, radius):
            near += 1
            near_wrong += bad
    n = len(pairs)
    return {
        "wder": wrong / n if n else float("nan"),
        "wder_near": near_wrong / near if near else float("nan"),
        "wder_pairs": n,
        "wder_near_pairs": near,
        "ref_changes": len(pts),
        "ref_words": len(ref_words),
        "hyp_words": len(hyp_words),
        "speaker_map": mapping,
        # Speaker-agnostic WER on the same alignment.
        "wer": (sub + dele + ins) / len(ref_tok) if ref_tok else float("nan"),
        "w_sub": sub,
        "w_del": dele,
        "w_ins": ins,
    }


def word_gaps(
    ref_words: list[Word], hyp_words: list[Word], t0: float, t1: float, min_gap: float = GAP_MIN_S,
    min_ref: int = GAP_MIN_REF_WORDS, margin: float = GAP_MARGIN_S,
) -> list[dict]:
    """Stretches of [t0, t1] without a hypothesis word, at least min_gap long,
    holding at least min_ref reference words (midpoints, margin trimmed off
    both ends). Definition: research/19 section 0."""
    edges = [t0] + [x for w in sorted(hyp_words, key=lambda w: (w.start, w.end)) for x in (w.start, w.end)] + [t1]
    ref_mid = sorted((w.start + w.end) / 2 for w in ref_words)
    out = []
    for i in range(0, len(edges) - 1, 2):
        a, b = edges[i], edges[i + 1]
        if b - a < min_gap:
            continue
        n = bisect.bisect_right(ref_mid, b - margin) - bisect.bisect_left(ref_mid, a + margin)
        if n >= min_ref:
            out.append({"start": round(a, 2), "end": round(b, 2), "len": round(b - a, 2), "ref_words": n})
    return out


def gap_metrics(ref_words: list[Word], hyp_words: list[Word], t0: float, t1: float) -> dict:
    g = word_gaps(ref_words, hyp_words, t0, t1)
    return {
        "gaps": len(g),
        "gaps_5s": sum(1 for x in g if x["len"] >= GAP_LONG_S),
        "gap_ref_words": sum(x["ref_words"] for x in g),
        "longest_gap_s": max((x["len"] for x in g), default=0.0),
    }


# --------------------------------------------------------------------------
# meeteval


def _seglst(rows: list[dict]):
    import meeteval

    return meeteval.io.SegLST(rows)


def wer_metrics(ref_segs: list[dict], hyp_segs: list[dict], hyp_word_segs: list[dict]) -> dict:
    """ref_segs, hyp_segs, hyp_word_segs: normalised SegLST rows of one session."""
    import meeteval

    ref = _seglst(ref_segs)
    hyp = _seglst(hyp_segs)
    hyp_w = _seglst(hyp_word_segs)
    out: dict = {}
    t = time.perf_counter()
    cp = next(iter(meeteval.wer.cpwer(ref, hyp).values()))
    out.update(
        cpwer=cp.error_rate,
        cp_errors=cp.errors,
        cp_length=cp.length,
        cp_ins=cp.insertions,
        cp_del=cp.deletions,
        cp_sub=cp.substitutions,
        cp_missed_speaker=cp.missed_speaker,
        cp_falarm_speaker=cp.falarm_speaker,
    )
    tcp = next(iter(meeteval.wer.tcpwer(ref, hyp_w, collar=TCP_COLLAR_S).values()))
    out.update(tcpwer=tcp.error_rate, tcp_errors=tcp.errors, tcp_length=tcp.length)
    # The ORC-based measures run with the streams of one side as ORC's
    # "hypothesis" (DI-cpWER: the reference speakers), and meeteval 0.4.3
    # refuses more than 10 such streams ("Are you sure?"), even for its greedy
    # matching. Earnings-22 calls have up to 13 reference speakers; those
    # measures are then left out (NaN) rather than failing the session.
    def orc_based(fn, a, b) -> float:
        try:
            return next(iter(fn(a, b).values())).error_rate
        except RuntimeError as e:
            if "Are you sure" not in str(e):
                raise
            return float("nan")

    out["dicpwer"] = orc_based(meeteval.wer.greedy_dicpwer, ref, hyp_w)
    out["orcwer"] = orc_based(meeteval.wer.greedy_orcwer, ref, hyp)
    out["attribution_loss"] = out["cpwer"] - out["dicpwer"]
    out["meeteval_s"] = round(time.perf_counter() - t, 1)
    return out


# --------------------------------------------------------------------------
# Hypothesis loading (diarist-cli export --format json)


def hyp_from_export(doc: dict, session_id: str) -> tuple[list[dict], list[dict]]:
    """Returns (segment-level SegLST rows, one row per word), raw text."""
    labels = {s["id"]: s.get("label") or f"spk{s['id']}" for s in doc.get("speakers", [])}
    merged = {s["id"]: s.get("merged_into") for s in doc.get("speakers", [])}

    def label(sid):
        seen = set()
        while merged.get(sid) and sid not in seen:
            seen.add(sid)
            sid = merged[sid]
        return labels.get(sid, f"spk{sid}")

    segs, words = [], []
    for s in doc.get("segments", []):
        spk = label(s["speaker_id"])
        segs.append(
            {
                "session_id": session_id,
                "speaker": spk,
                "start_time": s["start_ms"] / 1000.0,
                "end_time": s["end_ms"] / 1000.0,
                "words": s["text"],
            }
        )
        for w in s.get("words", []):
            words.append(
                {
                    "session_id": session_id,
                    "speaker": spk,
                    "start_time": w["start_ms"] / 1000.0,
                    "end_time": max(w["end_ms"], w["start_ms"]) / 1000.0,
                    "words": w["text"],
                }
            )
    return segs, words


def meeting_metrics(
    ref_segs: list[dict],
    ref_rttm: list[RttmRow],
    hyp_segs: list[dict],
    hyp_word_segs: list[dict],
    hyp_rttm: list[RttmRow] | None,
    duration: float | None,
    session_id: str,
    region: tuple[float, float] | None = None,
    lang: str | None = None,
) -> dict:
    """lang: the session language; anything but English (or None) switches
    the text normaliser to the multilingual one (common.token_normalizer)."""
    for rows in (ref_segs, hyp_segs, hyp_word_segs):
        for r in rows:
            r["session_id"] = session_id
    if region:
        a, b = region
        inside = lambda r: a <= (r["start_time"] + r["end_time"]) / 2 <= b  # noqa: E731
        hyp_segs = [r for r in hyp_segs if inside(r)]
        hyp_word_segs = [r for r in hyp_word_segs if inside(r)]
    out: dict = {}
    if hyp_rttm is not None:
        out.update(diarization_metrics(ref_rttm, hyp_rttm, duration, region))
    ref_n = normalize_seglst(ref_segs, lang)
    hyp_n = normalize_seglst(hyp_segs, lang)
    hyp_wn = normalize_seglst(hyp_word_segs, lang)
    ref_w, hyp_w = interpolate_words(ref_segs, lang), interpolate_words(hyp_word_segs, lang)
    out.update(wder(ref_w, hyp_w))
    t0, t1 = region or (0.0, duration or max([w.end for w in ref_w + hyp_w] + [0.0]))
    out.update(gap_metrics(ref_w, hyp_w, t0, t1))
    out.update(wer_metrics(ref_n, hyp_n, hyp_wn))
    # Speakers that actually carry words, for the transcript-level count.
    out["hyp_speakers_words"] = len({r["speaker"] for r in hyp_wn})
    out["ref_speakers_words"] = len({r["speaker"] for r in ref_n})
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--ref-seglst", type=Path, required=True)
    ap.add_argument("--ref-rttm", type=Path, required=True)
    ap.add_argument("--hyp-json", type=Path, required=True, help="diarist-cli export --format json")
    ap.add_argument("--hyp-rttm", type=Path)
    ap.add_argument("--duration", type=float)
    ap.add_argument("--session-id", default="s")
    ap.add_argument("--language", help="session language (default English normaliser)")
    a = ap.parse_args()
    doc = json.loads(a.hyp_json.read_text(encoding="utf-8"))
    segs, words = hyp_from_export(doc, a.session_id)
    res = meeting_metrics(
        read_seglst(a.ref_seglst),
        read_rttm(a.ref_rttm),
        segs,
        words,
        read_rttm(a.hyp_rttm) if a.hyp_rttm else None,
        a.duration,
        a.session_id,
        lang=a.language,
    )
    print(json.dumps(res, indent=2, default=str))


if __name__ == "__main__":
    main()
