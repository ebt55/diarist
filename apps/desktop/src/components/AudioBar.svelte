<script lang="ts">
  import type { Waveform } from "../lib/api";
  import { formatClock } from "../lib/format";
  import { SPEEDS, type Player } from "../lib/player.svelte";
  import { VOLUME_STEP, clampVolume } from "../lib/playerGain";
  import { app } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  interface Props {
    player: Player;
    waveform: Waveform | null;
    /** Show the mic/loopback toggles (stereo playback track). */
    stereo: boolean;
    noAudio: string | null;
    /** Meeting length, shown when the audio has no duration (or is missing). */
    fallbackMs: number;
  }
  let { player, waveform, stereo, noAudio, fallbackMs }: Props = $props();

  let track: HTMLDivElement | undefined = $state();
  let base: HTMLCanvasElement | undefined = $state();
  let played: HTMLCanvasElement | undefined = $state();
  let width = $state(0);
  let height = $state(0);
  let dragging = false;
  let scheme = $state(0);

  $effect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const bump = () => scheme++;
    mq.addEventListener("change", bump);
    return () => mq.removeEventListener("change", bump);
  });

  const duration = $derived(player.durationMs || waveform?.duration_ms || 0);
  const shownDuration = $derived(duration || fallbackMs);
  const pct = $derived(duration > 0 ? Math.min(100, (player.currentMs / duration) * 100) : 0);
  const long = $derived(duration >= 3_600_000);

  $effect(() => {
    if (!track) return;
    const ro = new ResizeObserver(([entry]) => {
      if (!entry) return;
      width = Math.round(entry.contentRect.width);
      height = Math.round(entry.contentRect.height);
    });
    ro.observe(track);
    return () => ro.disconnect();
  });

  function cssVar(name: string): string {
    return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  }

  /** Left channel (mic) up, right channel (loopback) down; dims a muted side. */
  function draw(canvas: HTMLCanvasElement, color: string, dimmed: string) {
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.max(1, width * dpr);
    canvas.height = Math.max(1, height * dpr);
    const ctx = canvas.getContext("2d");
    if (!ctx || !waveform) return;
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, width, height);
    const mid = height / 2;
    const rows = waveform.peaks;
    const n = rows[0]?.length ?? 0;
    if (n === 0) return;
    const bar = 2;
    const gap = 1;
    const cols = Math.floor(width / (bar + gap));
    const two = rows.length > 1;
    for (let c = 0; c < cols; c++) {
      const from = Math.floor((c * n) / cols);
      const to = Math.max(from + 1, Math.floor(((c + 1) * n) / cols));
      for (let ch = 0; ch < (two ? 2 : 1); ch++) {
        const row = rows[ch] ?? [];
        let peak = 0;
        for (let k = from; k < to; k++) peak = Math.max(peak, row[k] ?? 0);
        const muted = (ch === 0 && player.mode === "right") || (ch === 1 && player.mode === "left");
        ctx.fillStyle = muted ? dimmed : color;
        const h = Math.max(1, (peak / 255) * (two ? mid - 1 : height / 2 - 1));
        const x = c * (bar + gap);
        if (!two) ctx.fillRect(x, mid - h, bar, h * 2);
        else if (ch === 0) ctx.fillRect(x, mid - h, bar, h);
        else ctx.fillRect(x, mid + 1, bar, h);
      }
    }
  }

  $effect(() => {
    // Redraw on size, data, theme or channel mode changes.
    void [width, height, waveform, player.mode, app.theme, scheme];
    if (!base || !played || width === 0) return;
    draw(base, cssVar("--wave"), cssVar("--wave-dim"));
    draw(played, cssVar("--wave-played"), cssVar("--wave-dim"));
  });

  function seekFromPointer(e: PointerEvent) {
    if (!track || duration <= 0) return;
    const r = track.getBoundingClientRect();
    const f = Math.max(0, Math.min(1, (e.clientX - r.left) / r.width));
    player.seek(f * duration);
  }

  function onpointerdown(e: PointerEvent) {
    if (!player.ready) return;
    dragging = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    seekFromPointer(e);
  }

  const volumePct = $derived(Math.round(player.volume * 100));
  const raisedDb = $derived(player.normalize ? Math.round(player.level.normalizeDb) : 0);
  const levelTitle = $derived(
    !player.normalize
      ? "Auto level off: the recording plays as stored"
      : raisedDb > 0
        ? `Auto level: this recording plays ${raisedDb} dB louder than stored, with a limiter on the loudest moments. The file and the waveform do not change`
        : "Auto level: this recording is already loud enough, it plays as stored",
  );

  function saveVolume() {
    void app.setSetting("playback.volume", String(player.volume));
  }

  function toggleNormalize() {
    const on = !player.normalize;
    player.setNormalize(on);
    void app.setSetting("playback.normalize", on ? "true" : "false");
  }

  /** The slider takes its own keys: the meeting's arrow shortcuts would seek instead. */
  function onvolumekey(e: KeyboardEvent) {
    const steps: Record<string, number> = { ArrowUp: 1, ArrowRight: 1, ArrowDown: -1, ArrowLeft: -1, PageUp: 4, PageDown: -4 };
    let next: number;
    if (e.key === "Home") next = 0;
    else if (e.key === "End") next = 1;
    else if (e.key in steps) next = player.volume + (steps[e.key] ?? 0) * VOLUME_STEP;
    else return;
    e.preventDefault();
    player.setVolume(clampVolume(Math.round(next / VOLUME_STEP) * VOLUME_STEP));
    saveVolume();
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Home") player.seek(0);
    else if (e.key === "End") player.seek(duration);
    else if (e.key === "PageUp") player.skip(60_000);
    else if (e.key === "PageDown") player.skip(-60_000);
    else return;
    e.preventDefault();
  }
