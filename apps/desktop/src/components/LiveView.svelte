<script lang="ts">
  // The meeting while it records: timer, level meters, banners and the live
  // transcript (both channels interleaved by time). Labels stay provisional
  // (dotted chip, dimmed text) until the engine marks the text final.
  import { tick } from "svelte";
  import { formatClock } from "../lib/format";
  import type { SpeakerId } from "../lib/generated/SpeakerId";
  import type { SpeakerView } from "../lib/generated/SpeakerView";
  import {
    defaultSpeakerColor,
    defaultSpeakerName,
    live,
    warningText,
    type LiveRow,
    type Meter,
  } from "../lib/live.svelte";
  import { NO_LINE_NOTE, renameTarget } from "../lib/renameTarget";
  import { app } from "../lib/state.svelte";

  interface Props {
    speakers: SpeakerView[];
    /** Rename the line's speaker; `range` (meeting ms) is the line's time range, the name anchor. */
    onrename: (speaker: SpeakerId, range: { start_ms: number; end_ms: number }) => void;
  }
  let { speakers, onrename }: Props = $props();

  let listEl: HTMLDivElement | undefined = $state();
  let follow = $state(true);
  let cursor = $state(-1);

  const rows = $derived(live.rows);
  const byId = $derived(new Map(speakers.map((s) => [s.id, s])));
  const degraded = $derived(Object.values(live.degraded));
  const warnings = $derived(live.warnings.slice(live.warningsSeen));
  const stopping = $derived(live.phase === "stopping");

  function speakerOf(row: LiveRow): { name: string; color: string } {
    let sp = row.speakerId !== null ? byId.get(row.speakerId) : undefined;
    if (sp?.merged_into != null) sp = byId.get(sp.merged_into) ?? sp;
    return sp ? { name: sp.name, color: sp.color } : { name: defaultSpeakerName(row.label), color: defaultSpeakerColor(row.label) };
  }

  // Keep the newest text in view unless the user scrolled up.
  $effect(() => {
    void rows;
    if (!follow) return;
    void tick().then(() => {
      if (listEl && follow) listEl.scrollTop = listEl.scrollHeight;
    });
  });

  function onscroll() {
    if (!listEl) return;
    follow = listEl.scrollTop + listEl.clientHeight >= listEl.scrollHeight - 40;
  }

  function jumpToLive() {
    follow = true;
    cursor = -1;
    if (listEl) listEl.scrollTop = listEl.scrollHeight;
  }

  function renameRow(row: LiveRow) {
    if (row.speakerId !== null) onrename(row.speakerId, { start_ms: row.start_ms, end_ms: row.end_ms });
  }

  function moveCursor(i: number) {
    if (rows.length === 0) return;
    cursor = Math.max(0, Math.min(rows.length - 1, i));
    follow = false;
    void tick().then(() => document.getElementById(`lrow-${rows[cursor]?.key}`)?.scrollIntoView({ block: "nearest" }));
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.ctrlKey || e.altKey || e.metaKey) return;
    const at = cursor < 0 ? rows.length : cursor;
    switch (e.key) {
      case "ArrowDown":
      case "j":
        moveCursor(at + 1);
        break;
      case "ArrowUp":
      case "k":
        moveCursor(at - 1);
        break;
      case "PageDown":
        moveCursor(at + 10);
        break;
      case "PageUp":
        moveCursor(at - 10);
        break;
      case "Home":
        moveCursor(0);
        break;
      case "End":
        jumpToLive();
        break;
      case "r":
      case "R": {
        // The selected line, else the newest final line of a known speaker
        // (the newest line is usually a provisional "Unknown speaker").
        const target = renameTarget(rows, cursor, (r) => live.isFinal(r));
        if (target) renameRow(target);
        else app.toast("info", NO_LINE_NOTE);
        break;
      }
      default:
        return;
    }
    e.preventDefault();
  }

  /** Where the user is, so the finalized transcript can open at the same place. */
  export function anchor(): { follow: boolean; ms: number | null } {
    if (follow || !listEl) return { follow: true, ms: null };
    const top = listEl.getBoundingClientRect().top;
    for (const el of listEl.querySelectorAll<HTMLElement>(".lrow")) {
      if (el.getBoundingClientRect().bottom > top) return { follow: false, ms: Number(el.dataset.start) };
    }
    return { follow: true, ms: null };
  }

  export function focus() {
    listEl?.focus();
  }

  // ---- meters ----------------------------------------------------------------

  /** dBFS to a 0..100 bar (-60 dB is empty). */
  const pct = (db: number) => Math.max(0, Math.min(100, ((db + 60) / 60) * 100));
  function reading(m: Meter | null): Meter | null {
    return m && live.now - m.at < 1500 ? m : null;
  }
  const meters = $derived([
    { id: "mic", label: "Mic", meter: reading(live.mic), device: live.devices.mic?.name ?? "" },
    { id: "loopback", label: "System", meter: reading(live.loopback), device: live.devices.loopback?.name ?? "" },
  ]);
  const REASONS: Record<string, string> = {
    followed: "now follows the app that plays",
    default_changed: "the default device changed",
    invalidated: "the device was reset",
    resumed: "reopened after sleep",
    requested: "reopened",
  };
