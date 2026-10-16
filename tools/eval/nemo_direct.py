"""Run `nemo-speech transcribe` directly with the flags diarist-core uses for a
file job, plus extra flags, timing it and sampling VRAM. For spikes on runtime
options that the app does not pass yet (`--pnc-model`, `--speech-context`, ...).

  uv run python -I nemo_direct.py .work/clips/ami_050-350.wav --asr nemotron-3.5 \
      --out .work/spikes/pnc/n35_ami --language en -- --pnc-model .work/pnc/pnc.q8_0.gguf

Writes <out>/words.json (the runtime's JSON), <out>/stderr.txt and
<out>/run.json (command line with paths reduced to placeholders, wall time,
VRAM). --asr / --diar take a manifest name (manifests/models.json) or a GGUF
path. Engine and models folders: DIARIST_EVAL_ENGINE_DIR /
DIARIST_EVAL_MODELS_DIR, as for run.py.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import REPO  # noqa: E402
import gpu  # noqa: E402

ENGINE_DIR = Path(
    os.environ.get(
        "DIARIST_EVAL_ENGINE_DIR", REPO / "engine" / "nemo-speech" / "nemo-speech-0.2.0-windows-x86_64-cuda"
    )
)
MODELS_DIR = Path(os.environ.get("DIARIST_EVAL_MODELS_DIR", REPO / "models"))


def model_path(name_or_path: str) -> Path:
    p = Path(name_or_path)
    if p.suffix.lower() == ".gguf":
        return p.resolve()
    manifest = json.loads((REPO / "manifests" / "models.json").read_text(encoding="utf-8"))
    m = next((x for x in manifest if x["name"] == name_or_path), None)
    if not m:
        raise SystemExit(f"unknown model {name_or_path!r}")
    return MODELS_DIR / Path(*m["repo"].split("/")) / m["revision"] / m["file"]


def placeholder(a: str) -> str:
    low = a.lower()
    if low.endswith(".wav"):
        return "<wav>"
    if low.endswith(".gguf"):
        return "<" + re.split(r"[\\/]", a)[-1] + ">"
    if low.endswith(".json"):
        return "<out>"
    return a


def run(wav: Path, asr: str, out: Path, language: str | None, diarize: bool, diar: str, extra: list[str],
        idle_mib: int = gpu.DEFAULT_IDLE_MIB) -> dict:
    out.mkdir(parents=True, exist_ok=True)
    exe = ENGINE_DIR / "bin" / "nemo-speech.exe"
    cmd = [str(exe), "transcribe", str(wav), "--model", str(model_path(asr))]
    if language and language != "auto-omit":
        cmd += ["--language", language]
    cmd += ["--stream", "--endpointing"]
    if diarize:
        cmd += ["--diarize", "--diar-model", str(model_path(diar))]
    cmd += ["--device", "cuda:0", "--json", "--output", str(out / "words.json"), "--force", *extra]
    env = dict(os.environ)
    sysroot = os.environ.get("SystemRoot", r"C:\Windows")
    env["PATH"] = f"{ENGINE_DIR / 'bin'};{sysroot}\\System32;{sysroot}"
    env["NEMO_SPEECH_MODEL_DIR"] = str(MODELS_DIR)
    gpu.wait_idle(idle_mib)
    t0 = time.perf_counter()
    with gpu.Sampler() as mon:
        p = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", env=env)
    wall = time.perf_counter() - t0
    (out / "stderr.txt").write_text(p.stderr or "", encoding="utf-8")
    meta = {
        "cmd": "nemo-speech " + " ".join(placeholder(a) for a in cmd[1:]),
        "returncode": p.returncode,
        "wall_s": round(wall, 2),
        "vram_base_mib": mon.base,
        "vram_peak_delta_mib": mon.peak_delta(),
        "asr": asr,
        "language": language,
        "extra": extra,
    }
    if (out / "words.json").is_file() and p.returncode == 0:
        doc = json.loads((out / "words.json").read_text(encoding="utf-8"))
        meta["duration"] = doc.get("duration")
        meta["languages"] = doc.get("languages")
        meta["words"] = len(doc.get("words", []))
    (out / "run.json").write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
    return meta


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("wav", type=Path)
    ap.add_argument("--asr", default="nemotron-3.5", help="manifest name or GGUF path")
    ap.add_argument("--diar", default="nemotron-3-diarization")
    ap.add_argument("--no-diarize", action="store_true")
    ap.add_argument("--language", help="passed as --language (omit for none)")
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("extra", nargs="*", help="extra nemo-speech flags (after --)")
    a = ap.parse_args()
    meta = run(a.wav.resolve(), a.asr, a.out, a.language, not a.no_diarize, a.diar, a.extra)
    print(json.dumps(meta, indent=2))
    if meta["returncode"] != 0:
        err = (a.out / "stderr.txt").read_text(encoding="utf-8")
        print(err[-3000:], file=sys.stderr)
        raise SystemExit(meta["returncode"])


if __name__ == "__main__":
    main()
