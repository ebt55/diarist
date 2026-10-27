"""Fetch eight Earnings-22 calls (Rev, transcripts CC BY-SA 4.0) and build
their references: set "earnings22" of docs/05 "Extend" (group pub-phone).

Earnings-22 is 125 English earnings calls (119 hours) of companies from 27
countries, chosen for accented speech: an operator, the company's
executives and the analysts who call in, many of them over the phone.
Dense in names, numbers and finance terms. This slice is eight calls, 4.6
hours, one per region family (India, China, South Korea, Australia,
Brazil, Argentina, Russia, Canada), taken from the calls whose forced
alignment times at least 96.9 percent of the reference tokens, with 4 to 13
speakers each.

References: `transcripts/force_aligned_nlp_references/<id>.aligned.nlp` at
a pinned commit of revdotcom/speech-datasets: one row per token with its
speaker, aligned time, punctuation and casing (`words_pc` = the token with
its punctuation). Tokens the aligner left without a time are spread
between their timed neighbours ("interp"); markup such as <silence> is
dropped. The RTTM is derived from the word rows (adjacent words of one
speaker merged). Audio: `media/<id>.mp3` (Git LFS, pinned by its LFS
SHA-256), converted to 16 kHz mono.

Licence: the dataset README carries a CC BY-SA 4.0 badge; its LICENSE.md
names "the transcripts and associated text files" only and says nothing
about the audio, which Rev publishes in the same repository for
benchmarking. docs/research/20 records this as an open point.

  uv run python -I earnings22_fetch.py             # about 66 MB
  uv run python -I earnings22_fetch.py --files 4481221

Output: .work/public/earnings22/. Run with an isolated interpreter.
"""

from __future__ import annotations

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import public_data as P  # noqa: E402
from common import read_rttm, read_seglst  # noqa: E402

SET = "earnings22"
COMMIT = "c05ab6fd8b4b627d123c922a22a39e993dd37635"  # revdotcom/speech-datasets main, 29 May 2026
NLP_URL = f"https://raw.githubusercontent.com/revdotcom/speech-datasets/{COMMIT}/earnings22/transcripts/force_aligned_nlp_references/{{id}}.aligned.nlp"
MEDIA_URL = f"https://media.githubusercontent.com/media/revdotcom/speech-datasets/{COMMIT}/earnings22/media/{{id}}.mp3"
REGION = {
    "4481221": "India", "4474955": "China", "4420696": "South Korea", "4482383": "Australia",
    "4449269": "Brazil", "4469528": "Argentina", "4471586": "Russia", "4483623": "Canada",
}
# (file id, mp3 bytes, mp3 SHA-256 (the LFS oid), aligned.nlp SHA-256)
SELECTION = [
    ("4481221", 4947744, "626a68edf1fcedf944b512c3ea8022854e714bbc07de0b7a6e5a6033e40266c0", "d29fefa87e440cd653d91e22ff44308e36a8d109511ea5cdfbc9d8547806ae51"),
    ("4474955", 3500210, "ecd77a140c0bb2b179fa6804b0aa6211daa5ec3e0450839f3257012b0379541c", "ea6af551b70bd58f2bd7642896e24b1a1ff1d108c3b1a825bb200edd9066ea5c"),
    ("4420696", 14225810, "dc87791a57e111c886e93432bd12429febfe025147d1b48742df860da666f006", "e329f6447bf808f9bf22f89689285b161160d5234168362582e8f37e7a25d4a9"),
    ("4482383", 13890098, "6f8790a1954a6aa3d624fe28685060f3df547ab97247caf69dcd5d55d95c7a0d", "fcc0c329dd03fe7eed307a8aa0bce9c24f8345ea0b6e674d90e5555192f173f4"),
    ("4449269", 10574401, "e8b818c3f4050402b9a38db2cd3145e55fcdefab4f1e6e6882fc9d09ca56fe8a", "46dd0844deb7fb8388eb3e183385dff3330f04e39b25860f48a46420aa6a63d3"),
    ("4469528", 6687314, "698a5bb540373d1c9f6d422eb2323a05d5400827a6547d03eaecd61c797fe4c0", "aa9f9507c8cdd6f17086f6f9ec06b6ffd22c6a65a0b085cc639d3ec93332ccf8"),
    ("4471586", 5039858, "7bec6ad96978f5d170861fa1e30b12aec06fc65698046bdf4c6acfde8aae8018", "2cf6a449743dfb92b361abd1d0b30b76de4bcdab8b6f623697eb601bd9058996"),
    ("4483623", 6746930, "f355c449d8e41368bf122c352c5d96253ad757e7ea39efb7b6ccaf07f2e3dfb3", "cec0ba36c65d3f40390c402e749f8e835f8ff1f747217f0097359a4de45f1a2e"),
]
LICENCE = (
    "Transcripts CC BY-SA 4.0. earnings22/LICENSE.md: \"The transcripts and associated text files that are used for "
    "alignment in this directory are licensed under a Creative Commons Attribution-ShareAlike 4.0 International "
    "license.\" The audio has no licence statement of its own."
)


