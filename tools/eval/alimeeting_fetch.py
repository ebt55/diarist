"""Fetch the AliMeeting Eval set (Mandarin meetings, CC BY-SA 4.0) and build
its references: set "alimeeting" of docs/05 "Extend" (group pub-lang).

AliMeeting (Alibaba, OpenSLR SLR119, the ICASSP 2022 M2MeT challenge) is
real meetings of 2 to 4 people in Mandarin, 15 to 30 minutes each, recorded
both by an 8-channel table-top microphone array and by a headset on each
participant, with a high share of overlapped speech. The Eval set is 8
meetings, 4 hours. Each meeting becomes two sessions:
  <meeting>-near  the participants' headsets summed into one 16 kHz mono mix
  <meeting>-far   channel 0 of the array, 16 kHz mono
so the same Mandarin speech is scored close and distant.

References: the meeting's manual transcription TextGrid (Eval_Ali_far
textgrid_dir), one tier per speaker, one row per transcribed interval
(timing "segment"), the text as transcribed (punctuated) in `words_pc` and
without punctuation in `words`; the RTTM is the intervals. The per-headset
TextGrids hold the same intervals with the same times (checked on
R8001_M8004), so both sessions of a meeting share one reference. Mandarin
is scored per character (CER) by the harness.

The source is one 3.4 GB archive (Eval_Ali.tar.gz). It is read once; the
array recordings are reduced to channel 0 as they are extracted.

  uv run python -I alimeeting_fetch.py --prune      # 3.4 GB download, about 1 GB kept

--prune deletes the archive once the sessions are written. Output:
.work/public/alimeeting/. Run with an isolated interpreter.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import sys
import tarfile
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import public_data as P  # noqa: E402
from common import RttmRow, read_rttm, read_seglst  # noqa: E402

SET = "alimeeting"
URL = "https://speech-lab-share-data.oss-cn-shanghai.aliyuncs.com/AliMeeting/openlr/Eval_Ali.tar.gz"
ARCHIVE = "Eval_Ali.tar.gz"
SIZE = 3_420_000_000  # "[3.42G]" on openslr.org/119
SHA256 = "dc47343b2474b5ebcf458927e878155f6ddeb59c85e685b3645c32a1f9578d92"  # 3,673,718,355 bytes, 26 Oct 2026
MEETINGS = ["R8001_M8004", "R8003_M8001", "R8007_M8010", "R8007_M8011", "R8008_M8013", "R8009_M8018",
            "R8009_M8019", "R8009_M8020"]
LICENCE = "CC BY-SA 4.0 (openslr.org/119: \"License: CC BY-SA 4.0\"). No account or registration."


def parse_textgrid(text: str) -> list[tuple[str, float, float, str]]:
    """(tier name, xmin, xmax, text) of every non-empty interval of a
    long-format Praat TextGrid."""
    out, tier = [], None
    xmin = xmax = None
    for line in text.splitlines():
        s = line.strip()
        m = re.match(r'name = "(.*)"', s)
        if m:
            tier = m.group(1)
            continue
        m = re.match(r"xmin = ([\d.]+)", s)
        if m:
            xmin = float(m.group(1))
            continue
        m = re.match(r"xmax = ([\d.]+)", s)
        if m:
            xmax = float(m.group(1))
            continue
        m = re.match(r'text = "(.*)"\s*$', s)
        if m and tier is not None and xmin is not None and xmax is not None:
            t = m.group(1).replace('""', '"').strip()
            if t:
                out.append((tier, xmin, xmax, t))
    return out


def textgrid_rows(text: str) -> list[dict]:
    rows = []
    for tier, a, b, t in parse_textgrid(text):
        words = " ".join(w for w in (P.strip_punct(x) for x in re.split(r"[\s，。？！、；：,.?!;:]+", t)) if w)
        if not words:
            continue
        rows.append({"speaker": tier, "start_time": round(a, 3), "end_time": round(max(a, b), 3), "words": words,
                     "timing": "segment", "words_pc": t})
    return rows


def extract(archive: Path, work: Path, tg_dir: Path) -> dict[str, dict]:
    """Streams the archive once: far-field WAVs reduced to channel 0, headset
    WAVs kept for the mix, the far-field TextGrids saved to tg_dir. Returns
    {meeting: {"far": wav, "near": [wavs]}}."""
    out: dict[str, dict] = defaultdict(lambda: {"near": []})
    tg_dir.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, mode="r|gz") as tar:
        for mem in tar:
            if not mem.isfile():
                continue
            name = mem.name.replace("\\", "/")
            base = name.rsplit("/", 1)[-1]
            m = re.match(r"(R\d+_M\d+)", base)
            if not m:
                continue
            meeting = m.group(1)
            f = tar.extractfile(mem)
            if f is None:
                continue
            if "_far/" in name and base.endswith(".wav"):
                raw = work / base
                with open(raw, "wb") as g:
                    shutil.copyfileobj(f, g)
                dst = work / f"{meeting}.far.ch0.wav"
                P.to_wav16k(raw, dst, channel=0)
                raw.unlink()
                out[meeting]["far"] = dst
            elif "_near/" in name and base.endswith(".wav"):
                dst = work / base
                with open(dst, "wb") as g:
                    shutil.copyfileobj(f, g)
                out[meeting]["near"].append(dst)
            elif "_far/" in name and base.lower().endswith(".textgrid"):
                (tg_dir / f"{meeting}.TextGrid").write_bytes(f.read())
            else:
                continue
            P.log(f"extracted {name}")
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    P.common_args(ap)
    a = ap.parse_args()
    root = P.set_dir(SET)
    tg_dir = root / "refs" / "textgrid"
    wavs = [root / "audio" / f"{m}.{k}.wav" for m in MEETINGS for k in ("near", "far")]
    complete = all(p.is_file() for p in wavs) and all((tg_dir / f"{m}.TextGrid").is_file() for m in MEETINGS)
    if (root / "sessions.toml").is_file() and complete and not a.rebuild:
        print(f"present: {root} (pass --rebuild to rebuild the references)")
        return
    if not complete:
        archive = P.fetch(URL, root / "download" / ARCHIVE, SHA256, size=SIZE, allow_unpinned=a.allow_unpinned)
        work = root / "download" / "extract"
        work.mkdir(parents=True, exist_ok=True)
        for m, d in extract(archive, work, tg_dir).items():
            if "far" in d:
                (root / "audio").mkdir(parents=True, exist_ok=True)
                d["far"].replace(root / "audio" / f"{m}.far.wav")
            if d["near"]:
                P.to_wav16k(sorted(d["near"]) if len(d["near"]) > 1 else d["near"][0], root / "audio" / f"{m}.near.wav")
        shutil.rmtree(work, ignore_errors=True)
    sessions, stats = [], {}
    for m in MEETINGS:
        rows = textgrid_rows((tg_dir / f"{m}.TextGrid").read_text(encoding="utf-8-sig", errors="replace"))
        for kind in ("near", "far"):
            sid = f"ali-{m.lower()}-{kind}"
            wav = root / "audio" / f"{m}.{kind}.wav"
            rows = [dict(r) for r in rows]
            rttm = [RttmRow(sid, r["start_time"], r["end_time"] - r["start_time"], r["speaker"]) for r in rows
                    if r["end_time"] > r["start_time"]]
            seg, rt = P.write_refs(root, sid, rows, rttm)
            stats[sid] = P.ref_stats(read_seglst(seg), read_rttm(rt))
            sessions.append({
                "id": sid, "label": f"AliMeeting {m} {'headset mix' if kind == 'near' else 'array ch0'}",
                "group": "pub-lang", "set": SET, "language": "zh-CN", "ref_source": "alimeeting-segment",
                "audio": wav, "ref_rttm": rt, "ref_seglst": seg, "duration_s": round(P.wav_duration(wav), 1),
            })
    P.write_sessions(root, sessions)
    P.write_meta(root, {
        "set": SET,
        "content": "AliMeeting Eval: 8 Mandarin meetings, headset mix and array channel 0",
        "licence": LICENCE,
        "sources": {"archive": URL},
        "sha256": {ARCHIVE: SHA256},
        "references": stats,
        "punctuation_casing": "punctuation as transcribed (Chinese has no case)",
    })
    if a.prune:
        P.prune([root / "download"])
    print(f"{len(sessions)} sessions, {sum(s['duration_s'] for s in sessions) / 3600:.2f} h in {root}")


if __name__ == "__main__":
    main()
