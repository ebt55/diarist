"""Fetch a Hindi slice of Indic DiarBench (Sarvam AI, CC BY 4.0) and build
its references: set "indic" of docs/05 "Extend" (group pub-lang).

Indic DiarBench (Interspeech 2026, arXiv 2607.23808; sarvamai/indic-diarbench
on Hugging Face, not gated) is a speaker-attributed ASR benchmark of natural
multi-speaker conversation in the 22 scheduled languages of India, with
human-corrected, time-aligned, speaker-attributed transcripts and frequent
English code-mixing. Hindi is the only one of them in Diarist's language
list. Its Hindi part is 94 clips, 10.7 hours, in three conditions: near
field (virtual meetings, one microphone per speaker), far field (distant
microphones) and in the wild (YouTube).

The slice: near and far field only (the in-the-wild clips are cut from
third-party videos); per condition, in sample-id order, one clip per source
recording, clips of 5 to 30 minutes, until 2 hours or the clips run out:
10 near-field and 10 far-field clips of 5 to 15 minutes, 2 to 4 speakers,
3.68 hours (the 30-minute and 60-minute recordings are just over the
limit). Every clip is its own session. References: the
`annotated_transcript` turns (speaker, start, end, text), one row per turn
(timing "segment"); the RTTM is the turns. A turn writes each English
word twice, in Devanagari and then in Roman script in parentheses, so a
row has `words` (all Devanagari, the default reference), `words_mixed`
(English words in Roman script, the card's "normalized" form) and
`words_pc` (Devanagari with the transcript's punctuation); [bracketed]
event markup is dropped.

The source is one 1.2 GB parquet file whose audio column is stored per row
group. Only the parquet footer, the small columns and the four row groups
that hold the chosen clips are read, by HTTP range requests (0.63 GB).
The revision is pinned, and every clip's audio bytes are checked against a
SHA-256 recorded at the first fetch. Reading parquet needs pyarrow, which
the harness does not otherwise depend on:

  uv run --with pyarrow==21.0.0 python -I indic_fetch.py

Output: .work/public/indic/. Run with an isolated interpreter.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import re
import sys
import wave
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import public_data as P  # noqa: E402
from common import RttmRow, read_rttm, read_seglst  # noqa: E402

SET = "indic"
REVISION = "92877bad8aab6e598167d91c6ee02aa8ca6ede09"  # sarvamai/indic-diarbench, 11 Aug 2026
URL = f"https://huggingface.co/datasets/sarvamai/indic-diarbench/resolve/{REVISION}/Hindi/test-00000-of-00001.parquet"
SIZE = 1_217_269_358
FILE_SHA256 = "48c28d9dfc2e1353f639c49f5d0b29be3d6c1090031c90d78215f835289f7a14"  # the LFS oid of the whole file
CONDITIONS = {"Near field": "nf", "Far field": "ff"}
MIN_S, MAX_S = 300.0, 1800.0
# SHA-256 of each chosen clip's audio bytes as stored in the parquet, recorded at the first fetch.
CLIP_SHA256: dict[str, str] = {
    "hindi_054": "13f2abd1f41271333cf50c1b42b4c5ce70663c13b5cbf4ab647dc06a404080b0",
    "hindi_055": "d9310b0926265037f82e47764ffbed3bda061fbd58a0051549506b4d18c9adf9",
    "hindi_056": "cc567509a5b839c4de285f3fad36006f08943c2d1b16a23f7694e82320b3adcb",
    "hindi_057": "da5a0a7e36f200844e1d53ad332dac57edd5867eb59da307306fe54e3b8bcb5c",
    "hindi_058": "e3b251e47b4e46a8a1e9364752b3f2e1f3df72e9d20d0eae2370fb588d70fedf",
    "hindi_059": "19e1c67ae5e139546db6ee3c9a9acea2a694c72798414dbd23d5a2ebf855139e",
    "hindi_060": "cda22209a04114327e98ec95343057f17d8f5ed1d443df5ec660bf94dd60ef1b",
    "hindi_061": "ae9c2f782bcab6325ed739f8f5ffe0395af87dde9b3738b2a219c9cbe01a5097",
    "hindi_062": "1ca6c04e46dd224f500ea0550a5716bb122f2d22123a3b010490f14317d81aa9",
    "hindi_063": "e4b1e1b9893f1af7320146ee876b942ac199a28dd51b72b9cc411c1b8afc8b50",
    "hindi_075": "27cc12b8937aa16affaa871c4ef7ab4dda4ab02076e2be92f822db723f4aeca5",
    "hindi_076": "98dd557b9f63835940dfb3a48c79d4c6b28db7011b426a8cab65d18f95fab428",
    "hindi_077": "b85c9abfb484809ce9d8200a9a3c597353d53085f3942c9976bc70e3f76f6ced",
    "hindi_078": "9939e911c9fedddcb707c00a5fcbae2f1b187c6972af677ab18b315e156148d1",
    "hindi_079": "20cf212859d8d0b7285cce1c1ecbdd73b335ba605baf9e2eb89e8c0c2d65ddab",
    "hindi_080": "132d52f43ff6f8550d8ccfe6085ae5704f92dcd5f196687c843ee9ac29b78734",
    "hindi_081": "6b8098469bdcb526a056bcf8fc705ecd6190f0c101e277902c1d74bcc4857fed",
    "hindi_082": "cb1f084f03cef6940d50659c1821dfa6fdb8be3c5ecbeefbea4c5c1d3f489d9b",
    "hindi_083": "dad91212fc8be9495e0e3250bd35e0c6538ed1929674f2a2da0d83e7666fd996",
    "hindi_084": "ebbd89f351ae130506b65cc872514159cbf38e241e7d3abcbb6b006f40c609a4",
}
LICENCE = (
    "CC BY 4.0. Dataset card: \"This dataset is released under the CC BY 4.0 license.\" Repository not gated. "
    "In-the-wild clips (from YouTube) left out of the slice."
)


class RangeFile(io.RawIOBase):
    """A read-only, seekable view of a remote file, fetched by range."""

    def __init__(self, url: str, size: int, tmp: Path):
        self.url, self.size, self.pos, self.read_bytes, self.tmp = url, size, 0, 0, tmp

    def readable(self) -> bool:
        return True

    def seekable(self) -> bool:
        return True

    def tell(self) -> int:
        return self.pos

    def seek(self, off: int, whence: int = 0) -> int:
        self.pos = off if whence == 0 else (self.pos + off if whence == 1 else self.size + off)
        return self.pos

    def readinto(self, b) -> int:
        want = min(len(b), self.size - self.pos)
        if want <= 0:
            return 0
        data = P.range_get(self.url, self.pos, self.pos + want - 1, self.tmp)
        b[: len(data)] = data
        self.pos += len(data)
        self.read_bytes += len(data)
        return len(data)


def choose(rows: list[dict], hours: float) -> list[dict]:
    chosen = []
    for cond in CONDITIONS:
        total, seen = 0.0, set()
        for r in sorted((r for r in rows if r["dataset_type"] == cond), key=lambda r: r["sample_id"]):
            if total >= hours * 3600:
                break
            if r["recording_id"] in seen or not (MIN_S <= float(r["duration_seconds"]) <= MAX_S):
                continue
            seen.add(r["recording_id"])
            chosen.append(r)
            total += float(r["duration_seconds"])
    return chosen


def write_wav(data: bytes, dst: Path) -> None:
    """The parquet's audio bytes (WAV, 16 kHz mono per the card) as a 16 kHz
    mono PCM16 file; anything else goes through ffmpeg."""
    dst.parent.mkdir(parents=True, exist_ok=True)
    try:
        with wave.open(io.BytesIO(data)) as w:
            if w.getsampwidth() == 2 and w.getnchannels() == 1 and w.getframerate() == P.RATE:
                dst.write_bytes(data)
                return
    except (wave.Error, EOFError):
        pass
    raw = dst.with_suffix(".src")
    raw.write_bytes(data)
    P.to_wav16k(raw, dst)
    raw.unlink()


_GLOSS = re.compile(r"\s*\(([^()]*)\)")
_TAG = re.compile(r"\[[^\]]*\]")


def forms(text: str) -> tuple[str, str, str]:
    """A transcript turn writes each English word twice, in Devanagari and
    then in Roman script in parentheses ("हेलो (hello) गाइस, (guys)"), and
    marks events in brackets ([Unintelligible]). Returns (native words,
    code-mixed words: each glossed Devanagari word replaced by its Roman
    form, native text with punctuation); markup removed from all three."""
    text = _TAG.sub(" ", text)
    native_pc = re.sub(r"\s+([,.?!।;:])", r"\1", " ".join(_GLOSS.sub(" ", text).split()))
    mixed: list[str] = []
    pos = 0
    for m in _GLOSS.finditer(text):
        before = text[pos:m.start()].split()
        if before:
            before = before[:-1]  # the Devanagari word the gloss stands for
        mixed += before + m.group(1).split()
        pos = m.end()
    mixed += text[pos:].split()
    clean = lambda toks: " ".join(w for w in (P.strip_punct(x) for x in toks) if w)  # noqa: E731
    return clean(native_pc.split()), clean(mixed), native_pc


def turn_rows(turns: list[dict], sid: str) -> tuple[list[dict], list[RttmRow]]:
    rows = []
    for t in turns:
        words, mixed, native_pc = forms(str(t.get("transcript") or ""))
        if not words:
            continue
        st, en = float(t["start_time"]), float(t["end_time"])
        rows.append({"speaker": P.speaker_id(t["speaker_id"]), "start_time": round(st, 3),
                     "end_time": round(max(en, st), 3), "words": words, "timing": "segment",
                     "words_pc": native_pc, "words_mixed": mixed})
    rttm = [RttmRow(sid, r["start_time"], r["end_time"] - r["start_time"], r["speaker"]) for r in rows
            if r["end_time"] > r["start_time"]]
    return rows, rttm


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--hours", type=float, default=2.0, help="hours per condition (default 2)")
    P.common_args(ap)
    a = ap.parse_args()
    root = P.set_dir(SET)
    try:
        done = json.loads((root / "meta.json").read_text(encoding="utf-8")).get("hours_per_condition") == a.hours
    except (OSError, ValueError):
        done = False
    if done and (root / "sessions.toml").is_file() and not a.rebuild:
        print(f"present: {root} (pass --rebuild to rebuild)")
        return
    import pyarrow.parquet as pq

    rf = RangeFile(URL, SIZE, root / "download" / "range.part")
    pf = pq.ParquetFile(io.BufferedReader(rf, buffer_size=1 << 16))
    small = ["sample_id", "recording_id", "dataset_type", "duration_seconds", "num_speakers"]
    rows, where = [], {}
    for rg in range(pf.num_row_groups):
        for r in pf.read_row_group(rg, columns=small).to_pylist():
            rows.append(r)
            where[r["sample_id"]] = rg
    chosen = {r["sample_id"]: r for r in choose(rows, a.hours)}
    P.log(f"chosen {len(chosen)} clips in row groups {sorted({where[s] for s in chosen})}")
    # A clip whose WAV is already there and matches its pin (the parquet's
    # bytes are written as they are) needs no audio read.
    have = {s for s in chosen if CLIP_SHA256.get(s) and (root / "audio" / f"{s}.wav").is_file()
            and P.sha256_file(root / "audio" / f"{s}.wav") == CLIP_SHA256[s]}
    sessions, stats, shas = [], {}, {}
    for rg in sorted({where[s] for s in chosen}):
        need_audio = any(where[s] == rg and s not in have for s in chosen)
        cols = ["sample_id", "annotated_transcript"] + (["audio"] if need_audio else [])
        for r in pf.read_row_group(rg, columns=cols).to_pylist():
            pick = chosen.get(r["sample_id"])
            if not pick:
                continue
            sid = f"idb-{r['sample_id'].replace('_', '-')}"
            wav = root / "audio" / f"{r['sample_id']}.wav"
            pin = CLIP_SHA256.get(r["sample_id"])
            if r["sample_id"] in have:
                sha = pin
            else:
                data = r["audio"]["bytes"]
                sha = hashlib.sha256(data).hexdigest()
                if pin and pin != sha:
                    raise SystemExit(f"{r['sample_id']}: audio SHA-256 {sha} does not match the pin {pin}")
                if not pin and not a.allow_unpinned:
                    raise SystemExit(f"{r['sample_id']}: no pinned SHA-256; pass --allow-unpinned once")
                if not pin:
                    P.log(f"UNPINNED {r['sample_id']} sha256 {sha}")
                write_wav(data, wav)
            shas[r["sample_id"]] = sha
            ref_rows, rttm = turn_rows(r["annotated_transcript"], sid)
            seg, rt = P.write_refs(root, sid, ref_rows, rttm)
            cond = "near field" if pick["dataset_type"] == "Near field" else "far field"
            stats[r["sample_id"]] = {**P.ref_stats(read_seglst(seg), read_rttm(rt)),
                                     "recording_id": pick["recording_id"], "condition": cond}
            sessions.append({
                "id": sid, "label": f"Indic DiarBench {r['sample_id']} ({cond})", "group": "pub-lang", "set": SET,
                "language": "hi", "ref_source": "indic-turn", "audio": wav, "ref_rttm": rt, "ref_seglst": seg,
                "duration_s": round(P.wav_duration(wav), 1),
            })
    sessions.sort(key=lambda s: s["id"])
    P.write_sessions(root, sessions)
    P.write_meta(root, {
        "set": SET,
        "content": f"Indic DiarBench Hindi, near and far field, clips of {MIN_S / 60:.0f} to {MAX_S / 60:.0f} min, "
                   f"one per recording, {a.hours} h per condition",
        "hours_per_condition": a.hours,
        "licence": LICENCE,
        "sources": {"parquet": URL, "file_sha256": FILE_SHA256},
        "sha256": {f"{k}.audio": v for k, v in shas.items()},
        "downloaded_bytes": rf.read_bytes,
        "references": stats,
        "punctuation_casing": "punctuation as transcribed (Devanagari has no case)",
    })
    print(f"{len(sessions)} sessions, {sum(s['duration_s'] for s in sessions) / 3600:.2f} h in {root}; "
          f"read {rf.read_bytes / 1e6:.0f} MB of {SIZE / 1e6:.0f} MB")


if __name__ == "__main__":
    main()