def call_rows(text: str) -> list[dict]:
    lines = text.splitlines()
    head = lines[0].split("|")
    col = {name: i for i, name in enumerate(head)}
    rows = []
    for line in lines[1:]:
        c = line.split("|")
        if len(c) < len(head):
            continue
        tok = c[col["token"]].strip()
        if not tok or (tok.startswith("<") and tok.endswith(">")):
            continue
        st, en = c[col["ts"]].strip(), c[col["endTs"]].strip()
        pre = c[col["prepunctuation"]] if "prepunctuation" in col else ""
        row = {
            "speaker": f"spk{c[col['speaker']].strip()}",
            "start_time": round(float(st), 3) if st and en else None,
            "end_time": round(max(float(en), float(st)), 3) if st and en else None,
            "words": tok,
            "timing": "word",
            "words_pc": f"{pre}{tok}{c[col['punctuation']]}",
        }
        rows.append(row)
    P.fill_missing_times(rows)
    return rows


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--files", help="comma-separated file ids (default: the eight of the slice)")
    P.common_args(ap)
    a = ap.parse_args()
    want = {x.strip() for x in a.files.split(",")} if a.files else None
    root = P.set_dir(SET)
    sessions, inputs, stats = [], {}, {}
    for fid, size, mp3_sha, nlp_sha in SELECTION:
        if want and fid not in want:
            continue
        wav = root / "audio" / f"{fid}.wav"
        if a.rebuild or not wav.is_file():
            mp3 = P.fetch(MEDIA_URL.format(id=fid), root / "download" / f"{fid}.mp3", mp3_sha, size=size,
                          allow_unpinned=a.allow_unpinned)
            P.to_wav16k(mp3, wav)
        sid = f"e22-{fid}"
        seg, rt = root / "refs" / f"{sid}.ref.seglst.jsonl", root / "refs" / f"{sid}.ref.rttm"
        if a.rebuild or not (seg.is_file() and rt.is_file()):
            nlp = P.fetch(NLP_URL.format(id=fid), root / "download" / f"{fid}.aligned.nlp", nlp_sha, size=300_000,
                          allow_unpinned=a.allow_unpinned)
            rows = call_rows(nlp.read_text(encoding="utf-8"))
            seg, rt = P.write_refs(root, sid, rows, P.rows_to_rttm(rows, sid, 0.0))
        inputs[f"{fid}.mp3"], inputs[f"{fid}.aligned.nlp"] = mp3_sha, nlp_sha
        stats[fid] = {**P.ref_stats(read_seglst(seg), read_rttm(rt)), "region": REGION.get(fid)}
        sessions.append({
            "id": sid, "label": f"Earnings-22 {fid} ({REGION.get(fid, '?')})", "group": "pub-phone", "set": SET,
            "language": "en", "ref_source": "earnings22-aligned", "audio": wav, "ref_rttm": rt, "ref_seglst": seg,
            "duration_s": round(P.wav_duration(wav), 1),
        })
    P.write_sessions(root, sessions)
    P.write_meta(root, {
        "set": SET,
        "content": "Earnings-22, eight calls, one per region family, forced-aligned references",
        "licence": LICENCE,
        "sources": {"references": NLP_URL, "media": MEDIA_URL},
        "sha256": inputs,
        "references": stats,
        "punctuation_casing": True,
    })
    if a.prune:
        P.prune([root / "download"])
    print(f"{len(sessions)} sessions, {sum(s['duration_s'] for s in sessions) / 3600:.2f} h in {root}")


if __name__ == "__main__":
    main()
