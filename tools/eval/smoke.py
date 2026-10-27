"""Smoke test for a session's reference: cut a short clip, run the runtime on
it directly (nemo_direct.py, the app's file-job flags plus the shipped
lookahead and diarizer preset), and score the clip with metrics.py. Shows
that a new set's references parse, line up with the audio and score,
without a full run.

  uv run python -I smoke.py --sessions-file sessions.public.toml --session ami-en2002a-mix --start 600 --dur 180
  uv run python -I smoke.py --sessions-file sessions.public.toml --session ali-r8008_m8013-near --start 300 --dur 180

The ASR defaults to Nemotron EN for English sessions and Nemotron 3.5
otherwise; --language overrides the session's language tag (the EN model
gets none, as in the app). Engine and models: DIARIST_EVAL_ENGINE_DIR /
DIARIST_EVAL_MODELS_DIR, as for nemo_direct.py and run.py (the installed
app's folders work: %LOCALAPPDATA%\\app.diarist.desktop\\engine\\... and
...\\models).

Writes .work/smoke/<session>/: clip.wav, ref.seglst.jsonl, ref.rttm,
words.json (runtime), hyp.seglst.jsonl, hyp.rttm, metrics.json. Prints one
summary line. Run it under the GPU queue when other jobs share the GPU.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import metrics as M  # noqa: E402
import nemo_direct  # noqa: E402
from common import EVAL_DIR, WORK, RttmRow, lang_base, load_sessions, read_rttm, read_seglst, write_jsonl, write_rttm  # noqa: E402
from public_data import ffmpeg  # noqa: E402

SHIPPED = ["--asr.streaming.rnnt_right_context", "13", "--asr.diar.preset", "v3-offline"]
HYP_GAP_S = 1.0  # runtime words of one speaker closer than this form one hypothesis segment


def clip_ref(rows: list[dict], rttm: list[RttmRow], start: float, dur: float, sid: str) -> tuple[list[dict], list[RttmRow]]:
    end = start + dur
    out = []
    for r in rows:
        a, b = float(r["start_time"]), float(r["end_time"])
        if start <= (a + b) / 2 <= end:
            out.append({**r, "session_id": sid, "start_time": round(max(a, start) - start, 3),
                        "end_time": round(min(b, end) - start, 3)})
    turns = []
    for t in rttm:
        a, b = max(t.start, start), min(t.end, end)
        if b > a:
            turns.append(RttmRow(sid, a - start, b - a, t.speaker))
    return out, turns


def hyp_from_words(words: list[dict], sid: str) -> tuple[list[dict], list[dict], list[RttmRow]]:
    """Runtime words -> (segment rows, word rows, RTTM)."""
    wrows = []
    for w in words:
        a = float(w["start"])
        wrows.append({"session_id": sid, "speaker": f"spk{w.get('speaker', 0)}", "start_time": a,
                      "end_time": max(float(w["end"]), a), "words": str(w["word"])})
    wrows.sort(key=lambda r: r["start_time"])
    segs: list[dict] = []
    for r in wrows:
        if segs and segs[-1]["speaker"] == r["speaker"] and r["start_time"] - segs[-1]["end_time"] <= HYP_GAP_S:
            segs[-1]["end_time"] = max(segs[-1]["end_time"], r["end_time"])
            segs[-1]["words"] += " " + r["words"]
        else:
            segs.append(dict(r))
    rttm = [RttmRow(sid, s["start_time"], s["end_time"] - s["start_time"], s["speaker"]) for s in segs]
    return segs, wrows, rttm


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--sessions-file", type=Path, default=EVAL_DIR / "sessions.public.toml")
    ap.add_argument("--session", required=True)
    ap.add_argument("--start", type=float, default=300.0, help="clip start in seconds")
    ap.add_argument("--dur", type=float, default=180.0, help="clip length in seconds (120 to 300 is the intent)")
    ap.add_argument("--asr", help="manifest name or GGUF (default: nemotron-en for English, nemotron-3.5 otherwise)")
    ap.add_argument("--language", help="runtime language tag (default: the session's; none for the EN model)")
    ap.add_argument("--no-shipped-flags", action="store_true", help="omit the shipped lookahead and diarizer preset")
    ap.add_argument("--score-only", action="store_true", help="rescore an existing run")
    a = ap.parse_args()

    s = load_sessions(a.sessions_file)[a.session]
    lang = s.language or "en"
    asr = a.asr or ("nemotron-en" if lang_base(lang) == "en" else "nemotron-3.5")
    tag = a.language or (None if asr == "nemotron-en" else lang)
    out = WORK / "smoke" / a.session
    out.mkdir(parents=True, exist_ok=True)
    clip = out / "clip.wav"
    if not a.score_only:
        ffmpeg("-ss", f"{a.start}", "-t", f"{a.dur}", "-i", str(s.audio), "-ac", "1", "-ar", "16000",
               "-c:a", "pcm_s16le", str(clip))
    ref, turns = clip_ref(read_seglst(s.ref_seglst), read_rttm(s.ref_rttm), a.start, a.dur, a.session)
    write_jsonl(out / "ref.seglst.jsonl", ref)
    write_rttm(out / "ref.rttm", turns)
    if not a.score_only:
        meta = nemo_direct.run(clip.resolve(), asr, out, tag, True, "nemotron-3-diarization",
                               [] if a.no_shipped_flags else SHIPPED)
        if meta["returncode"] != 0:
            print((out / "stderr.txt").read_text(encoding="utf-8")[-3000:], file=sys.stderr)
            raise SystemExit(meta["returncode"])
    else:
        meta = json.loads((out / "run.json").read_text(encoding="utf-8"))
    doc = json.loads((out / "words.json").read_text(encoding="utf-8"))
    segs, words, hyp_rttm = hyp_from_words(doc.get("words", []), a.session)
    write_jsonl(out / "hyp.seglst.jsonl", segs)
    write_rttm(out / "hyp.rttm", hyp_rttm)
    res = M.meeting_metrics(ref, turns, segs, words, hyp_rttm, a.dur, a.session, None, lang)
    res.update(session=a.session, start=a.start, dur=a.dur, asr=asr, language_tag=tag, detected=doc.get("languages"),
               wall_s=meta.get("wall_s"), vram_peak_delta_mib=meta.get("vram_peak_delta_mib"), cmd=meta.get("cmd"))
    (out / "metrics.json").write_text(json.dumps(res, indent=2, default=str, ensure_ascii=False) + "\n", encoding="utf-8")
    pc = lambda x: "-" if x is None else f"{100 * x:.1f}"  # noqa: E731
    print(
        f"{a.session} [{a.start:.0f}+{a.dur:.0f} s] {asr} lang {tag or '-'}: ref {res['ref_words']} words "
        f"{res['ref_speakers']} spk | hyp {res['hyp_words']} words {res['hyp_speakers']} spk | "
        f"DER25 {pc(res.get('der_c025'))} WDER {pc(res.get('wder'))} cpWER {pc(res.get('cpwer'))} "
        f"tcpWER {pc(res.get('tcpwer'))} ORC {pc(res.get('orcwer'))} | wall {meta.get('wall_s')} s"
    )


if __name__ == "__main__":
    main()
