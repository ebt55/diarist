"""Shared helpers for the eval harness: paths, session list, SegLST/RTTM I/O,
text normalisation and word expansion.

Dev-only. Nothing here ships with the app.
"""

from __future__ import annotations

import json
import re
import sys
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
    kind: str  # "zoom", or "ami" / "ref" (prebuilt SegLST + RTTM references)
    group: str  # tuning | heldout | control | stress | overlap | pub-*
    audio: Path
    vtt: Path | None = None
    ref_rttm: Path | None = None
    ref_seglst: Path | None = None
    note: str = ""
    language: str | None = None  # overrides the config's language (public non-English sets)
    extra: dict = field(default_factory=dict)


# The owner's gate groups, then the public sets (docs/05 "Extend").
PUBLIC_GROUPS = ["pub-meeting", "pub-far", "pub-phone", "pub-lang"]
GROUP_ORDER = ["tuning", "heldout", "control", "overlap", "stress", *PUBLIC_GROUPS]


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


def load_sessions(path: Path, _seen: set | None = None) -> dict[str, Session]:
    """Reads sessions.toml (gitignored; see sessions.example.toml).

    A top-level `include = [...]` lists further session files (relative to
    the repo root; glob patterns allowed), such as the per-set lists the
    public fetch scripts write. An include that matches nothing is skipped
    with a note, so a set that has not been fetched does not stop a run."""
    if not path.is_file():
        raise SystemExit(
            f"{path} not found: copy sessions.example.toml to sessions.toml and fill in the paths"
        )
    seen = _seen if _seen is not None else set()
    seen.add(path.resolve())
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
            language=s.get("language"),
            extra={k: v for k, v in s.items()},
        )
    for pattern in data.get("include", []):
        import glob

        matches = [Path(m) for m in sorted(glob.glob(str(_path(pattern)))) if Path(m).is_file()]
        if not matches:
            print(f"note: {pattern} (included from {path.name}) matches no file; skipped", file=sys.stderr)
        for m in matches:
            if m.resolve() in seen:
                continue
            for sid, sess in load_sessions(m, seen).items():
                if sid in out:
                    raise SystemExit(f"session id {sid!r} appears twice ({m})")
                out[sid] = sess
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


def multilingual_tokens(text: str) -> list[str]:
    """Non-English normaliser: NFKC, casefold, every punctuation or symbol
    character (Unicode P*, S*) becomes a space. Combining marks (Devanagari
    vowel signs, Japanese voicing marks) are kept, unlike whisper's
    BasicTextNormalizer, which splits Hindi words at every vowel sign."""
    s = unicodedata.normalize("NFKC", text).casefold()
    s = "".join(" " if unicodedata.category(c)[0] in "PS" else c for c in s)
    return s.split()


# Languages scored per character (CER): a Han, kana or hangul character is one
# unit, a run of other letters or digits (an English word in a Mandarin
# sentence) is one unit.
CER_LANGS = {"zh", "ja"}
# Hesitations dropped on both sides, as the English fillers are. Mandarin:
# 嗯 and 呃 are only ever interjections (啊 is also a sentence particle, kept).
_ML_FILLERS = {
    "fr": {"euh", "heu", "hum", "hm", "mh", "mm", "mhm"},
    "zh": {"嗯", "呃"},
}


def lang_base(lang: str | None) -> str:
    return (lang or "en").replace("_", "-").split("-")[0].lower()


def _is_cjk(c: str) -> bool:
    o = ord(c)
    return (
        0x3040 <= o <= 0x30FF  # hiragana, katakana
        or 0x3400 <= o <= 0x4DBF
        or 0x4E00 <= o <= 0x9FFF  # CJK unified ideographs
        or 0xF900 <= o <= 0xFAFF
        or 0xAC00 <= o <= 0xD7AF  # hangul syllables
        or 0x20000 <= o <= 0x2FA1F
    )


def cjk_units(tok: str) -> list[str]:
    out, run = [], ""
    for c in tok:
        if _is_cjk(c):
            if run:
                out.append(run)
                run = ""
            out.append(c)
        else:
            run += c
    if run:
        out.append(run)
    return out


@lru_cache(maxsize=200_000)
def _ml_token(base: str, tok: str) -> tuple[str, ...]:
    fill = _ML_FILLERS.get(base, set())
    out: list[str] = []
    for t in multilingual_tokens(tok):
        units = cjk_units(t) if base in CER_LANGS else [t]
        out.extend(u for u in units if u not in fill)
    return tuple(out)


def token_normalizer(lang: str | None = None):
    """The per-token normaliser for a session language: the English one for
    en (and no language), the multilingual one otherwise."""
    base = lang_base(lang)
    if base == "en":
        return normalize_token
    return lambda tok: _ml_token(base, tok)


def normalizer_name(lang: str | None = None) -> str:
    base = lang_base(lang)
    if base == "en":
        return NORMALIZER_NAME
    unit = "characters (CER) for Han and kana, words otherwise" if base in CER_LANGS else "words"
    fill = ", hesitations " + " ".join(sorted(_ML_FILLERS[base])) + " dropped" if base in _ML_FILLERS else ""
    return f"NFKC, casefold, punctuation and symbols removed, combining marks kept; units: {unit}{fill}"


def normalize_text(text: str, lang: str | None = None) -> list[str]:
    norm = token_normalizer(lang)
    out: list[str] = []
    for tok in text.split():
        out.extend(norm(tok))
    return out


# --------------------------------------------------------------------------
# Words with times and speakers (for WDER)


@dataclass
class Word:
    word: str  # normalised
    speaker: str
    start: float
    end: float


def interpolate_words(segs: list[dict], lang: str | None = None) -> list[Word]:
    """Segments -> normalised words. Raw tokens get a share of the segment's
    span proportional to their length; a raw token that normalises to several
    words shares its span equally."""
    to_words = token_normalizer(lang)
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
            norm = to_words(tok)
            for k, w in enumerate(norm):
                ka = a + (b - a) * k / len(norm)
                kb = a + (b - a) * (k + 1) / len(norm)
                words.append(Word(w, str(s["speaker"]), ka, kb))
    words.sort(key=lambda w: (w.start, w.end))
    return words


def normalize_seglst(segs: list[dict], lang: str | None = None) -> list[dict]:
    """Copy of segments with normalised words (empty ones dropped)."""
    out = []
    for s in segs:
        ws = normalize_text(str(s.get("words", "")), lang)
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
