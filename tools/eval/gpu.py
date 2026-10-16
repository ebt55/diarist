"""GPU memory sampling for the eval harness (dev-only).

Per-process VRAM is not visible on WDDM (research/08 pitfall 10), so peak VRAM
is the largest total `memory.used` that nvidia-smi reports during a run minus
the reading just before it. Another process starting on the GPU during a run
inflates the number; wait_idle() refuses to start while another GPU job runs.
"""

from __future__ import annotations

import subprocess
import sys
import threading
import time

# The idle desktop on the reference machine holds about 1.2 GB, but browser
# tabs and other agents' app windows push it to 1.8 GB or more for hours, with
# only short bursts of utilisation. A transcription job holds 1.4 to 2 GB more
# and keeps the GPU busy, so "busy" means: memory above the idle level and a
# sustained utilisation, or memory far above it.
DEFAULT_IDLE_MIB = 1600
BUSY_UTIL_PCT = 25
HARD_LIMIT_EXTRA_MIB = 1200


def used_mib() -> int | None:
    """Total memory.used of GPU 0 in MiB, or None without nvidia-smi."""
    try:
        out = subprocess.run(
            ["nvidia-smi", "--query-gpu=memory.used", "--format=csv,noheader,nounits"],
            capture_output=True, text=True, timeout=30,
        ).stdout
    except (OSError, subprocess.TimeoutExpired):
        return None
    vals = [int(x) for x in out.split() if x.strip().isdigit()]
    return vals[0] if vals else None


def utilisation_median(samples: int = 10, interval_s: float = 0.5) -> int | None:
    vals = []
    for _ in range(samples):
        try:
            out = subprocess.run(
                ["nvidia-smi", "--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"],
                capture_output=True, text=True, timeout=30,
            ).stdout
        except (OSError, subprocess.TimeoutExpired):
            return None
        vals += [int(x) for x in out.split() if x.strip().isdigit()][:1]
        time.sleep(interval_s)
    return sorted(vals)[len(vals) // 2] if vals else None


def wait_idle(limit_mib: int = DEFAULT_IDLE_MIB, wait_s: float = 60, tries: int = 60) -> int | None:
    """Blocks while another GPU job seems to run: memory above limit_mib with
    a median utilisation of BUSY_UTIL_PCT or more over 5 s, or memory above
    limit_mib + HARD_LIMIT_EXTRA_MIB. Returns the memory reading."""
    for _ in range(tries):
        u = used_mib()
        if u is None or u <= limit_mib:
            return u
        util = utilisation_median()
        if u <= limit_mib + HARD_LIMIT_EXTRA_MIB and (util is None or util < BUSY_UTIL_PCT):
            print(f"GPU holds {u} MiB but is idle (median utilisation {util} %); starting", file=sys.stderr, flush=True)
            return u
        print(f"GPU busy: {u} MiB used, median utilisation {util} %; waiting {wait_s:.0f} s", file=sys.stderr, flush=True)
        time.sleep(wait_s)
    raise SystemExit(f"GPU still busy after {tries} tries")


class Sampler:
    """`with Sampler() as s:` samples memory.used every interval_ms in the
    background; s.base is the reading before the block, s.samples holds
    (unix time, MiB) pairs."""

    def __init__(self, interval_ms: int = 200):
        self.interval_ms = interval_ms
        self.base: int | None = None
        self.samples: list[tuple[float, int]] = []
        self._proc: subprocess.Popen | None = None
        self._thread: threading.Thread | None = None

    def __enter__(self) -> "Sampler":
        self.base = used_mib()
        try:
            self._proc = subprocess.Popen(
                ["nvidia-smi", "--query-gpu=memory.used", "--format=csv,noheader,nounits", f"--loop-ms={self.interval_ms}"],
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
            )
        except OSError:
            self._proc = None
            return self
        self._thread = threading.Thread(target=self._read, daemon=True)
        self._thread.start()
        return self

    def _read(self) -> None:
        assert self._proc and self._proc.stdout
        for line in self._proc.stdout:
            line = line.strip().split(",")[0].strip()
            if line.isdigit():
                self.samples.append((time.time(), int(line)))

    def __exit__(self, *exc) -> None:
        if self._proc:
            self._proc.terminate()
            try:
                self._proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self._proc.kill()
        if self._thread:
            self._thread.join(timeout=5)

    def peak(self, t0: float | None = None, t1: float | None = None) -> int | None:
        vals = [m for t, m in self.samples if (t0 is None or t >= t0) and (t1 is None or t <= t1)]
        return max(vals) if vals else None

    def peak_delta(self, t0: float | None = None, t1: float | None = None) -> int | None:
        p = self.peak(t0, t1)
        return None if p is None or self.base is None else p - self.base
