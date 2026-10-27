"""Fetch the four EN2002 meetings of the AMI test set (CC BY 4.0) in two
channels and build their references: set "ami" of docs/05 "Extend".

EN2002a to EN2002d are the non-scenario Edinburgh meetings of the AMI
Full-corpus-ASR test partition: unscripted discussions, 3 or 4 speakers,
24 to 30 percent of the speech overlapped, 150 minutes in all. Each meeting
is fetched twice, so the same words can be scored on two kinds of audio:
  Mix-Headset  the sum of the headset microphones   (group pub-meeting)
  Array1-01    microphone 1 of the table-top array  (group pub-far)

References: the word-level manual annotation (ami_public_manual_1.6.2.zip,
with each word's punctuation and casing in `words_pc`) and the pyannote
AMI-diarization-setup only_words test RTTM at a pinned commit (references
derived from the same manual annotation, words only, adjacent words of a
speaker merged), as for ES2002a.

  uv run python -I ami_test_fetch.py                  # about 0.6 GB
  uv run python -I ami_test_fetch.py --meetings EN2002a

Output: .work/public/ami/ (audio/, refs/, sessions.toml, meta.json); the
manual-annotation zip is read from data/samples/ami/ when ami_fetch.py put
it there, else fetched into download/. Run with an isolated interpreter.
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import public_data as P  # noqa: E402
from ami_fetch import reference_rows  # noqa: E402
from common import REPO, read_rttm, read_seglst  # noqa: E402

SET = "ami"
MEETINGS = ["EN2002a", "EN2002b", "EN2002c", "EN2002d"]
CHANNELS = {
    "mix": ("Mix-Headset", "pub-meeting"),
    "arr1": ("Array1-01", "pub-far"),
}
AUDIO_URL = "https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/{m}/audio/{m}.{ch}.wav"
PYANNOTE_REV = "67c2d539286e89f68952d5dcf83912bd9f01dfae"
RTTM_URL = f"https://raw.githubusercontent.com/pyannote/AMI-diarization-setup/{PYANNOTE_REV}/only_words/rttms/test/{{m}}.rttm"
MANUAL_URL = "https://groups.inf.ed.ac.uk/ami/AMICorpusAnnotations/ami_public_manual_1.6.2.zip"
MANUAL = "ami_public_manual_1.6.2.zip"

# SHA-256 of every input, recorded at the first fetch (26 Oct 2026).
PINS: dict[str, str | None] = {
    MANUAL: "b56e5babb2496b8795deeeda7e71178d7fbc9963f94276cf2a3f4b56ebbc9f9d",
    "EN2002a.Mix-Headset.wav": "a15017d0d3c871866971ee178052a0ce3a7e44d6fbf89fc23dcab326511b65f5",
    "EN2002b.Mix-Headset.wav": "50e2c8e4737850c4bf3cc24e7087c755e1c733cd91822106afd0df485ba64893",
    "EN2002c.Mix-Headset.wav": "a057708b3adb553f709fcde97d2af8e0455076bc3f88c250975227433d6e0270",
    "EN2002d.Mix-Headset.wav": "6aa99d9d71cb695ea699d6be277f78eff68e847c93560031486eba9f18164ebe",
    "EN2002a.Array1-01.wav": "6c0644431679e59adc70129f2ba6c2fb737fa731cf1ffe8a033cd7f0c441170e",
    "EN2002b.Array1-01.wav": "e3650cc84250caa0992ac0398f4a3127a8c14ed2c93edc10535245335cc41077",
    "EN2002c.Array1-01.wav": "5e8b41c7ab6f7dca07c52c354ece85e31a71491a47ef0493407cb89b12a41f60",
    "EN2002d.Array1-01.wav": "7a131dcaaf871a32a0af9ac0c9a21acbf78ed4f2e343d29969a7f4551c966723",
    "EN2002a.rttm": "a93919ae5f18e476614b7e5a2713c4dc6bba23e95149fba18dbe68602b13ee40",
    "EN2002b.rttm": "c4e6ef3f9f493d82d14664a9669479aca378de05639e4640f02f8398e2ef714a",
    "EN2002c.rttm": "e595fa5953a923cbd62ffc881bd9725df98a6587bc14730dd279937773953bca",
    "EN2002d.rttm": "f11eeb70b0e1721b5b4b831a891d2b70c0b1a6ad368d0495d73c998335148463",
}

LICENCE = (
    "CC BY 4.0. AMI site: \"The AMI corpus and its annotations are released under the Creative Commons "
    "Attribution 4.0 license agreement (also called CC BY 4.0).\" RTTMs: pyannote/AMI-diarization-setup, Apache-2.0, "
    "derived from the same manual annotation."
)


def manual_zip(root: Path, allow_unpinned: bool) -> Path:
    for p in (REPO / "data" / "samples" / MANUAL, REPO / "data" / "samples" / "ami" / MANUAL):
        if p.is_file() and p.stat().st_size > 0:
            return P.fetch(MANUAL_URL, p, PINS[MANUAL], allow_unpinned=allow_unpinned)
    return P.fetch(MANUAL_URL, root / "download" / MANUAL, PINS[MANUAL], size=23_000_000, allow_unpinned=allow_unpinned)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--meetings", default=",".join(MEETINGS))
    P.common_args(ap)
    a = ap.parse_args()
    meetings = [m.strip() for m in a.meetings.split(",") if m.strip()]
    root = P.set_dir(SET)
    zip_path = manual_zip(root, a.allow_unpinned)
    sessions, inputs, stats = [], {MANUAL: P.sha256_file(zip_path)}, {}
    for m in meetings:
        rttm_src = P.fetch(RTTM_URL.format(m=m), root / "download" / f"{m}.rttm", PINS.get(f"{m}.rttm"),
                           size=50_000, allow_unpinned=a.allow_unpinned)
        inputs[f"{m}.rttm"] = P.sha256_file(rttm_src)
        seg = root / "refs" / f"{m}.ref.seglst.jsonl"
        rt = root / "refs" / f"{m}.ref.rttm"
        if a.rebuild or not (seg.is_file() and rt.is_file()):
            rows = reference_rows(zip_path, m, m)
            seg, rt = P.write_refs(root, m, rows, read_rttm(rttm_src))
            print(f"[{m}] {len(rows)} words, {sum(1 for r in rows if r['words_pc'] != r['words'])} with punctuation")
        stats[m] = P.ref_stats(read_seglst(seg), read_rttm(rt))
        for key, (ch, group) in CHANNELS.items():
            name = f"{m}.{ch}.wav"
            wav = P.fetch(AUDIO_URL.format(m=m, ch=ch), root / "audio" / name, PINS.get(name),
                          size=70_000_000, allow_unpinned=a.allow_unpinned)
            inputs[name] = P.sha256_file(wav)
            sessions.append({
                "id": f"ami-{m.lower()}-{key}", "label": f"AMI {m} {ch}", "group": group, "set": SET,
                "language": "en", "ref_source": "ami-word", "audio": wav, "ref_rttm": rt, "ref_seglst": seg,
                "duration_s": round(P.wav_duration(wav), 1),
            })
    P.write_sessions(root, sessions)
    P.write_meta(root, {
        "set": SET,
        "content": "AMI Full-corpus-ASR test meetings EN2002a-d (non-scenario), Mix-Headset and Array1-01",
        "licence": LICENCE,
        "sources": {"audio": AUDIO_URL, "rttm": RTTM_URL, "manual": MANUAL_URL},
        "sha256": inputs,
        "references": stats,
        "punctuation_casing": True,
    })
    hours = sum(s["duration_s"] for s in sessions) / 3600
    print(f"{len(sessions)} sessions, {hours:.2f} h of audio in {root}")


if __name__ == "__main__":
    main()
