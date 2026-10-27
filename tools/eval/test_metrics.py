"""Sanity checks for metrics.py on synthetic meetings, and for levers.py's loop rule
against crates/core/src/transcript/loops.rs (no GPU, no private data).

  uv run python -I -m pytest test_metrics.py   (or: uv run python -I test_metrics.py)
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import levers as L  # noqa: E402
import metrics as M  # noqa: E402
from common import RttmRow, normalize_token, seglst_to_rttm  # noqa: E402

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


# crates/core/src/transcript/loops.rs `NORM_PAIRS`: the Rust `norm` of each
# text (combining vowel signs kept, tone marks, viramas and control
# characters dropped, Rust's white space trimmed).
NORM_PAIRS = [
    ("नहीं", "नहीं"),
    ("ที่", "ที"),
    ("தமிழ்", "தமிழ"),
    ("Ко-ко,", "коко"),
    ("a\x1cb", "ab"),
    ("İ", "i"),
    ("Ⅻ", "ⅻ"),
    (" ok\xa0", "ok"),
    ("đâu?", "đâu"),
]


def test_loop_norm_is_the_rust_norm():
    for text, want in NORM_PAIRS:
        assert L.loop_norm(text) == want, (text, L.loop_norm(text), want)
    # Devanagari is collapsible (a script with spaces), Thai never.
    assert L.loop_eligible(L.loop_norm("नहीं"))
    assert not L.loop_eligible(L.loop_norm("ที่"))
    # Digits only, in any script, are never collapsed.
    assert not L.loop_eligible(L.loop_norm("٣٣"))


def _words(starts, text="no"):
    return [{"start": t, "end": t + 80, "text": text, "norm": L.loop_norm(text), "speaker": "spk_0", "seg": None}
            for t in starts]


def test_loop_rule_matches_loops_rs():
    # crates/core/src/transcript/loops.rs `real_repeats_after_a_burst_stay`
    # and `a_train_of_bursts_keeps_one_word_per_burst`.
    w = _words([0] * 5 + [300, 600, 900])
    assert L.loop_removed(w, rule="plan") == 7
    assert L.loop_removed(w) == 4
    assert L.loop_removed(_words([0, 0, 0, 250], "I")) == 2
    assert L.loop_removed(_words([0, 80, 161, 241], "and")) == 0
    assert L.loop_removed(_words([0, 80, 161, 241], "and"), rule="plan") == 3
    assert L.loop_removed(_words([1000 + (k // 8) * 160 for k in range(24)], "the")) == 21
    # Never more than the plan's rule.
    w = _words([0, 0, 0, 0, 300, 600, 900, 1200, 1500])
    assert L.loop_removed(w) == L.loop_removed(w, rule="plan") == 0


if __name__ == "__main__":
    for name, fn in list(globals().items()):
        if name.startswith("test_"):
            fn()
            print("ok", name)
