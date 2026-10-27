"""Sanity checks for metrics.py on synthetic meetings (no GPU, no private data).

  uv run python -I -m pytest test_metrics.py   (or: uv run python -I test_metrics.py)
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import metrics as M  # noqa: E402
from common import RttmRow, normalize_text, normalize_token, seglst_to_rttm  # noqa: E402

TEXT = {
    "A": "so the budget for next quarter is mostly travel and hardware",
    "B": "right and what about the contractors we hired in spring",
    "C": "they are paid out of the research line not this one",
}


def meeting(order=("A", "B", "A", "C", "B", "A"), names=None) -> list[dict]:
    names = names or {}
    t, out = 0.0, []
    for i, spk in enumerate(order):
        n = len(TEXT[spk].split())
        out.append(
            {"session_id": "x", "speaker": names.get(spk, spk), "start_time": t, "end_time": t + 0.4 * n, "words": TEXT[spk]}
        )
        t += 0.4 * n + 0.6
    return out


def words_of(segs):
    out = []
    for s in segs:
        toks = s["words"].split()
        step = (s["end_time"] - s["start_time"]) / len(toks)
        for k, w in enumerate(toks):
            a = s["start_time"] + k * step
            out.append({**s, "start_time": a, "end_time": a + step, "words": w})
    return out


def run(ref, hyp):
    return M.meeting_metrics(
        [dict(r) for r in ref], seglst_to_rttm(ref, "x"), [dict(h) for h in hyp], words_of(hyp),
        seglst_to_rttm(hyp, "x"), None, "x",
    )


def test_perfect_hypothesis_with_other_labels():
    ref = meeting()
    hyp = meeting(names={"A": "spk_0", "B": "spk_1", "C": "spk_2"})
    r = run(ref, hyp)
    assert r["wder"] == 0 and r["wder_near"] == 0
    assert r["cpwer"] == 0 and r["dicpwer"] == 0 and r["tcpwer"] == 0
    assert abs(r["der_c0"]) < 1e-9 and abs(r["jer"]) < 1e-9
    assert r["count_error"] == 0


def test_one_speaker_hypothesis():
    ref = meeting()
    hyp = meeting(names={"A": "spk_0", "B": "spk_0", "C": "spk_0"})
    r = run(ref, hyp)
    n = {k: sum(len(s["words"].split()) for s in ref if s["speaker"] == k) for k in "ABC"}
    assert abs(r["wder"] - (1 - n["A"] / sum(n.values()))) < 1e-9
    assert r["count_error"] == -2
    # greedy DI-cpWER can stop in a local optimum, so only "close to 0"
    assert r["dicpwer"] < 0.1 and r["cpwer"] > 0.5 and r["orcwer"] == 0


def test_swapped_turn_counts_near_change():
    ref = meeting()
    hyp = [dict(s) for s in meeting(names={"A": "h0", "B": "h1", "C": "h2"})]
    hyp[3]["speaker"] = "h1"  # C's turn given to B
    r = run(ref, hyp)
    assert 0 < r["wder"] < 0.3
    assert r["wder_near"] >= r["wder"]
    assert r["attribution_loss"] > 0


def test_normaliser_drops_fillers_and_punctuation():
    assert normalize_token("Um,") == ()
    assert normalize_token("Budget.") == ("budget",)
    assert normalize_token("OK") == ("okay",)


def test_multilingual_normaliser():
    assert normalize_text("Euh, j'ai vu ÇA !", "fr") == ["j", "ai", "vu", "ça"]
    # Mandarin per character, a Latin run stays one unit, 嗯 is dropped
    assert normalize_text("嗯，我们讨论AI。", "zh-CN") == ["我", "们", "讨", "论", "ai"]
    # Devanagari vowel signs stay inside the word; the danda goes
    assert normalize_text("आप कैसे हैं।", "hi") == ["आप", "कैसे", "हैं"]
    assert normalize_text("Okay.", None) == normalize_text("Okay.", "en-US") == ["okay"]


def test_mandarin_identical_text_scores_zero():
    ref = [{"session_id": "x", "speaker": "A", "start_time": 0.0, "end_time": 2.0, "words": "我们今天开会。"},
           {"session_id": "x", "speaker": "B", "start_time": 2.5, "end_time": 4.0, "words": "好的，没问题"}]
    hyp = [dict(r, speaker={"A": "s0", "B": "s1"}[r["speaker"]]) for r in ref]
    r = M.meeting_metrics([dict(x) for x in ref], seglst_to_rttm(ref, "x"), [dict(h) for h in hyp], words_of(hyp),
                          seglst_to_rttm(hyp, "x"), None, "x", None, "zh-CN")
    assert r["cpwer"] == 0 and r["wder"] == 0 and r["ref_words"] == 11


def test_dropped_turn_is_one_gap_and_deletions():
    ref = meeting()
    hyp = [dict(h) for i, h in enumerate(meeting()) if i != 3]  # turn 4 (C, 11 words, 4.4 s) missing
    r = run(ref, hyp)
    assert r["gaps"] == 1 and r["gaps_5s"] == 1  # 4.4 s turn plus the 0.6 s pauses on both sides
    assert r["gap_ref_words"] == 11 and r["longest_gap_s"] >= 5.0
    assert (r["w_del"], r["w_ins"], r["w_sub"]) == (11, 0, 0)
    assert abs(r["wer"] - 11 / r["ref_words"]) < 1e-9
    assert run(ref, meeting())["gaps"] == 0


if __name__ == "__main__":
    for name, fn in list(globals().items()):
        if name.startswith("test_"):
            fn()
            print("ok", name)
