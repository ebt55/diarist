"""Synthetic checks for gapfill.py's fill rule (docs/plans/18-beta6.md section
3.3) and run.py's cache key for replayed configs (no GPU, no private data).

  uv run python -I -m pytest test_gapfill.py   (or: uv run python -I test_gapfill.py)
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gapfill as G  # noqa: E402


def w(word, start, end=None, spk=1):
    return {"word": word, "start": start, "end": start + 0.08 if end is None else end, "speaker": spk, "confidence": 1}


def doc(words, duration=60.0):
    return {"duration": duration, "languages": [], "text": " ".join(x["word"] for x in words), "words": words}


def timeline(words: list[dict]) -> list[tuple]:
    return [(x["word"], x["start"], x["speaker"]) for x in words]


def test_to_us_rounds_like_secs_to_us():
    assert G.to_us(0.08) == 80_000
    assert G.to_us(-1.0) == 0
    assert G.to_us(1.0000005) == 1_000_001  # half away from zero
    assert G.to_us(12.64) == 12_640_000


def test_stretches_lead_between_trail_and_overlap():
    sp = [(1_000_000, 1_080_000), (1_050_000, 2_000_000), (2_000_000, 2_080_000), (5_000_000, 5_080_000)]
    st = G.stretches(sp, 10_000_000)
    assert st == [
        (0, 1_000_000, None, 0),
        (2_080_000, 5_000_000, 2, 3),
        (5_080_000, 10_000_000, 3, None),
    ]
    # A word that ends later than the next one keeps the coverage.
    st = G.stretches([(0, 3_000_000), (1_000_000, 1_080_000), (4_000_000, 4_080_000)], 4_080_000)
    assert st == [(3_000_000, 4_000_000, 0, 2)]


def test_fill_inserts_only_inside_the_shrunk_stretch_of_at_least_g():
    en = [w("a", 1.0), w("b", 1.2), w("c", 4.0), w("d", 4.2), w("e", 5.0)]
    # Stretch 1.28 .. 4.0 (2.72 s): window 1.53 .. 3.75.
    n35 = [
        w("a", 1.0), w("b", 1.2),
        w("early", 1.50),          # starts before the window
        w("in1", 1.53), w("in2", 2.5), w("in3", 3.67),  # inside (3.67 + 0.08 = 3.75)
        w("late", 3.70),           # ends after the window
        w("c", 4.0), w("d", 4.2),
        w("short", 4.5),           # inside 4.28 .. 5.0, which is under G
        w("e", 5.0),
    ]
    fused, rep = G.fill(doc(en, 6.0), doc(n35, 6.0), 1.5)
    words = [x["word"] for x in fused["words"]]
    assert words == ["a", "b", "in1", "in2", "in3", "c", "d", "e"]
    assert rep["stretches"] == 1 and rep["gaps"] == 1 and rep["words_filled"] == 3 and rep["unmapped"] == 0
    # Every primary word is there, unchanged and in order.
    assert [x for x in fused["words"] if x["word"] in "abcde"] == en
    # The trailing stretch 5.08 .. 6.0 is under G; with G = 0.9 it qualifies
    # but its window 5.33 .. 5.75 is empty.
    _, rep = G.fill(doc(en, 6.0), doc(n35, 6.0), 0.9)
    assert rep["stretches"] == 3 and rep["words_filled"] == 3


def test_leading_and_trailing_stretches_are_filled():
    en = [w("mid", 10.0)]
    n35 = [w("x", 2.0), w("mid", 10.0), w("y", 15.0)]
    fused, rep = G.fill(doc(en, 20.0), doc(n35, 20.0), 2.0)
    assert [x["word"] for x in fused["words"]] == ["x", "mid", "y"]
    assert rep["stretches"] == 2 and rep["gaps"] == 2


def test_labels_map_by_co_timed_words_one_to_one():
    # 3.5 calls EN's speaker 1 "7" and EN's speaker 2 "8".
    en = [w("a", 1.0, spk=1), w("b", 1.08, spk=1), w("c", 9.0, spk=2), w("d", 9.08, spk=2), w("e", 9.16, spk=2)]
    n35 = [w("a", 1.0, spk=7), w("b", 1.08, spk=7), w("gap1", 3.0, spk=7), w("gap2", 6.0, spk=8),
           w("c", 9.0, spk=8), w("d", 9.08, spk=8), w("e", 9.16, spk=8)]
    fused, rep = G.fill(doc(en, 10.0), doc(n35, 10.0), 1.0)
    assert timeline(fused["words"])[2:4] == [("gap1", 3.0, 1), ("gap2", 6.0, 2)]
    assert rep["mapping"] == {"7": 1, "8": 2} and rep["unmapped"] == 0


def test_pairs_need_sixty_percent_of_the_longer_span():
    # A one-frame EN word against a four-frame 3.5 word overlaps 25 percent of
    # the longer span: no vote, so 3.5's label stays unmapped.
    en = [w("a", 1.0, spk=1), w("b", 8.0, spk=1)]
    n35 = [w("aa", 1.0, 1.32, spk=5), w("x", 4.0, spk=5)]
    _, rep = G.fill(doc(en, 10.0), doc(n35, 10.0), 1.0)
    assert rep["mapping"] == {} and rep["unmapped"] == 1
    # Two frames against three: 0.16 / 0.24 = 67 percent, a vote.
    n35 = [w("aa", 1.0, 1.24, spk=5), w("x", 4.0, spk=5)]
    en = [w("a", 1.0, 1.16, spk=1), w("b", 8.0, spk=1)]
    _, rep = G.fill(doc(en, 10.0), doc(n35, 10.0), 1.0)
    assert rep["mapping"] == {"5": 1} and rep["unmapped"] == 0


def test_unmapped_label_takes_the_nearer_bounding_word():
    # 3.5 has three speakers where EN has two: label 9 gets no mapping.
    en = [w("a", 1.0, spk=1), w("b", 1.08, spk=1), w("c", 9.0, spk=2), w("d", 9.08, spk=2)]
    n35 = [w("a", 1.0, spk=3), w("b", 1.08, spk=3), w("c", 9.0, spk=4), w("d", 9.08, spk=4),
           w("near_b", 2.0, spk=9), w("near_c", 8.0, spk=9), w("mid", 5.04, spk=9)]
    fused, rep = G.fill(doc(en, 10.0), doc(n35, 10.0), 1.0)
    got = {x["word"]: x["speaker"] for x in fused["words"]}
    # Stretch 1.16 .. 9.0: "mid" spans 5.04 .. 5.12, 3.88 s from b's end and
    # 3.88 s to c's start: the tie goes to the earlier word.
    assert got["near_b"] == 1 and got["near_c"] == 2 and got["mid"] == 1
    assert rep["unmapped"] == 3 and rep["unmapped_labels"] == ["9"]


def test_oracle_fills_only_where_the_words_beat_an_empty_stretch():
    en = [w("a", 1.0), w("b", 10.0), w("c", 20.0)]
    n35 = [w("a", 1.0), w("right", 5.0), w("words", 5.5), w("b", 10.0), w("wrong", 15.0), w("c", 20.0)]
    ref = [("a", 1.04), ("right", 5.04), ("words", 5.54), ("b", 10.04), ("other", 15.04), ("thing", 15.5), ("c", 20.04)]
    fused, rep = G.fill(doc(en, 21.0), doc(n35, 21.0), 2.0, oracle_ref=ref)
    assert [x["word"] for x in fused["words"]] == ["a", "right", "words", "b", "c"]
    assert rep["gaps"] == 1 and rep["declined"] == 1


def test_no_second_words_leaves_the_primary_alone():
    en = [w("a", 1.0), w("b", 10.0)]
    fused, rep = G.fill(doc(en), doc([]), 1.0)
    assert fused["words"] == en and rep["words_filled"] == 0 and rep["gaps"] == 0


def test_run_cache_key_includes_replay_and_gap_fill():
    import run as R

    with tempfile.TemporaryDirectory() as tmp:
        old = R.WORK
        R.WORK = Path(tmp)
        try:
            cfg = {"asr": "nemotron-en", "diarizer": "nemotron-3-diarization", "language": "en", "punctuation": True,
                   "replay": "en", "gap_fill": {"fill": "n35", "g_s": 1.5}}
            d = R.run_dir("c", "s")
            d.mkdir(parents=True)
            (d / "transcript.json").write_text("{}", encoding="utf-8")
            meta = {"schema": R.RUN_SCHEMA, "status": "ok", "asr": "nemotron-en", "diarizer": "nemotron-3-diarization",
                    "language": "en", "engine_args": [], "punctuation": True, "collapse_loops": True,
                    "replay": "en", "gap_fill": {"fill": "n35", "g_s": 1.5}}
            (d / "run.json").write_text(json.dumps(meta), encoding="utf-8")
            assert R.cached("c", "s", cfg) is not None
            assert R.cached("c", "s", {**cfg, "gap_fill": {"fill": "n35", "g_s": 2.0}}) is None
            assert R.cached("c", "s", {k: v for k, v in cfg.items() if k != "gap_fill"}) is None
            assert R.cached("c", "s", {**cfg, "replay": "n35"}) is None
        finally:
            R.WORK = old


if __name__ == "__main__":
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print(f"ok {name}")