</script>

<div class="audiobar" class:disabled={!player.ready}>
  <button
    type="button"
    class="btn btn-icon play"
    onclick={() => player.toggle()}
    disabled={!player.ready}
    aria-label={player.playing ? "Pause (Space)" : "Play (Space)"}
    title={player.playing ? "Pause (Space)" : "Play (Space)"}
  >
    <Icon name={player.playing ? "pause" : "play"} />
  </button>
  <span class="time mono">
    {formatClock(player.currentMs, long)} <span class="muted">/ {formatClock(shownDuration, long)}</span>
  </span>

  <div
    class="track"
    bind:this={track}
    role="slider"
    tabindex={player.ready ? 0 : -1}
    aria-label="Seek (Left and Right arrows skip 5 s)"
    aria-valuemin="0"
    aria-valuemax={Math.round(duration / 1000)}
    aria-valuenow={Math.round(player.currentMs / 1000)}
    aria-valuetext={formatClock(player.currentMs, long)}
    aria-disabled={!player.ready}
    {onpointerdown}
    onpointermove={(e) => dragging && seekFromPointer(e)}
    onpointerup={() => (dragging = false)}
    {onkeydown}
  >
    {#if noAudio}
      <span class="no-audio">{noAudio}</span>
    {:else if waveform}
      <canvas bind:this={base} class="wave"></canvas>
      <canvas bind:this={played} class="wave" style="clip-path: inset(0 {100 - pct}% 0 0)"></canvas>
    {:else}
      <div class="line"><div class="line-fill" style="width: {pct}%"></div></div>
    {/if}
    {#if !noAudio}<div class="head" style="left: {pct}%"></div>{/if}
  </div>

  <label class="speed" title="Speed ([ and ])">
    <span class="sr-only">Playback speed</span>
    <select
      value={player.rate}
      disabled={!player.ready}
      onchange={(e) => player.setRate(Number((e.currentTarget as HTMLSelectElement).value))}
    >
      {#each SPEEDS as s (s)}<option value={s}>{s}×</option>{/each}
    </select>
  </label>

  <div class="level">
    <button
      type="button"
      class="btn auto"
      aria-pressed={player.normalize}
      disabled={!player.ready}
      onclick={toggleNormalize}
      title={levelTitle}
    >
      Auto level{#if raisedDb > 0}<span class="db mono">+{raisedDb} dB</span>{/if}
    </button>
    <label class="volume" title="Volume {volumePct}%">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
        stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">
        <path d={player.volume === 0 ? "M4 9h4l5-4v14l-5-4H4zM17 10l4 4m0-4-4 4" : "M4 9h4l5-4v14l-5-4H4zM16.5 9a4 4 0 0 1 0 6M19 6.5a7.5 7.5 0 0 1 0 11"} />
      </svg>
      <span class="sr-only">Volume</span>
      <input
        type="range"
        min="0"
        max="1"
        step={VOLUME_STEP}
        value={player.volume}
        disabled={!player.ready}
        aria-valuetext="{volumePct} percent"
        oninput={(e) => player.setVolume(Number((e.currentTarget as HTMLInputElement).value))}
        onchange={saveVolume}
        onkeydown={onvolumekey}
      />
    </label>
  </div>

  {#if stereo}
    <div class="seg-control" role="group" aria-label="Channels (M cycles)">
      <button type="button" aria-pressed={player.mode === "mix"} disabled={!player.ready}
        onclick={() => { if (player.soloLeft) player.toggleSolo("left"); if (player.soloRight) player.toggleSolo("right"); }}
        title="Both channels as a mono mix">Mix</button>
      <button type="button" aria-pressed={player.soloLeft} disabled={!player.ready}
        onclick={() => player.toggleSolo("left")} title="Only the left channel (your microphone)">Mic</button>
      <button type="button" aria-pressed={player.soloRight} disabled={!player.ready}
        onclick={() => player.toggleSolo("right")} title="Only the right channel (system audio)">System</button>
    </div>
  {/if}
</div>

<style>
  .audiobar {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 10px;
    align-items: center;
    padding: 6px 14px;
    border-top: 1px solid var(--border);
    background: var(--bg-sidebar);
  }
  .play {
    width: 34px;
    height: 34px;
    border-radius: 50%;
    background: var(--accent);
    color: var(--accent-fg);
  }
  .play:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .play:disabled {
    background: var(--bg-hover);
    color: var(--fg-faint);
  }
  .time {
    min-width: 92px;
    font-size: 12px;
    white-space: nowrap;
  }
  .muted {
    color: var(--fg-muted);
  }
  .track {
    position: relative;
    flex: 1;
    height: 40px;
    min-width: 80px;
    border-radius: 4px;
    cursor: pointer;
    touch-action: none;
  }
  .disabled .track {
    cursor: default;
  }
  .track:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: 2px;
  }
  .wave {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
  }
  .line {
    position: absolute;
    top: 50%;
    right: 0;
    left: 0;
    height: 4px;
    margin-top: -2px;
    border-radius: 2px;
    background: var(--wave);
  }
  .line-fill {
    height: 100%;
    border-radius: 2px;
    background: var(--wave-played);
  }
  .head {
    position: absolute;
    top: 2px;
    bottom: 2px;
    width: 2px;
    margin-left: -1px;
    background: var(--fg);
    pointer-events: none;
  }
  .no-audio {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--fg-muted);
    font-size: 12px;
  }
  .speed select {
    padding: 3px 4px;
  }
  .level {
    display: inline-flex;
    gap: 8px;
    align-items: center;
  }
  .auto {
    padding: 3px 8px;
    font-size: 12px;
  }
  .auto[aria-pressed="true"] {
    color: var(--accent);
  }
  .auto[aria-pressed="false"] {
    color: var(--fg-muted);
  }
  .db {
    font-size: 11px;
  }
  .volume {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    color: var(--fg-muted);
  }
  .volume input {
    width: 76px;
    accent-color: var(--accent);
  }
  @container meeting (max-width: 620px) {
    .track {
      flex-basis: 100%;
      order: -1;
      height: 32px;
    }
    .speed {
      margin-left: auto;
    }
  }
</style>
