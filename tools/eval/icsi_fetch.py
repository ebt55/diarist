"""Fetch four ICSI meetings (CC BY 4.0) and build their references: set
"icsi" of docs/05 "Extend" (group pub-meeting).

ICSI meetings are natural weekly research-group meetings recorded at ICSI
Berkeley in 2000 to 2002. The four here, Bed004, Bed016, Bmr019 and
Bro018, were picked by participant count and overlap: 7, 10, 9 and 5
participants (two above the 8 speakers the Nemotron 3 diarizer can hold),
9 to 33 percent of the speech overlapped, one meeting from each of the
three main series, 218 minutes in all. Audio: the corpus's
"Headset mix" (`<meeting>.interaction.wav`), the sum of the close-talking
microphones, converted to 16 kHz mono.

References: the ICSI core NXT annotation (ICSI_core_NXT.zip, 22 Jul 2016):
one row per transcribed word with its forced-alignment time; words without
a time (about a tenth, many in the read-digits part at the end of a
meeting) are spread over their segment (timing "interp"). Clitics (`'s`,
`'m`) join the word before them; punctuation (`.` `?` `,`) and the
transcript's casing go to `words_pc`. The RTTM is derived from the word
rows the way the AMI only_words references are: adjacent words of one
speaker merged, any pause splits.

  uv run python -I icsi_fetch.py                 # about 0.5 GB
  uv run python -I icsi_fetch.py --meetings Bro018

Output: .work/public/icsi/. Run with an isolated interpreter.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import public_data as P  # noqa: E402
from common import read_rttm, read_seglst  # noqa: E402

SET = "icsi"
MEETINGS = ["Bed004", "Bed016", "Bmr019", "Bro018"]
AUDIO_URL = "https://groups.inf.ed.ac.uk/ami/ICSIsignals/NXT/{m}.interaction.wav"
NXT_URL = "https://groups.inf.ed.ac.uk/ami/ICSICorpusAnnotations/ICSI_core_NXT.zip"
NXT = "ICSI_core_NXT.zip"
PINS: dict[str, str | None] = {
    NXT: "cf4860245b9ca9c9ed11a66e5a74cfea2a99a686a52f7580aa0a6bc2225895a9",
    "Bed004.interaction.wav": "dbefcab8226c70f50d7d7a83d3253c8505e035f97f78e580afe45267c51e7e22",
    "Bed016.interaction.wav": "f5e0ffde1b059e6ba08191617059667a49dd1b9e6f4a5e997f356d07fd1b8729",
    "Bmr019.interaction.wav": "f45aba45583f132960c577fc2c26e7a6e486467320a0f95db3cd2b74d678b176",
    "Bro018.interaction.wav": "b0aea781dffaba27b3151372a8262a8c34e99c1fc700a76d9fdae4a5a6ac5e07",
}
LICENCE = (
    "CC BY 4.0. ICSI site: \"All of the signals and transcription, and some of the annotations, have been released "
    "publicly under the Creative Commons Attribution 4.0 International Licence (CC BY 4.0).\""
)
WORD = {"W", "LET", "ABBR", "TRUNCW"}
END_PUNCT = {".", "CM"}  # "." carries . ? ! as text; CM is a comma
_ID = re.compile(r"id\(([^)]+)\)")


def _segments(z: zipfile.ZipFile, name: str) -> tuple[str | None, list[tuple[float | None, float | None, str, str]]]:
    """(participant, [(start, end, first word id, last word id)]) of one channel."""
    root = ET.fromstring(z.read(name))
    out, who = [], None
    for el in root.iter():
        if not el.tag.endswith("segment"):
            continue
        who = who or el.get("participant")
        st, en = el.get("starttime"), el.get("endtime")
        for ch in el:
            ids = _ID.findall(ch.get("href", ""))
            if ids:
                out.append((float(st) if st else None, float(en) if en else None, ids[0], ids[-1]))
    return who, out


def channel_rows(z: zipfile.ZipFile, meeting: str, ch: str) -> list[dict]:
    who, segs = _segments(z, f"ICSI/Segments/{meeting}.{ch}.segs.xml")
    spk = who or f"{meeting}.{ch}"
    root = ET.fromstring(z.read(f"ICSI/Words/{meeting}.{ch}.words.xml"))
    els = [el for el in root if el.tag.split("}")[-1] == "w"]
    index = {el.get("{http://nite.sourceforge.net/}id"): i for i, el in enumerate(els)}
    seg_of: dict[int, tuple[float | None, float | None]] = {}
    for st, en, a, b in segs:
        if a in index and b in index:
            for i in range(index[a], index[b] + 1):
                seg_of[i] = (st, en)
    rows: list[dict] = []
    owner: list[int] = []  # element index of each row, for the segment lookup
    last: dict | None = None
    for i, el in enumerate(els):
        c, text = el.get("c"), (el.text or "").strip()
        if not text:
            continue
        st, en = el.get("starttime"), el.get("endtime")
        if not (st and en):
            st = en = None
        if c in WORD and not (text.startswith("'") and last is not None):
            last = {
                "speaker": spk,
                "start_time": round(float(st), 3) if st else None,
                "end_time": round(float(en), 3) if en else None,
                "words": text,
                "timing": "word",
                "words_pc": text,
            }
            rows.append(last)
            owner.append(i)
        elif last is not None and (c == "APOSS" or (c in WORD and text.startswith("'"))):
            last["words"] += text
            last["words_pc"] += text
            if en and last["start_time"] is not None:
                last["end_time"] = max(last["end_time"], round(float(en), 3))
        elif last is not None and (c in END_PUNCT or c == "HYPH"):
            last["words_pc"] += text
    # Words without their own time: spread inside their segment, between the
    # timed words around them.
    k = 0
    while k < len(rows):
        if rows[k]["start_time"] is not None:
            k += 1
            continue
        j = k
        while j < len(rows) and rows[j]["start_time"] is None and seg_of.get(owner[j]) == seg_of.get(owner[k]):
            j += 1
        s_st, s_en = seg_of.get(owner[k], (None, None))
        lo = rows[k - 1]["end_time"] if k > 0 and seg_of.get(owner[k - 1]) == seg_of.get(owner[k]) else s_st
        hi = rows[j]["start_time"] if j < len(rows) and seg_of.get(owner[j]) == seg_of.get(owner[k]) else s_en
        if lo is None or hi is None:
            lo = lo if lo is not None else (rows[k - 1]["end_time"] if k > 0 else 0.0)
            hi = hi if hi is not None else lo
        hi = max(hi, lo)
        n = j - k
        for m in range(n):
            rows[k + m].update(
                start_time=round(lo + (hi - lo) * m / n, 3), end_time=round(lo + (hi - lo) * (m + 1) / n, 3),
                timing="interp",
            )
        k = j
    return rows


def meeting_rows(zip_path: Path, meeting: str) -> list[dict]:
    with zipfile.ZipFile(zip_path) as z:
        chans = sorted(
            m.group(1) for n in z.namelist() if (m := re.match(rf"ICSI/Words/{meeting}\.(\w+)\.words\.xml$", n))
        )
        rows = []
        for ch in chans:
            rows += channel_rows(z, meeting, ch)
    return rows


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--meetings", default=",".join(MEETINGS))
    P.common_args(ap)
    a = ap.parse_args()
    root = P.set_dir(SET)
    zip_path = None
    sessions, inputs, stats = [], {NXT: PINS[NXT]}, {}
    for m in [x.strip() for x in a.meetings.split(",") if x.strip()]:
        name = f"{m}.interaction.wav"
        wav = root / "audio" / f"{m}.wav"
        if a.rebuild or not wav.is_file():
            src = P.fetch(AUDIO_URL.format(m=m), root / "download" / name, PINS.get(name), size=120_000_000,
                          allow_unpinned=a.allow_unpinned)
            inputs[name] = P.sha256_file(src)
            P.to_wav16k(src, wav)
        else:
            inputs[name] = PINS.get(name)
        seg, rt = root / "refs" / f"{m}.ref.seglst.jsonl", root / "refs" / f"{m}.ref.rttm"
        if a.rebuild or not (seg.is_file() and rt.is_file()):
            zip_path = zip_path or P.fetch(NXT_URL, root / "download" / NXT, PINS[NXT], size=19_500_000,
                                           allow_unpinned=a.allow_unpinned)
            rows = meeting_rows(zip_path, m)
            seg, rt = P.write_refs(root, m, rows, P.rows_to_rttm(rows, m, 0.0))
        stats[m] = P.ref_stats(read_seglst(seg), read_rttm(rt))
        print(f"[{m}] {stats[m]['words']} words, {stats[m]['speakers']} speakers, {stats[m]['interp_rows']} interpolated")
        sessions.append({
            "id": f"icsi-{m.lower()}", "label": f"ICSI {m}", "group": "pub-meeting", "set": SET, "language": "en",
            "ref_source": "icsi-word", "audio": wav, "ref_rttm": rt, "ref_seglst": seg,
            "duration_s": round(P.wav_duration(wav), 1),
        })
    P.write_sessions(root, sessions)
    P.write_meta(root, {
        "set": SET,
        "content": "ICSI Meeting Corpus test meetings, Headset mix, 16 kHz mono",
        "licence": LICENCE,
        "sources": {"audio": AUDIO_URL, "annotations": NXT_URL},
        "sha256": inputs,
        "references": stats,
        "punctuation_casing": True,
    })
    if a.prune:
        P.prune([root / "download"])
    print(f"{len(sessions)} sessions, {sum(s['duration_s'] for s in sessions) / 3600:.2f} h in {root}")


if __name__ == "__main__":
    main()
