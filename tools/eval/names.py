"""Spike L (docs/plans/18 section 3.6, plan 16 section 12.4): word boosting for
names, scored for its kill rule. Never touches the GPU; the runs come from
nemo_direct.py. Numbers only: on a private session no name or text is printed.

  uv run python -I names.py pick --runs-dir <.work/runs of a cached EN run> --config rs_en_shipped \
      --write-phrases .work/spikes/names/meeting.phrases.txt
  uv run python -I names.py score --clip --phrases Craig beagle "Planet of the Apes" \
      --base .work/spikes/names/clip_base --runs .work/spikes/names/clip_b1 ...
  uv run python -I names.py score --session <id> --phrases-file .work/spikes/names/meeting.phrases.txt \
      --base .work/spikes/names/mtg_base --runs .work/spikes/names/mtg_b1 ...

pick: over the owner's Zoom meetings (every kind = "zoom" session outside the
  `talks` group), the reference occurrences of the meeting's speaker names and
  how many of them a cached run gets wrong. A speaker name is a token of a
  Zoom participant label (the names Diarist shows once the speakers are named
  after the participants; of a "Name - Org" label, the name part), normalised
  as the harness normalises words, of 3
  letters or more, without digits. An occurrence is the token itself or its
  possessive ("'s"). A reference occurrence is wrong when the Levenshtein
  alignment of the time-ordered normalised words (metrics.py's WER alignment)
  does not pair it with an equal hypothesis word. The meeting with the most
  wrong occurrences is picked (ties: more occurrences, then sessions-file
  order); --write-phrases writes its names, one per line, to a private file.

score: per nemo_direct run directory (words.json) against the reference, with
  the boosted phrases: speaker-agnostic WER (the clip: levers.py boost's clip
  WER), edit operations against the base run, and per phrase the reference
  occurrences, those the base run gets wrong (the targets), those the run gets
  right (fixed: a target the run gets right; broken: right in the base, wrong
  in the run), and the run's insertions: occurrences of the phrase in the
  hypothesis that the alignment does not pair with a reference occurrence,
  also per hour of audio. --clip reads AMI ES2002a 50 to 350 s, as research/12
  section 4 and levers.py boost do.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import EVAL_DIR, WORK, interpolate_words, load_sessions, normalize_token, read_seglst  # noqa: E402
import metrics as M  # noqa: E402


def name_tokens(labels: list[str], cased: bool = False) -> list[str]:
    """The normalised name tokens of the labels; cased=True gives each as the
    label spells it (the form passed to --speech-context)."""
    out: list[str] = []
    seen: set[str] = set()
    for lab in labels:
        # Zoom's "Name - Org" label form (zoom_vtt_to_ref.py): the name part only.
        for raw in re.split(r"[\s\-_/(),.]+", lab.split(" - ")[0]):
            for t in normalize_token(raw):
                if len(t) >= 3 and t.isalpha() and t not in seen:
                    seen.add(t)
                    out.append(raw if cased and normalize_token(raw) == (t,) else t)
    return out


def is_occ(tok: str, name: str) -> bool:
    return tok == name or tok == name + "'s"


def align(ref: list[str], hyp: list[str]) -> dict[int, int]:
    """Reference index -> hypothesis index for the aligned pairs (equal or
    substituted, position by position inside a block) of the Levenshtein
    alignment, as metrics.wder pairs them."""
    from rapidfuzz.distance import Levenshtein

    pairs: dict[int, int] = {}
    for op in Levenshtein.opcodes(ref, hyp):
        if op.tag in ("equal", "replace"):
            for k in range(min(op.src_end - op.src_start, op.dest_end - op.dest_start)):
                pairs[op.src_start + k] = op.dest_start + k
    return pairs


def phrase_spans(toks: list[str], ph: tuple[str, ...]) -> list[tuple[int, int]]:
    n = len(ph)
    if n == 1:
        return [(i, i + 1) for i, t in enumerate(toks) if is_occ(t, ph[0])]
    return [(i, i + n) for i in range(len(toks) - n + 1) if tuple(toks[i:i + n]) == ph]


def occurrences(ref: list[str], hyp: list[str], ph: tuple[str, ...]) -> dict:
    """Reference occurrences right / wrong and hypothesis insertions of one
    phrase. An occurrence is right when every one of its words is paired with
    the same word (a name: the name or its possessive); a hypothesis
    occurrence not paired that way with a reference occurrence is an
    insertion."""
    pairs = align(ref, hyp)

    def same(i: int, k: int) -> bool:
        j = pairs.get(i)
        if j is None:
            return False
        return is_occ(hyp[j], ph[0]) if len(ph) == 1 else hyp[j] == ph[k]

    right = []
    paired_hyp: set[int] = set()
    for a, b in phrase_spans(ref, ph):
        ok = all(same(i, i - a) for i in range(a, b))
        right.append(ok)
        if ok:
            paired_hyp.update(pairs[i] for i in range(a, b))
    hyp_spans = phrase_spans(hyp, ph)
    ins = sum(1 for a, b in hyp_spans if not all(j in paired_hyp for j in range(a, b)))
    return {"right": right, "hyp": len(hyp_spans), "ins": ins}


def ref_hyp_session(sid: str, words_json: Path | None = None, transcript: Path | None = None) -> tuple[list[str], list[str]]:
    import run as R

    s = load_sessions(EVAL_DIR / "sessions.toml")[sid]
    ref_segs, _, _ = R.load_reference(s)
    ref = [w.word for w in interpolate_words(ref_segs)]
    if transcript is not None:
        _, hyp_words = M.hyp_from_export(json.loads(transcript.read_text(encoding="utf-8")), sid)
    else:
        ws = json.loads(words_json.read_text(encoding="utf-8")).get("words", [])
        hyp_words = [{"speaker": str(w.get("speaker", 0)), "start_time": float(w["start"]),
                      "end_time": max(float(w["end"]), float(w["start"])), "words": w["word"]} for w in ws]
    hyp = [w.word for w in interpolate_words(hyp_words)]
    return ref, hyp


def clip_ref(offset: float, dur: float) -> list[str]:
    s = load_sessions(EVAL_DIR / "sessions.toml")["ami"]
    rows = [r for r in read_seglst(s.ref_seglst) if offset <= float(r["start_time"]) <= offset + dur]
    return [t for r in sorted(rows, key=lambda r: float(r["start_time"])) for t in normalize_token(r["words"])]


def clip_hyp(words_json: Path) -> list[str]:
    ws = json.loads(words_json.read_text(encoding="utf-8")).get("words", [])
    return [t for w in ws for t in normalize_token(w["word"])]


def cmd_pick(a) -> None:
    import zoom_vtt_to_ref

    sessions = load_sessions(EVAL_DIR / "sessions.toml")
    rows = []
    for order, (sid, s) in enumerate(sessions.items()):
        if s.kind != "zoom" or s.group == "talks":
            continue
        meta_p = WORK / "refs" / f"{sid}.ref.speakers.json"
        if not meta_p.is_file():
            zoom_vtt_to_ref.convert(s.vtt, sid, WORK / "refs")
        labels = list(json.loads(meta_p.read_text(encoding="utf-8"))["labels"])
        names = name_tokens(labels)
        tr = Path(a.runs_dir) / a.config / sid / "transcript.json"
        if not tr.is_file():
            print(f"| {s.label} | no cached run |")
            continue
        ref, hyp = ref_hyp_session(sid, transcript=tr)
        occ = wrong = 0
        for n in names:
            o = occurrences(ref, hyp, (n,))
            occ += len(o["right"])
            wrong += sum(1 for r in o["right"] if not r)
        rows.append((wrong, occ, -order, sid, s.label, len(labels), len(names)))
    rows.sort(reverse=True)
    print("| Session | Zoom labels | Name tokens | Reference occurrences | Wrong in the cached run |")
    print("|---|---|---|---|---|")
    for wrong, occ, _, sid, label, nl, nn in rows:
        print(f"| {label} | {nl} | {nn} | {occ} | {wrong} |")
    best = rows[0]
    print(f"\npicked: {best[4]} ({best[3]})")
    if a.write_phrases:
        labels = list(json.loads((WORK / "refs" / f"{best[3]}.ref.speakers.json").read_text(encoding="utf-8"))["labels"])
        p = Path(a.write_phrases)
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text("\n".join(name_tokens(labels, cased=True)) + "\n", encoding="utf-8")
        print(f"phrases: {len(name_tokens(labels))} written to the private file")


def cmd_score(a) -> None:
    from rapidfuzz.distance import Levenshtein

    if a.phrases_file:
        phrases = [ln.strip() for ln in Path(a.phrases_file).read_text(encoding="utf-8").splitlines() if ln.strip()]
        private = True
    else:
        phrases, private = a.phrases, False
    phs = [tuple(t for w in p.split() for t in normalize_token(w)) for p in phrases]
    runs = [Path(a.base), *map(Path, a.runs)]
    if a.clip:
        ref = clip_ref(a.offset, a.dur)
        hyps = {d: clip_hyp(d / "words.json") for d in runs}
        hours = a.dur / 3600
    else:
        ref = None
        hyps = {}
        for d in runs:
            r, h = ref_hyp_session(a.session, words_json=d / "words.json")
            ref = ref or r
            hyps[d] = h
        dur = json.loads((Path(a.base) / "words.json").read_text(encoding="utf-8")).get("duration")
        hours = float(dur) / 3600
    base = hyps[Path(a.base)]
    base_occ = [occurrences(ref, base, ph) for ph in phs]
    label = (lambda i: f"P{i + 1}") if private else (lambda i: phrases[i])
    out = []
    for d in runs:
        h = hyps[d]
        occ = [occurrences(ref, h, ph) for ph in phs]
        targets = fixed = broken = 0
        per = []
        for i, (b, o) in enumerate(zip(base_occ, occ)):
            t = sum(1 for x in b["right"] if not x)
            f = sum(1 for x, y in zip(b["right"], o["right"]) if not x and y)
            br = sum(1 for x, y in zip(b["right"], o["right"]) if x and not y)
            targets, fixed, broken = targets + t, fixed + f, broken + br
            per.append({"phrase": label(i), "ref": len(o["right"]), "right": sum(o["right"]), "targets": t,
                        "fixed": f, "broken": br, "ins": o["ins"], "ins_base": b["ins"],
                        "ins_per_h": round(o["ins"] / hours, 2)})
        err = (d / "stderr.txt").read_text(encoding="utf-8", errors="replace") if (d / "stderr.txt").is_file() else ""
        msgs = sorted({ln.strip() for ln in err.splitlines() if "boost" in ln.lower() and " -> " not in ln})
        out.append({
            "run": d.name, "words": len(h), "wer": round(100 * Levenshtein.distance(ref, h) / len(ref), 2),
            "edits_vs_base": Levenshtein.distance(base, h), "targets": targets, "fixed": fixed, "broken": broken,
            "ins": sum(p["ins"] for p in per), "max_ins_per_h": max((p["ins_per_h"] for p in per), default=0.0),
            "phrases": per, "runtime_boost_messages": msgs,
        })
    print(json.dumps({"hours": round(hours, 4), "ref_words": len(ref), "runs": out}, indent=1))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("pick")
    p.add_argument("--runs-dir", required=True)
    p.add_argument("--config", default="rs_en_shipped")
    p.add_argument("--write-phrases")
    p = sub.add_parser("score")
    p.add_argument("--base", required=True)
    p.add_argument("--runs", nargs="*", default=[])
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("--clip", action="store_true")
    g.add_argument("--session")
    p.add_argument("--phrases", nargs="*")
    p.add_argument("--phrases-file")
    p.add_argument("--offset", type=float, default=50.0)
    p.add_argument("--dur", type=float, default=300.0)
    a = ap.parse_args()
    {"pick": cmd_pick, "score": cmd_score}[a.cmd](a)


if __name__ == "__main__":
    main()
