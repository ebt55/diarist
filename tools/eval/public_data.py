"""Shared helpers for the public-set fetch scripts (`*_fetch.py` other than
ami_fetch.py and fleurs_fetch.py): pinned downloads with curl, SHA-256
checks, audio conversion to 16 kHz mono WAV, reference writing, and the
per-set session list that sessions.public.toml includes.

Layout of one set, tools/eval/.work/public/<set>/ (gitignored):
  download/      what was fetched, as fetched (removed with --prune once the
                 set's outputs are written)
  audio/         16 kHz mono PCM WAV per session
  refs/          <session>.ref.seglst.jsonl and <session>.ref.rttm
  sessions.toml  one [sessions.<id>] table per session (kind "ref")
  meta.json      source, revision, licence, SHA-256 of every input, counts

Reference rows (SegLST, one JSON object per line), the same fields as the
Zoom and AMI references plus two optional ones:
  session_id, speaker, start_time, end_time   seconds in the session's WAV
  words      the words as spoken, punctuation removed, the source's casing
  timing     "word" (source word times), "interp" (a word without its own
             time, spread between its timed neighbours) or "segment" (one
             row per turn or utterance)
  words_pc   only where the source punctuates and cases: the same words with
             the source's punctuation and casing (one word, or the whole
             turn for "segment" rows), for the punctuation work
             (docs/research/18-punctuation.md)

Downloads run curl (C:\\Windows\\System32\\curl.exe on Windows) with
--ssl-revoke-best-effort and a neutral User-Agent. Every pinned file is
checked against its SHA-256; a file without a pin is refused unless
--allow-unpinned is given, which prints the hash to pin.

Agents sharing the PC can set DIARIST_EVAL_LOCKS to the folder that holds
their quiet.lock: while it exists, no download larger than 50 MB starts (one
that is running when it appears stops and resumes afterwards) and smaller
files are throttled to 1 MB/s.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import time
import unicodedata
import wave
from pathlib import Path
from typing import Iterable

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import REPO, WORK, RttmRow, sha256_file, write_jsonl, write_rttm  # noqa: E402

PUBLIC = WORK / "public"
USER_AGENT = "diarist-research"
BIG_BYTES = 50_000_000
RATE = 16000

_WIN_CURL = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32" / "curl.exe"
CURL = str(_WIN_CURL) if os.name == "nt" and _WIN_CURL.is_file() else "curl"


def log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def set_dir(name: str) -> Path:
    return PUBLIC / name


# --------------------------------------------------------------------------
# Downloads


def _quiet_lock() -> Path | None:
    locks = os.environ.get("DIARIST_EVAL_LOCKS")
    return Path(locks) / "quiet.lock" if locks else None


def wait_quiet() -> None:
    q = _quiet_lock()
    announced = False
    while q is not None and q.exists():
        if not announced:
            log(f"waiting: {q} exists (a timed measurement is running)")
            announced = True
        time.sleep(30)


def curl(url: str, dest: Path, *, size: int | None = None) -> None:
    """One download to dest via dest.part (resumed when a .part exists). A
    large download pauses while DIARIST_EVAL_LOCKS/quiet.lock exists: curl is
    stopped and resumes from the partial file afterwards."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_name(dest.name + ".part")
    big = size is None or size > BIG_BYTES
    q = _quiet_lock() if big else None
    shown = f"{size / 1e6:.1f} MB" if size else "size unknown"
    while True:
        if big:
            wait_quiet()
        cmd = [CURL, "-L", "--fail", "--retry", "3", "--retry-delay", "5", "-A", USER_AGENT, "-sS"]
        if os.name == "nt":
            cmd.append("--ssl-revoke-best-effort")
        lock = _quiet_lock()
        if not big and lock is not None and lock.exists():
            cmd += ["--limit-rate", "1M"]  # a small file during a timed measurement: allowed, throttled
        have = part.stat().st_size if part.is_file() else 0
        if size and have >= size:
            break
        if have > 0:
            cmd += ["-C", "-"]
        cmd += ["-o", str(part), url]
        log(f"downloading {url} ({shown}{f', resuming at {have / 1e6:.1f} MB' if have else ''})")
        p = subprocess.Popen(cmd)
        paused = False
        while p.poll() is None:
            time.sleep(2)
            if q is not None and q.exists():
                p.terminate()
                p.wait()
                paused = True
                log("paused: quiet.lock appeared; resuming when it is gone")
                break
        if paused:
            continue
        if p.returncode != 0:
            raise SystemExit(
                f"curl failed ({p.returncode}) for {url}. On a firewalled PC the first download from a new "
                "host or by a new process can be blocked: allow it and run again rather than retrying in a loop."
            )
        break
    part.replace(dest)