</script>

<div class="live">
  <div class="strip">
    <div class="status" role="timer" aria-label="Recording time">
      <span class="rec-dot live" aria-hidden="true"></span>
      <span class="rec-label">{stopping ? "Stopping…" : "Recording"}</span>
      <span class="timer mono">{formatClock(live.elapsedMs, live.elapsedMs >= 3_600_000)}</span>
    </div>
    <div class="meters">
      {#each meters as m (m.id)}
        {@const peak = m.meter ? pct(m.meter.peak) : 0}
        {@const rms = m.meter ? pct(m.meter.rms) : 0}
        <div class="meter" title={m.device ? `${m.label}: ${m.device}` : m.label}>
          <span class="m-label">{m.label}</span>
          <div
            class="m-bar"
            role="meter"
            aria-label="{m.label} level"
            aria-valuemin={-60}
            aria-valuemax={0}
            aria-valuenow={m.meter ? Math.round(m.meter.peak) : -60}
          >
            <div class="m-rms" class:hot={m.meter && m.meter.peak > -3} style="width: {rms}%"></div>
            <div class="m-peak" style="left: calc({peak}% - 2px)" class:hidden={!m.meter}></div>
          </div>
          <span class="m-db mono">{m.meter ? `${Math.round(m.meter.peak)} dB` : "–"}</span>
          {#if m.meter && Math.abs(m.meter.gain) >= 0.5}
            <span class="chip gain mono" title="Automatic gain on this channel; peak before it {Math.round(m.meter.rawPeak)} dB"
              >+{m.meter.gain.toFixed(0)} dB</span
            >
          {/if}
          <span class="m-dev">{m.device}</span>
        </div>
      {/each}
    </div>
    {#if live.deviceNotice}
      <span class="chip notice" title={live.deviceNotice.name}>
        {live.deviceNotice.channel === "mic" ? "Mic" : "System audio"}: {live.deviceNotice.name}
        {REASONS[live.deviceNotice.reason] ? `(${REASONS[live.deviceNotice.reason]})` : ""}
      </span>
    {/if}
    <button type="button" class="btn btn-rec stop" disabled={stopping} onclick={() => live.stop()} title="Stop recording (Ctrl+.)">
      <span class="sq" aria-hidden="true"></span>{stopping ? "Stopping…" : "Stop"}
    </button>
  </div>

  {#if degraded.length > 0}
    <div class="banner warn" role="status">
      <strong>Transcribing paused, still recording.</strong>
      <span>
        {degraded.map((d) => `${d.channel === "mic" ? "Mic" : "System audio"}: ${d.reason}`).join(" · ")}. The
        transcript is completed from the recording when you stop.
      </span>
    </div>
  {/if}
  {#if warnings.length > 0}
    <div class="banner warn" role="alert">
      <div class="w-list">
        {#each warnings as w, i (i)}<span>{warningText(w)}</span>{/each}
      </div>
      <button type="button" class="link" onclick={() => live.dismissWarnings()}>Dismiss</button>
    </div>
  {/if}

  <div class="list-wrap">
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      id="live-transcript"
      class="list"
      role="listbox"
      tabindex="0"
      aria-label="Live transcript (R renames the speaker, End follows)"
      aria-activedescendant={rows[cursor] ? `lrow-${rows[cursor]?.key}` : undefined}
      bind:this={listEl}
      {onscroll}
      {onkeydown}
    >
      {#each rows as row, i (row.key)}
        {@const sp = speakerOf(row)}
        {@const final = live.isFinal(row)}
        <div
          id="lrow-{row.key}"
          class="lrow"
          class:cursor={i === cursor}
          data-channel={row.channel}
          data-start={row.start_ms}
          role="option"
          tabindex="-1"
          aria-selected={i === cursor}
          style="--spk: {sp.color}"
          onclick={() => {
            cursor = i;
            follow = false;
            listEl?.focus({ preventScroll: true });
          }}
          onkeydown={() => {}}
          ondblclick={() => renameRow(row)}
        >
          <span class="ts mono">{formatClock(row.start_ms)}</span>
          <div class="body">
            <span class="chip spk" class:provisional={!final} title={final ? "Label from the live diarizer (checked again when you stop)" : "Provisional label"}>{sp.name}</span>
            <span class="text" class:tentative={!final}>{row.text}</span>
          </div>
        </div>
      {:else}
        <p class="empty">
          Listening… Words appear here within a second or two of being spoken. The mic is labelled
          <strong>Me</strong>; voices from the system audio get provisional speaker labels.
        </p>
      {/each}
    </div>
    {#if !follow && rows.length > 0}
      <button type="button" class="btn jump" onclick={jumpToLive}>↓ Live <kbd>End</kbd></button>
    {/if}
  </div>
</div>

<style>
  .live {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }
  .strip {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 16px;
    align-items: center;
    margin: 0 14px 8px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-sidebar);
  }
  .status {
    display: flex;
    gap: 8px;
    align-items: center;
    color: var(--rec);
    font-weight: 650;
  }
  .rec-dot {
    width: 11px;
    height: 11px;
  }
  .timer {
    min-width: 54px;
    font-size: 17px;
  }
  .meters {
    display: flex;
    flex: 1;
    flex-wrap: wrap;
    gap: 6px 18px;
    min-width: 260px;
  }
  .meter {
    display: grid;
    grid-template-columns: 48px minmax(80px, 160px) 50px auto;
    gap: 2px 8px;
    align-items: center;
    font-size: 12px;
  }
  .m-label {
    color: var(--fg-muted);
    font-weight: 600;
  }
  .m-bar {
    position: relative;
    height: 8px;
    overflow: hidden;
    border-radius: 4px;
    background: var(--bg-hover);
  }
  .m-rms {
    height: 100%;
    border-radius: 4px;
    background: var(--ok-fg);
    transition: width 0.1s linear;
  }
  .m-rms.hot {
    background: var(--rec);
  }
  .m-peak {
    position: absolute;
    top: 0;
    width: 2px;
    height: 100%;
    background: var(--fg);
    transition: left 0.1s linear;
  }
  .m-peak.hidden {
    display: none;
  }
  .m-db {
    color: var(--fg-muted);
    text-align: right;
  }
  .m-dev {
    grid-column: 2 / -1;
    overflow: hidden;
    color: var(--fg-faint);
    font-size: 11px;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .gain {
    border-color: var(--accent-border);
    color: var(--accent);
  }
  .notice {
    max-width: 320px;
    overflow: hidden;
    border-color: var(--accent-border);
    color: var(--accent);
    text-overflow: ellipsis;
  }
  .stop {
    margin-left: auto;
  }
  .sq {
    display: inline-block;
    width: 9px;
    height: 9px;
    border-radius: 1px;
    background: currentColor;
  }
  .banner {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    margin: 0 14px 8px;
    padding: 7px 10px;
    border: 1px solid var(--warning-border);
    border-radius: var(--radius);
    background: var(--warning-bg);
    color: var(--warning-fg);
  }
  .banner strong {
    white-space: nowrap;
  }
  .w-list {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 2px;
  }
  .list-wrap {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  .list {
    height: 100%;
    padding: 4px 0 24px;
    overflow-y: auto;
    outline: none;
  }
  .lrow {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 10px;
    margin: 0 10px 0 8px;
    padding: 4px 8px 4px 6px;
    border-left: 3px solid var(--spk);
    border-radius: 0 4px 4px 0;
    line-height: 1.5;
  }
  .lrow:hover {
    background: var(--bg-hover-soft);
  }
  .lrow.cursor {
    box-shadow: inset 0 0 0 1px var(--border-strong);
  }
  .list:focus-visible .lrow.cursor {
    box-shadow: inset 0 0 0 2px var(--focus);
  }
  .ts {
    padding: 1px 2px;
    color: var(--fg-muted);
    font-size: 12px;
    line-height: 21px;
  }
  .body {
    min-width: 0;
  }
  .spk {
    margin-right: 6px;
    border-color: color-mix(in srgb, var(--spk) 55%, transparent);
    background: color-mix(in srgb, var(--spk) 16%, transparent);
    font-weight: 600;
    vertical-align: 1px;
  }
  .spk.provisional {
    border-style: dotted;
    background: transparent;
  }
  .text {
    overflow-wrap: anywhere;
  }
  .text.tentative {
    color: var(--fg-muted);
  }
  .empty {
    max-width: 520px;
    margin: 18px 22px;
    color: var(--fg-muted);
  }
  .jump {
    position: absolute;
    right: 22px;
    bottom: 14px;
    box-shadow: var(--shadow);
  }
</style>
