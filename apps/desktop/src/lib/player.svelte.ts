// Playback: one <audio crossorigin="anonymous"> per open meeting, fed by the
// asset protocol and routed through a Web Audio gain matrix so a hard-panned
// stereo track (left mic, right loopback) is a mono mix by default, with
// solo-left and solo-right toggles, then a level gain (normalisation from the
// waveform peaks times the volume, `playerGain.ts`) and a safety limiter. The
// file is never fetched into JS.

import {
  EAR_WEIGHTS,
  LIMITER,
  clampVolume,
  playbackGain,
  softClipCurve,
  type ChannelMode,
  type PlaybackGain,
} from "./playerGain";

export type { ChannelMode } from "./playerGain";

export const SPEEDS = [0.75, 1, 1.25, 1.5, 1.75, 2] as const;

export class Player {
  url = $state<string | null>(null);
  error = $state<string | null>(null);
  playing = $state(false);
  currentMs = $state(0);
  durationMs = $state(0);
  rate = $state(1);
  soloLeft = $state(false);
  soloRight = $state(false);
  /** 0..1 (`playback.volume`), times the normalisation. */
  volume = $state(1);
  /** Raise a quiet track so its peak sits near -1 dBFS (`playback.normalize`). */
  normalize = $state(true);
  /** The waveform's `peaks[channel][bucket]` (0..255), for the normalisation. */
  #peaks = $state.raw<readonly (readonly number[])[] | null>(null);