def fetch(url: str, dest: Path, sha256: str | None, *, size: int | None = None, allow_unpinned: bool = False) -> Path:
    """dest, downloaded if missing or if its hash does not match the pin."""
    if dest.is_file() and dest.stat().st_size > 0:
        if sha256 is None or sha256_file(dest) == sha256:
            return dest
        log(f"{dest.name}: hash differs from the pin; downloading again")
        dest.unlink()
    if sha256 is None and not allow_unpinned:
        raise SystemExit(f"no SHA-256 pinned for {url}; pass --allow-unpinned once to fetch it and print the hash")
    curl(url, dest, size=size)
    got = sha256_file(dest)
    if sha256 is None:
        log(f"UNPINNED {dest.name} sha256 {got} size {dest.stat().st_size}")
    elif got != sha256:
        bad = dest.with_name(dest.name + ".bad")
        dest.replace(bad)
        raise SystemExit(f"{url}: SHA-256 {got} does not match the pin {sha256} (kept as {bad})")
    return dest


# --------------------------------------------------------------------------
# Audio


def ffmpeg(*args: str) -> None:
    subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-y", *args], check=True)


def to_wav16k(src: Path | list[Path], dst: Path, *, channel: int | None = None) -> Path:
    """16 kHz mono PCM16 WAV. One source: channel `channel` if given, else
    the downmix. Several sources (one per speaker): summed sample by sample,
    without normalisation (each track keeps its level)."""
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp = dst.with_name(dst.stem + ".tmp.wav")
    srcs = src if isinstance(src, list) else [src]
    if len(srcs) == 1:
        af = [] if channel is None else ["-af", f"pan=mono|c0=c{channel}"]
        ffmpeg("-i", str(srcs[0]), "-vn", *af, "-ac", "1", "-ar", str(RATE), "-c:a", "pcm_s16le", str(tmp))
    else:
        ins: list[str] = []
        for s in srcs:
            ins += ["-i", str(s)]
        mix = "".join(f"[{i}:a]" for i in range(len(srcs)))
        mix += f"amix=inputs={len(srcs)}:duration=longest:normalize=0[m]"
        ffmpeg(*ins, "-filter_complex", mix, "-map", "[m]", "-ac", "1", "-ar", str(RATE), "-c:a", "pcm_s16le", str(tmp))
    tmp.replace(dst)
    return dst


def wav_duration(path: Path) -> float:
    with wave.open(str(path)) as w:
        return w.getnframes() / float(w.getframerate())


# --------------------------------------------------------------------------
# References


def strip_punct(tok: str) -> str:
    """A token without leading or trailing punctuation or symbols (inner
    apostrophes and hyphens stay)."""
    a, b = 0, len(tok)
    while a < b and unicodedata.category(tok[a])[0] in "PS":
        a += 1
    while b > a and unicodedata.category(tok[b - 1])[0] in "PS":
        b -= 1
    return tok[a:b]


def fill_missing_times(rows: list[dict]) -> None:
    """Rows in spoken order whose start_time is None get times spread evenly
    between the nearest timed neighbours; their timing becomes "interp"."""
    n = len(rows)
    i = 0
    while i < n:
        if rows[i]["start_time"] is not None:
            i += 1
            continue
        j = i
        while j < n and rows[j]["start_time"] is None:
            j += 1
        left = rows[i - 1]["end_time"] if i > 0 else (rows[j]["start_time"] if j < n else 0.0)
        right = rows[j]["start_time"] if j < n else left
        right = max(right, left)
        k = j - i
        for m in range(k):
            a = left + (right - left) * m / k
            b = left + (right - left) * (m + 1) / k
            rows[i + m].update(start_time=round(a, 3), end_time=round(b, 3), timing="interp")
        i = j


def rows_to_rttm(rows: list[dict], rec: str, max_gap: float) -> list[RttmRow]:
    """Speaker turns from word or segment rows: rows of one speaker are
    merged while the gap to that speaker's previous row is at most max_gap
    seconds (0 merges only rows that touch or overlap)."""
    by_spk: dict[str, list[tuple[float, float]]] = {}
    for r in rows:
        a, b = float(r["start_time"]), float(r["end_time"])
        if b <= a:
            b = a + 0.01
        by_spk.setdefault(str(r["speaker"]), []).append((a, b))
    out: list[RttmRow] = []
    for spk, spans in by_spk.items():
        spans.sort()
        cur_a, cur_b = spans[0]
        for a, b in spans[1:]:
            if a - cur_b <= max_gap:
                cur_b = max(cur_b, b)
            else:
                out.append(RttmRow(rec, cur_a, cur_b - cur_a, spk))
                cur_a, cur_b = a, b
        out.append(RttmRow(rec, cur_a, cur_b - cur_a, spk))
    out.sort(key=lambda r: (r.start, r.speaker))
    return out


