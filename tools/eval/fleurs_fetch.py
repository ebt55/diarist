"""Fetch a slice of the FLEURS test set (google/fleurs, CC-BY-4.0) per language
for the WER-only mode of run.py (`run.py --config ... --fleurs de,es,...`).

  uv run python -I fleurs_fetch.py                    # de es fr it pt hi ja en, 10 min each
  uv run python -I fleurs_fetch.py --langs de,ja --minutes 5

Source: the dataset's own files on Hugging Face, data/<config>/test.tsv
(transcripts) and data/<config>/audio/test.tar.gz (16 kHz WAVs), streamed and
abandoned as soon as the minutes are reached, so a 10-minute slice costs a
small part of the 0.3 to 0.6 GB archive. The parquet conversion
(refs/convert/parquet) cannot be read in part: each language is one row group
whose audio column is a single dictionary page holding every recording, so
any read of the audio downloads the whole file.

Utterances are taken in archive order, one recording per sentence id (FLEURS
has several speakers per sentence), until the minutes are reached.

Output per language in .work/fleurs/<lang>/:
  audio.wav    16 kHz mono PCM16: the utterances joined with GAP_S of silence
  refs.jsonl   one row per utterance: id, file, start, end (seconds in
               audio.wav), raw_transcription (cased, punctuated),
               transcription (FLEURS's own normalised form), gender
  meta.json    config name, codes, counts, duration, source URLs, licence

Run with an isolated interpreter (the archives are downloaded data).
"""

from __future__ import annotations

import argparse
import io
import json
import os
import sys
import tarfile
import urllib.request
import wave
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import WORK, write_jsonl  # noqa: E402

BASE = "https://huggingface.co/datasets/google/fleurs/resolve/main/data/{config}"
OUT = WORK / "fleurs"
GAP_S = 1.5  # silence between utterances: longer than the 800 ms endpointing threshold
RATE = 16000

# app language code -> (FLEURS config, Nemotron 3.5 prompt locale). The app
# passes the short code; the GGUF's prompt dictionary maps it (pt -> pt-PT,
# es -> es-US) or, for ja, has no short alias at all.
LANGS = {
    "de": ("de_de", "de-DE"),
    "es": ("es_419", "es-US"),
    "fr": ("fr_fr", "fr-FR"),
    "it": ("it_it", "it-IT"),
    "pt": ("pt_br", "pt-BR"),
    "hi": ("hi_in", "hi-IN"),
    "ja": ("ja_jp", "ja-JP"),
    "en": ("en_us", "en-US"),
}


class Counting(io.RawIOBase):
    """Counts the bytes read from a response stream."""

    def __init__(self, raw):
        self.raw = raw
        self.n = 0

    def readable(self) -> bool:
        return True

    def readinto(self, b) -> int:
        data = self.raw.read(len(b))
        b[: len(data)] = data
        self.n += len(data)
        return len(data)


def get(url: str, timeout: float = 120):
    req = urllib.request.Request(url, headers={"User-Agent": "diarist-eval"})
    return urllib.request.urlopen(req, timeout=timeout)


def read_tsv(config: str) -> dict[str, dict]:
    """file name -> row of test.tsv (id, file, raw, normalised, chars, samples, gender)."""
    with get(f"{BASE.format(config=config)}/test.tsv") as r:
        text = r.read().decode("utf-8")
    rows = {}
    for line in text.splitlines():
        parts = line.split("\t")
        if len(parts) < 7:
            continue
        rows[parts[1]] = {
            "id": int(parts[0]),
            "file": parts[1],
            "raw_transcription": parts[2],
            "transcription": parts[3],
            "num_samples": int(parts[5]),
            "gender": parts[6],
        }
    return rows


def pcm16_mono_16k(data: bytes) -> bytes:
    """WAV bytes -> raw PCM16 mono 16 kHz frames (FLEURS ships 16 kHz WAV;
    anything else goes through ffmpeg)."""
    try:
        with wave.open(io.BytesIO(data)) as w:
            if w.getsampwidth() == 2 and w.getnchannels() == 1 and w.getframerate() == RATE:
                return w.readframes(w.getnframes())
    except (wave.Error, EOFError):
        pass
    import subprocess

    p = subprocess.run(
        ["ffmpeg", "-nostdin", "-v", "error", "-i", "pipe:0", "-ac", "1", "-ar", str(RATE), "-f", "s16le", "pipe:1"],
        input=data, capture_output=True, check=True,
    )
    return p.stdout


def fetch(lang: str, minutes: float, force: bool = False) -> None:
    config, locale = LANGS[lang]
    out = OUT / lang
    if (out / "meta.json").is_file() and not force:
        meta = json.loads((out / "meta.json").read_text(encoding="utf-8"))
        if meta.get("minutes_requested") == minutes:
            print(f"[{lang}] present ({meta['utterances']} utterances, {meta['duration_s'] / 60:.1f} min)")
            return
    tsv = read_tsv(config)
    target = minutes * 60
    seen: set[int] = set()
    rows, pcm, t = [], bytearray(), 0.0
    gap = b"\0\0" * int(GAP_S * RATE)
    url = f"{BASE.format(config=config)}/audio/test.tar.gz"
    resp = get(url, timeout=300)
    total = int(resp.headers.get("Content-Length") or 0)
    stream = Counting(resp)
    with tarfile.open(fileobj=io.BufferedReader(stream, 1 << 20), mode="r|gz") as tar:
        for member in tar:
            if not member.isfile():
                continue
            name = Path(member.name).name
            row = tsv.get(name)
            if row is None or row["id"] in seen:
                continue
            fobj = tar.extractfile(member)
            if fobj is None:
                continue
            seen.add(row["id"])
            frames = pcm16_mono_16k(fobj.read())
            dur = len(frames) / 2 / RATE
            if pcm:
                pcm += gap
                t += GAP_S
            rows.append(
                {
                    "id": row["id"],
                    "file": name,
                    "start": round(t, 3),
                    "end": round(t + dur, 3),
                    "raw_transcription": row["raw_transcription"],
                    "transcription": row["transcription"],
                    "gender": row["gender"],
                }
            )
            pcm += frames
            t += dur
            if t >= target:
                break
    resp.close()
    out.mkdir(parents=True, exist_ok=True)
    with wave.open(str(out / "audio.wav"), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(bytes(pcm))
    write_jsonl(out / "refs.jsonl", rows)
    meta = {
        "lang": lang,
        "fleurs_config": config,
        "locale": locale,
        "utterances": len(rows),
        "test_utterances": len(tsv),
        "duration_s": round(t, 3),
        "gap_s": GAP_S,
        "minutes_requested": minutes,
        "source": url,
        "licence": "CC-BY-4.0 (google/fleurs)",
        "downloaded_mb": round(stream.n / 1e6, 1),
        "archive_mb": round(total / 1e6, 1),
    }
    (out / "meta.json").write_text(json.dumps(meta, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"[{lang}] {len(rows)} utterances, {t / 60:.1f} min; read {stream.n / 1e6:.0f} of {total / 1e6:.0f} MB")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--langs", default=",".join(LANGS), help="comma-separated app codes: " + ",".join(LANGS))
    ap.add_argument("--minutes", type=float, default=10.0)
    ap.add_argument("--force", action="store_true")
    a = ap.parse_args()
    for lang in [x.strip() for x in a.langs.split(",") if x.strip()]:
        if lang not in LANGS:
            raise SystemExit(f"unknown language {lang!r}; known: {', '.join(LANGS)}")
        fetch(lang, a.minutes, a.force)


if __name__ == "__main__":
    main()
