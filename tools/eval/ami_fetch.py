"""Fetch AMI ES2002a (CC-BY-4.0) and build its word-level reference.

Downloads only what is missing:
  - ES2002a.Mix-Headset.wav (16 kHz mono) from the AMI corpus mirror
  - ES2002a.rttm from pyannote/AMI-diarization-setup (only_words, train split)
  - ami_public_manual_1.6.2.zip (word-level manual annotations, about 23 MB)

An existing WAV or RTTM in data/samples/ (where earlier phases put them) is
used as is; new downloads go to data/samples/ami/. The word reference is
written to data/samples/ami/ES2002a.ref.seglst.jsonl: one word per line with
its manual start/end time and the participant's global id (FEE005, ...), the
same ids the RTTM uses.

The AMI server fails the Windows schannel revocation check, so curl runs with
--ssl-revoke-best-effort (research/08 pitfall 9).

Run with an isolated interpreter (the zip is downloaded data):
  uv run python -I ami_fetch.py
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import REPO, write_jsonl  # noqa: E402

MEETING = "ES2002a"
SAMPLES = REPO / "data" / "samples"
AMI_DIR = SAMPLES / "ami"
URLS = {
    "wav": f"https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/{MEETING}/audio/{MEETING}.Mix-Headset.wav",
    "rttm": f"https://raw.githubusercontent.com/pyannote/AMI-diarization-setup/main/only_words/rttms/train/{MEETING}.rttm",
    "manual": "https://groups.inf.ed.ac.uk/ami/AMICorpusAnnotations/ami_public_manual_1.6.2.zip",
}
NAMES = {
    "wav": f"{MEETING}.Mix-Headset.wav",
    "rttm": f"{MEETING}.rttm",
    "manual": "ami_public_manual_1.6.2.zip",
}


def curl(url: str, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_suffix(dest.suffix + ".part")
    cmd = ["curl", "-L", "--fail", "--retry", "3", "-o", str(part), url]
    if os.name == "nt":
        cmd.insert(1, "--ssl-revoke-best-effort")
    print(f"downloading {url}", file=sys.stderr)
    subprocess.run(cmd, check=True)
    part.replace(dest)


def ensure(kind: str) -> Path:
    for d in (SAMPLES, AMI_DIR):
        p = d / NAMES[kind]
        if p.is_file() and p.stat().st_size > 0:
            return p
    dest = AMI_DIR / NAMES[kind]
    curl(URLS[kind], dest)
    return dest


def reference_rows(zip_path: Path, meeting: str = MEETING, session_id: str = "ami") -> list[dict]:
    """One row per manually transcribed word of `meeting`, speaker = the
    participant's global id. `words_pc` is the word with the punctuation
    tokens that follow it attached (the manual transcript's own casing and
    punctuation, docs/research/18)."""
    with zipfile.ZipFile(zip_path) as z:
        names = z.namelist()
        agents: dict[str, str] = {}
        meetings = next((n for n in names if n.endswith("corpusResources/meetings.xml")), None)
        if meetings:
            root = ET.fromstring(z.read(meetings))
            for m in root.iter():
                if m.tag.endswith("meeting") and m.get("observation") == meeting:
                    for s in m:
                        if s.tag.endswith("speaker") and s.get("nxt_agent"):
                            agents[s.get("nxt_agent")] = s.get("global_name") or s.get("nxt_agent")
        rows = []
        for n in sorted(names):
            base = n.rsplit("/", 1)[-1]
            if not (base.startswith(meeting + ".") and base.endswith(".words.xml")):
                continue
            letter = base.split(".")[1]
            spk = agents.get(letter, letter)
            root = ET.fromstring(z.read(n))
            last = None
            for el in root.iter():
                if not el.tag.endswith("w"):
                    continue
                text = (el.text or "").strip()
                if el.get("punc") == "true":
                    if last is not None and text:
                        last["words_pc"] += text
                    continue
                st, en = el.get("starttime"), el.get("endtime")
                if not text or st is None or en is None:
                    last = None
                    continue
                last = {
                    "session_id": session_id,
                    "speaker": spk,
                    "start_time": round(float(st), 3),
                    "end_time": round(max(float(en), float(st)), 3),
                    "words": text,
                    "timing": "word",
                    "words_pc": text,
                }
                rows.append(last)
    rows.sort(key=lambda r: (r["start_time"], r["end_time"]))
    return rows


def build_reference(zip_path: Path, out: Path) -> int:
    rows = reference_rows(zip_path)
    write_jsonl(out, rows)
    return len(rows)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rebuild", action="store_true", help="rebuild the word reference even if present")
    a = ap.parse_args()
    wav, rttm = ensure("wav"), ensure("rttm")
    out = AMI_DIR / f"{MEETING}.ref.seglst.jsonl"
    if a.rebuild or not out.is_file():
        manual = ensure("manual")
        n = build_reference(manual, out)
        print(f"wrote {out} ({n} words)")
    print(f"wav  {wav}\nrttm {rttm}\nref  {out}")


if __name__ == "__main__":
    main()