def write_refs(root: Path, sid: str, rows: list[dict], rttm: list[RttmRow]) -> tuple[Path, Path]:
    rows = sorted(rows, key=lambda r: (r["start_time"], r["end_time"]))
    for r in rows:
        r["session_id"] = sid
    seg = root / "refs" / f"{sid}.ref.seglst.jsonl"
    rt = root / "refs" / f"{sid}.ref.rttm"
    write_jsonl(seg, rows)
    write_rttm(rt, [RttmRow(sid, r.start, r.dur, r.speaker) for r in rttm])
    return seg, rt


def ref_stats(rows: list[dict], rttm: list[RttmRow]) -> dict:
    """Counts for meta.json: speakers, rows, words, punctuated rows, overlap."""
    ev = []
    for r in rttm:
        ev += [(r.start, 1), (r.end, -1)]
    ev.sort()
    cur, last, speech, ov = 0, 0.0, 0.0, 0.0
    for t, d in ev:
        if cur >= 1:
            speech += t - last
        if cur >= 2:
            ov += t - last
        cur += d
        last = t
    return {
        "speakers": len({r.speaker for r in rttm}),
        "rows": len(rows),
        "words": sum(len(str(r["words"]).split()) for r in rows),
        # rows whose source form carries a leading or trailing mark (. , ? -)
        "rows_with_punctuation": sum(1 for r in rows if r.get("words_pc") and strip_punct(r["words_pc"]) != r["words_pc"]),
        "interp_rows": sum(1 for r in rows if r.get("timing") == "interp"),
        "speech_s": round(speech, 1),
        "overlap_share_of_speech": round(ov / speech, 3) if speech else None,
    }


# --------------------------------------------------------------------------
# Session list and metadata


def rel(p: Path) -> str:
    """Repo-relative path with forward slashes (sessions files resolve
    relative paths from the repo root)."""
    try:
        return p.resolve().relative_to(REPO.resolve()).as_posix()
    except ValueError:
        return p.resolve().as_posix()


def _toml_str(v: str) -> str:
    return json.dumps(v, ensure_ascii=False)


def write_sessions(root: Path, sessions: Iterable[dict]) -> Path:
    """sessions.toml for the set: kind "ref", uem "ref" (the annotated span
    plus 1 s), and the label, group, language and paths of each session."""
    lines = [
        "# Generated by the set's fetch script; included by tools/eval/sessions.public.toml.",
        "# Paths are relative to the repo root.",
        "",
    ]
    for s in sessions:
        s = dict(s)
        sid = s.pop("id")
        lines.append(f"[sessions.{sid}]")
        s.setdefault("kind", "ref")
        s.setdefault("uem", "ref")
        for k in ("label", "kind", "group", "set", "language", "ref_source", "uem", "note"):
            if s.get(k) is not None:
                lines.append(f"{k} = {_toml_str(str(s.pop(k)))}")
        for k in ("audio", "ref_rttm", "ref_seglst"):
            lines.append(f"{k} = {_toml_str(rel(Path(s.pop(k))))}")
        for k, v in s.items():
            if isinstance(v, bool):
                lines.append(f"{k} = {'true' if v else 'false'}")
            elif isinstance(v, (int, float)):
                lines.append(f"{k} = {v}")
            elif isinstance(v, str):
                lines.append(f"{k} = {_toml_str(v)}")
        lines.append("")
    path = root / "sessions.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines), encoding="utf-8", newline="\n")
    return path


def write_meta(root: Path, meta: dict) -> None:
    root.mkdir(parents=True, exist_ok=True)
    (root / "meta.json").write_text(json.dumps(meta, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def prune(paths: Iterable[Path]) -> None:
    for p in paths:
        if p.is_dir():
            shutil.rmtree(p, ignore_errors=True)
        elif p.is_file():
            p.unlink()
        else:
            continue
        log(f"pruned {p}")


def common_args(ap) -> None:
    ap.add_argument("--allow-unpinned", action="store_true", help="fetch files without a pinned SHA-256 and print the hash")
    ap.add_argument("--prune", action="store_true", help="delete download/ once the set's outputs are written")
    ap.add_argument("--rebuild", action="store_true", help="rebuild references and audio from download/")