  mode: ChannelMode = $derived(
    this.soloLeft && !this.soloRight ? "left" : this.soloRight && !this.soloLeft ? "right" : "mix",
  );
  /** The gain now applied: normalisation for this mode, volume, limiter. */
  level: PlaybackGain = $derived(playbackGain(this.#peaks, this.mode, this.normalize, this.volume));

  #el: HTMLAudioElement | null = null;
  #ctx: AudioContext | null = null;
  #gains: GainNode[] = [];
  #master: GainNode | null = null;
  #limiter: DynamicsCompressorNode | null = null;
  #clip: WaveShaperNode | null = null;
  #clipCurve: Float32Array<ArrayBuffer> | null = null;
  #raf = 0;
  #cleanup: (() => void) | null = null;

  attach(el: HTMLAudioElement): void {
    this.#el = el;
    const on = (name: string, fn: () => void) => {
      el.addEventListener(name, fn);
      return () => el.removeEventListener(name, fn);
    };
    const offs = [
      on("play", () => {
        this.playing = true;
        this.#tick();
      }),
      on("pause", () => {
        this.playing = false;
        cancelAnimationFrame(this.#raf);
        this.#sync();
      }),
      on("ended", () => (this.playing = false)),
      on("durationchange", () => (this.durationMs = Number.isFinite(el.duration) ? el.duration * 1000 : 0)),
      on("timeupdate", () => {
        if (!this.playing) this.#sync();
      }),
      on("seeked", () => this.#sync()),
      on("error", () => {
        if (this.url) this.error = "The audio file could not be played; it may have been moved, or deleted by audio retention";
      }),
    ];
    this.#cleanup = () => offs.forEach((off) => off());
  }

  destroy(): void {
    cancelAnimationFrame(this.#raf);
    this.#cleanup?.();
    this.#el?.pause();
    void this.#ctx?.close().catch(() => {});
    this.#ctx = null;
    this.#master = null;
    this.#limiter = null;
    this.#clip = null;
  }

  get ready(): boolean {
    return this.url !== null && this.error === null;
  }

  async toggle(): Promise<void> {
    const el = this.#el;
    if (!el || !this.ready) return;
    if (!el.paused) {
      el.pause();
      return;
    }
    this.#ensureGraph();
    if (this.#ctx?.state === "suspended") await this.#ctx.resume();
    try {
      await el.play();
    } catch (e) {
      this.error = `Playback failed (${e instanceof Error ? e.message : String(e)}); press Play again`;
    }
  }

  play(): void {
    if (this.#el?.paused) void this.toggle();
  }

  seek(ms: number): void {
    const el = this.#el;
    if (!el) return;
    const max = this.durationMs > 0 ? this.durationMs : Number.POSITIVE_INFINITY;
    const clamped = Math.max(0, Math.min(ms, max));
    el.currentTime = clamped / 1000;
    this.currentMs = clamped;
  }

  skip(deltaMs: number): void {
    this.seek(this.currentMs + deltaMs);
  }

  setRate(rate: number): void {
    this.rate = rate;
    if (this.#el) this.#el.playbackRate = rate;
  }

  stepRate(direction: 1 | -1): void {
    const i = SPEEDS.findIndex((s) => s >= this.rate);
    const next = SPEEDS[Math.max(0, Math.min(SPEEDS.length - 1, (i < 0 ? 1 : i) + direction))];
    if (next !== undefined) this.setRate(next);
  }

  toggleSolo(side: "left" | "right"): void {
    if (side === "left") {
      this.soloLeft = !this.soloLeft;
      if (this.soloLeft) this.soloRight = false;
    } else {
      this.soloRight = !this.soloRight;
      if (this.soloRight) this.soloLeft = false;
    }
    this.#applyMatrix();
    this.#applyLevel();
  }

  setVolume(volume: number): void {
    this.volume = clampVolume(volume);
    this.#applyLevel();
  }

  setNormalize(on: boolean): void {
    this.normalize = on;
    this.#applyLevel();
  }

  /** The waveform peaks of the track (null: none, no normalisation). */
  setPeaks(peaks: readonly (readonly number[])[] | null): void {
    this.#peaks = peaks;
    this.#applyLevel();
  }

  /** mix -> left -> right -> mix. */
  cycleMode(): void {
    if (this.mode === "mix") this.toggleSolo("left");
    else if (this.mode === "left") this.toggleSolo("right");
    else this.toggleSolo("right");
  }

  #sync(): void {
    if (this.#el) this.currentMs = this.#el.currentTime * 1000;
  }

  #tick = (): void => {
    this.#sync();
    if (this.playing) this.#raf = requestAnimationFrame(this.#tick);
  };

  /** Built on first play (needs a user gesture); before that nothing plays. */
  #ensureGraph(): void {
    if (this.#ctx || !this.#el) return;
    const ctx = new AudioContext();
    const source = ctx.createMediaElementSource(this.#el);
    // Upmix mono sources to L=R so the matrix below treats them the same.
    const up = ctx.createGain();
    up.channelCount = 2;
    up.channelCountMode = "explicit";
    up.channelInterpretation = "speakers";
    const split = ctx.createChannelSplitter(2);
    const merge = ctx.createChannelMerger(2);
    source.connect(up).connect(split);
    // gains[in * 2 + out]: left->left, left->right, right->left, right->right.
    this.#gains = [0, 1, 2, 3].map((k) => {
      const g = ctx.createGain();
      split.connect(g, k >> 1);
      g.connect(merge, 0, k & 1);
      return g;
    });
    // The level gain, then a limiter and a soft clip that only work when the
    // gain raises the track past -1 dBFS (ratio 1 and no curve pass the audio
    // unchanged).
    const master = ctx.createGain();
    const limiter = ctx.createDynamicsCompressor();
    limiter.threshold.value = LIMITER.thresholdDb;
    limiter.knee.value = LIMITER.kneeDb;
    limiter.attack.value = LIMITER.attackS;
    limiter.release.value = LIMITER.releaseS;
    limiter.ratio.value = 1;
    const clip = ctx.createWaveShaper();
    merge.connect(master).connect(limiter).connect(clip).connect(ctx.destination);
    this.#master = master;
    this.#limiter = limiter;
    this.#clip = clip;
    this.#ctx = ctx;
    this.#applyMatrix();
    this.#applyLevel();
  }

  #applyMatrix(): void {
    // gains[in * 2 + out]: each input channel feeds both ears at its weight.
    const [wl, wr] = EAR_WEIGHTS[this.mode];
    const values = [wl, wl, wr, wr];
    this.#gains.forEach((g, i) => g.gain.setTargetAtTime(values[i] ?? 0, g.context.currentTime, 0.01));
  }

  #applyLevel(): void {
    const master = this.#master;
    const limiter = this.#limiter;
    const clip = this.#clip;
    if (!master || !limiter || !clip) return;
    const { gain, limit } = this.level;
    const t = master.context.currentTime;
    master.gain.setTargetAtTime(gain, t, 0.02);
    limiter.ratio.setValueAtTime(limit ? LIMITER.ratio : 1, t);
    if ((clip.curve !== null) !== limit) clip.curve = limit ? (this.#clipCurve ??= softClipCurve()) : null;
  }
}
