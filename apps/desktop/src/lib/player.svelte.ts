// Playback: one <audio crossorigin="anonymous"> per open meeting, fed by the
// asset protocol and routed through a Web Audio gain matrix so a hard-panned
// stereo track (left mic, right loopback) is a mono mix by default, with
// solo-left and solo-right toggles. The file is never fetched into JS.

export type ChannelMode = "mix" | "left" | "right";

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

  mode: ChannelMode = $derived(
    this.soloLeft && !this.soloRight ? "left" : this.soloRight && !this.soloLeft ? "right" : "mix",
  );

  #el: HTMLAudioElement | null = null;
  #ctx: AudioContext | null = null;
  #gains: GainNode[] = [];
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
    merge.connect(ctx.destination);
    this.#ctx = ctx;
    this.#applyMatrix();
  }

  #applyMatrix(): void {
    const m: Record<ChannelMode, number[]> = {
      mix: [0.5, 0.5, 0.5, 0.5],
      left: [1, 1, 0, 0],
      right: [0, 0, 1, 1],
    };
    const values = m[this.mode];
    this.#gains.forEach((g, i) => g.gain.setTargetAtTime(values[i] ?? 0, g.context.currentTime, 0.01));
  }
}
