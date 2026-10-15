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
  (common.normalize_token); fillers are dropped on both sides.
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
    for op in Levenshtein.opcodes(ref_tok, hyp_tok):
        if op.tag in ("equal", "replace"):
            n = min(op.src_end - op.src_start, op.dest_end - op.dest_start)
            pairs.extend((op.src_start + k, op.dest_start + k) for k in range(n))
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
    out["tcpwer"] = tcp.error_rate
    di = next(iter(meeteval.wer.greedy_dicpwer(ref, hyp_w).values()))
    out["dicpwer"] = di.error_rate
    orc = next(iter(meeteval.wer.greedy_orcwer(ref, hyp).values()))
    out["orcwer"] = orc.error_rate
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
) -> dict:
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
    ref_n = normalize_seglst(ref_segs)
    hyp_n = normalize_seglst(hyp_segs)
    hyp_wn = normalize_seglst(hyp_word_segs)
    out.update(wder(interpolate_words(ref_segs), interpolate_words(hyp_word_segs)))
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
    )
    print(json.dumps(res, indent=2, default=str))


if __name__ == "__main__":
    main()
