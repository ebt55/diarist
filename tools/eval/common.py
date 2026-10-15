"""Shared helpers for the eval harness: paths, session list, SegLST/RTTM I/O,
text normalisation and word expansion.

Dev-only. Nothing here ships with the app.
"""

from __future__ import annotations

import json
import re
import tomllib
import unicodedata
import warnings
from dataclasses import dataclass, field
from functools import lru_cache
from pathlib import Path
from typing import Iterable

EVAL_DIR = Path(__file__).resolve().parent
REPO = EVAL_DIR.parents[1]
WORK = EVAL_DIR / ".work"
BIN = EVAL_DIR / ".bin"
REFS_OVERRIDE = REPO / "data" / "eval" / "refs"
DOCS_EVAL = REPO / "docs" / "eval"

# Word times inside a Zoom cue are not known; consumers spread a cue's span
# over its words in proportion to their character length.
CUE_TIMING_NOTE = (
    "cue-level times from Zoom transcript.vtt; word times inside a cue are "
    "interpolated in proportion to word length"
)


# --------------------------------------------------------------------------
# Sessions


@dataclass
class Session:
    id: str
    label: str
    kind: str  # "zoom" or "ami"
    group: str  # tuning | heldout | control | stress | overlap
    audio: Path
    vtt: Path | None = None
    ref_rttm: Path | None = None
    ref_seglst: Path | None = None
    note: str = ""
    extra: dict = field(default_factory=dict)


GROUP_ORDER = ["tuning", "heldout", "control", "overlap", "stress"]


def _path(v: str | None) -> Path | None:
    """Absolute, or relative to the repo root. Under WSL (the metrics
    fallback) a Windows drive path becomes /mnt/<drive>/..."""
    if not v:
        return None
    import os

    if os.name != "nt":
        m = re.match(r"^([A-Za-z]):[\\/](.*)$", v)
        if m:
            return Path("/mnt") / m.group(1).lower() / m.group(2).replace("\\", "/")
        v = v.replace("\\", "/")
    p = Path(v)
    return p if p.is_absolute() else (REPO / p)


def load_sessions(path: Path) -> dict[str, Session]:
    """Reads sessions.toml (gitignored; see sessions.example.toml)."""
    if not path.is_file():
        raise SystemExit(
            f"{path} not found: copy sessions.example.toml to sessions.toml and fill in the paths"
        )
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    out: dict[str, Session] = {}
    for sid, s in data.get("sessions", {}).items():
        out[sid] = Session(
            id=sid,
            label=s.get("label", sid),
            kind=s.get("kind", "zoom"),
            group=s.get("group", "tuning"),
            audio=_path(s["audio"]),
            vtt=_path(s.get("vtt")),
            ref_rttm=_path(s.get("ref_rttm")),
            ref_seglst=_path(s.get("ref_seglst")),
            note=s.get("note", ""),
            extra={k: v for k, v in s.items()},
        )
    return out


# --------------------------------------------------------------------------
# SegLST (JSONL here: one segment object per line) and RTTM


def read_jsonl(path: Path) -> list[dict]:
    rows = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    return rows


