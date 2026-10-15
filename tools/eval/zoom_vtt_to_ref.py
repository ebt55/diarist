"""Zoom transcript.vtt -> reference SegLST (JSONL) + RTTM + speaker map.

Zoom attributes each cue to the participant whose own audio stream carried
it ("Name: text"), so speaker identity per cue is close to ground truth.
Times are cue-level: a cue is a phrase, not a word. Word times inside a cue
are interpolated (in proportion to word length) by whoever needs them; every
SegLST row says so in its "timing" field and the sidecar repeats it.

Outputs, for --out-dir D and --session-id S:
  D/S.ref.seglst.jsonl   one cue per line: session_id, speaker, start_time,
                         end_time, words (raw cue text), timing
  D/S.ref.rttm           one SPEAKER line per cue
  D/S.ref.speakers.json  Zoom label -> speaker id map, counts, timing note

The speaker map holds participant names, so keep D outside the repo
(tools/eval/.work is gitignored).

Usage:
  uv run python -I zoom_vtt_to_ref.py <transcript.vtt> --session-id aug2 --out-dir .work/refs
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import CUE_TIMING_NOTE, RttmRow, write_jsonl, write_rttm  # noqa: E402

_TIME = re.compile(
    r"^\s*(?:(\d+):)?(\d{1,2}):(\d{2})[.,](\d{3})\s*-->\s*(?:(\d+):)?(\d{1,2}):(\d{2})[.,](\d{3})"
)
# "Name: text" or "Name - Org: text". A label is short and has no sentence
# punctuation; anything else is treated as an unlabelled cue.
_LABEL = re.compile(r"^(?P<label>[^:?!.\n]{1,80}?):\s+(?P<text>.+)$", re.S)


def _secs(h, m, s, ms) -> float:
    return int(h or 0) * 3600 + int(m) * 60 + int(s) + int(ms) / 1000.0


def parse_vtt(path: Path) -> list[dict]:
    """Returns cues: {start, end, label or None, text}."""
    raw = path.read_text(encoding="utf-8-sig").replace("\r\n", "\n").replace("\r", "\n")
    cues = []
    for block in re.split(r"\n\s*\n", raw):
        lines = [ln for ln in block.split("\n") if ln.strip()]
        for i, ln in enumerate(lines):
            m = _TIME.match(ln)
            if not m:
                continue
            g = m.groups()
            start, end = _secs(*g[0:4]), _secs(*g[4:8])
            text = " ".join(x.strip() for x in lines[i + 1 :]).strip()
            if not text:
                break
            lm = _LABEL.match(text)
            if lm:
                cues.append({"start": start, "end": end, "label": lm["label"].strip(), "text": lm["text"].strip()})
            else:
                cues.append({"start": start, "end": end, "label": None, "text": text})
            break
    cues.sort(key=lambda c: (c["start"], c["end"]))
    return cues


def convert(vtt: Path, session_id: str, out_dir: Path) -> dict:
    cues = parse_vtt(vtt)
    labels: dict[str, str] = {}
    unlabelled = 0
    segs = []
    prev_spk = None
    for c in cues:
        label = c["label"]
        if label is None:
            # Zoom leaves the name off when it could not attribute the cue;
            # keep it with the previous speaker and count it.
            unlabelled += 1
            spk = prev_spk or "S0"
        else:
            if label not in labels:
                labels[label] = f"S{len(labels) + 1}"
            spk = labels[label]
        prev_spk = spk
        segs.append(
            {
                "session_id": session_id,
                "speaker": spk,
                "start_time": round(c["start"], 3),
                "end_time": round(max(c["end"], c["start"]), 3),
                "words": c["text"],
                "timing": "cue-interpolated",
            }
        )
    out_dir.mkdir(parents=True, exist_ok=True)
    write_jsonl(out_dir / f"{session_id}.ref.seglst.jsonl", segs)
    write_rttm(
        out_dir / f"{session_id}.ref.rttm",
        [RttmRow(session_id, s["start_time"], s["end_time"] - s["start_time"], s["speaker"]) for s in segs],
    )
    changes = sum(1 for a, b in zip(segs, segs[1:]) if a["speaker"] != b["speaker"])
    meta = {
        "session_id": session_id,
        "source": str(vtt),
        "timing": CUE_TIMING_NOTE,
        "labels": labels,
        "speakers": len(labels),
        "cues": len(segs),
        "unlabelled_cues_kept_with_previous_speaker": unlabelled,
        "speaker_changes": changes,
        "words": sum(len(s["words"].split()) for s in segs),
    }
    (out_dir / f"{session_id}.ref.speakers.json").write_text(
        json.dumps(meta, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    return meta


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("vtt", type=Path)
    ap.add_argument("--session-id", required=True)
    ap.add_argument("--out-dir", type=Path, required=True)
    a = ap.parse_args()
    meta = convert(a.vtt, a.session_id, a.out_dir)
    # Counts only: names stay in the sidecar file.
    print(
        f"{a.session_id}: {meta['cues']} cues, {meta['speakers']} speakers, "
        f"{meta['speaker_changes']} changes, {meta['words']} words, "
        f"{meta['unlabelled_cues_kept_with_previous_speaker']} unlabelled cues"
    )


if __name__ == "__main__":
    main()