def write_jsonl(path: Path, rows: Iterable[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")


def read_seglst(path: Path) -> list[dict]:
    """JSONL, or a JSON array (meeteval's own SegLST file format)."""
    text = path.read_text(encoding="utf-8").lstrip()
    if text.startswith("["):
        return json.loads(text)
    return read_jsonl(path)


@dataclass
class RttmRow:
    rec: str
    start: float
    dur: float
    speaker: str

    @property
    def end(self) -> float:
        return self.start + self.dur


def read_rttm(path: Path) -> list[RttmRow]:
    rows = []
    for line in path.read_text(encoding="utf-8").splitlines():
        parts = line.split()
        if len(parts) < 8 or parts[0] != "SPEAKER":
            continue
        rows.append(RttmRow(parts[1], float(parts[3]), float(parts[4]), parts[7]))
    return rows


def write_rttm(path: Path, rows: Iterable[RttmRow]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        for r in rows:
            f.write(
                f"SPEAKER {r.rec} 1 {r.start:.3f} {max(r.dur, 0.0):.3f} <NA> <NA> {r.speaker} <NA> <NA>\n"
            )


def seglst_to_rttm(segs: list[dict], rec: str) -> list[RttmRow]:
    return [
        RttmRow(rec, float(s["start_time"]), float(s["end_time"]) - float(s["start_time"]), str(s["speaker"]))
        for s in segs
        if float(s["end_time"]) > float(s["start_time"])
    ]


# --------------------------------------------------------------------------
# Text normalisation (applied identically to reference and hypothesis)

_FILLERS = {"um", "uh", "uhm", "umm", "hmm", "hm", "mm", "mhm", "mmhmm", "er", "erm", "ah", "eh"}
_ALIASES = {"ok": "okay"}

with warnings.catch_warnings():
    warnings.simplefilter("ignore")
    try:
        from whisper_normalizer.english import EnglishTextNormalizer

        _WHISPER = EnglishTextNormalizer()
    except Exception:  # pragma: no cover - fallback when the package is missing
        _WHISPER = None

NORMALIZER_NAME = (
    "whisper EnglishTextNormalizer per token + filler removal + ok->okay"
    if _WHISPER
    else "lowercase, strip punctuation, filler removal"
)


@lru_cache(maxsize=200_000)
def normalize_token(tok: str) -> tuple[str, ...]:
    """One raw token (may carry punctuation) -> zero or more normalised words."""
    if _WHISPER is not None:
        s = _WHISPER(tok)
    else:
        s = unicodedata.normalize("NFKC", tok).lower()
        s = s.replace("-", " ")
        s = re.sub(r"[^\w' ]+", " ", s)
        s = s.strip("' ")
    out = []
    for w in s.split():
        w = w.strip("'")
        w = _ALIASES.get(w, w)
        if w and w not in _FILLERS:
            out.append(w)
    return tuple(out)


def normalize_text(text: str) -> list[str]:
    out: list[str] = []
    for tok in text.split():
        out.extend(normalize_token(tok))
    return out


# --------------------------------------------------------------------------
# Words with times and speakers (for WDER)


@dataclass
class Word:
    word: str  # normalised
    speaker: str
    start: float
    end: float


def interpolate_words(segs: list[dict]) -> list[Word]:
    """Segments -> normalised words. Raw tokens get a share of the segment's
    span proportional to their length; a raw token that normalises to several
    words shares its span equally."""
    words: list[Word] = []
    for s in segs:
        toks = str(s.get("words", "")).split()
        if not toks:
            continue
        t0, t1 = float(s["start_time"]), float(s["end_time"])
        total = sum(len(t) + 1 for t in toks)
        acc = 0
        for tok in toks:
            a = t0 + (t1 - t0) * acc / total
            acc += len(tok) + 1
            b = t0 + (t1 - t0) * acc / total
            norm = normalize_token(tok)
            for k, w in enumerate(norm):
                ka = a + (b - a) * k / len(norm)
                kb = a + (b - a) * (k + 1) / len(norm)
                words.append(Word(w, str(s["speaker"]), ka, kb))
    words.sort(key=lambda w: (w.start, w.end))
    return words


def normalize_seglst(segs: list[dict]) -> list[dict]:
    """Copy of segments with normalised words (empty ones dropped)."""
    out = []
    for s in segs:
        ws = normalize_text(str(s.get("words", "")))
        if not ws:
            continue
        r = {k: s[k] for k in ("session_id", "speaker", "start_time", "end_time")}
        r["words"] = " ".join(ws)
        out.append(r)
    return out


def sha256_file(path: Path, chunk: int = 1 << 20) -> str:
    import hashlib

    h = hashlib.sha256()
    with open(path, "rb") as f:
        while b := f.read(chunk):
            h.update(b)
    return h.hexdigest()
