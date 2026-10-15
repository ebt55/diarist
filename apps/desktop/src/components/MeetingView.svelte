<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { api, errorText, type ExportFormat, type Waveform } from "../lib/api";
  import { formatDate, formatDuration } from "../lib/format";
  import type { MeetingDetail } from "../lib/generated/MeetingDetail";
  import type { MeetingId } from "../lib/generated/MeetingId";
  import type { SegmentId } from "../lib/generated/SegmentId";
  import type { SegmentView } from "../lib/generated/SegmentView";
  import type { SpeakerId } from "../lib/generated/SpeakerId";
  import type { SpeakerView } from "../lib/generated/SpeakerView";
  import { SPEAKER_PALETTE as PALETTE } from "../lib/generated/palette";
  import { Player } from "../lib/player.svelte";
  import { app, isTyping } from "../lib/state.svelte";
  import AudioBar from "./AudioBar.svelte";
  import Icon from "./Icon.svelte";
  import Menu, { type MenuItem } from "./Menu.svelte";
  import SpeakerLegend from "./SpeakerLegend.svelte";
  import Transcript from "./Transcript.svelte";

  interface Props {
    meetingId: MeetingId;
    /** Visible and receiving shortcuts (false while Settings is open). */
    active: boolean;
  }
  let { meetingId, active }: Props = $props();

  const FIRST_PAGE = 300;
  const PAGE = 2000;
  const EXPORTS: { format: ExportFormat; label: string }[] = [
    { format: "md", label: "Markdown (.md)" },
    { format: "txt", label: "Plain text (.txt)" },
    { format: "json", label: "JSON (.json)" },
    { format: "srt", label: "Subtitles (.srt)" },
    { format: "vtt", label: "WebVTT (.vtt)" },
  ];

  const player = new Player();
  let audioEl: HTMLAudioElement | undefined = $state();
  let titleEl: HTMLInputElement | undefined = $state();
  let transcript: ReturnType<typeof Transcript> | undefined = $state();

  let detail = $state.raw<MeetingDetail | null>(null);
  let segments = $state.raw<SegmentView[]>([]);
  let loaded = $state(false);
  let fullyLoaded = $state(false);
  let loadError = $state<string | null>(null);
  let waveform = $state.raw<Waveform | null>(null);
  let noAudio = $state<string | null>(null);
  let renaming = $state.raw<{ speakerId: SpeakerId; anchor: SegmentId | null } | null>(null);
  let menu = $state.raw<{ items: MenuItem[]; x: number; y: number; label: string } | null>(null);

  const meeting = $derived(detail?.meeting ?? null);
  const speakers = $derived(detail?.speakers ?? []);
  const roots = $derived(speakers.filter((s) => s.merged_into === null));
  const stereo = $derived(
    waveform ? waveform.channels === 2 : meeting?.source === "live" && player.ready,
  );
  const labelsFull = $derived(roots.filter((s) => s.channel !== "mic").length >= 8);

  onMount(() => {
    if (audioEl) player.attach(audioEl);
    void load();
    return () => player.destroy();
  });

  async function load() {
    const id = meetingId;
    try {
      const [d, first] = await Promise.all([api.getMeeting(id), api.getSegments(id, 0, FIRST_PAGE)]);
      detail = d;
      segments = first;
      loaded = true;
      void loadAudio(id);
      let all = first;
      let page = first;
      let limit = FIRST_PAGE;
      while (page.length === limit) {
        limit = PAGE;
        page = await api.getSegments(id, all.length, PAGE);
        all = all.concat(page);
      }
      if (all.length !== first.length) segments = all;
      fullyLoaded = true;
    } catch (e) {
      loadError = errorText(e);
      loaded = true;
    }
  }

  async function loadAudio(id: MeetingId) {
    try {
      player.url = await api.getAudioUrl(id);
    } catch (e) {
      noAudio = errorText(e);
      return;
    }
    try {
      waveform = await api.getWaveform(id, 1600);
    } catch {
      waveform = null; // Undecodable here: play without peaks.
    }
  }

  async function reload(focus?: SegmentId) {
    if (focus !== undefined) transcript?.setPendingFocus(focus);
    try {
      const [d, segs] = await Promise.all([
        api.getMeeting(meetingId),
        api.getSegments(meetingId, 0, 1_000_000),
      ]);
      detail = d;
      segments = segs;
    } catch (e) {
      app.error(`Could not reload the meeting: ${errorText(e)}`);
    }
  }

  // Open-at-segment requests (search hits).
  $effect(() => {
    const f = app.focus;
    if (!f || f.meetingId !== meetingId || !fullyLoaded) return;
    untrack(() => {
      transcript?.reveal(f.segmentId);
      player.seek(f.startMs);
      app.focus = null;
    });
  });

  // ---- Edits ---------------------------------------------------------------

  async function commitTitle() {
    if (!titleEl || !meeting) return;
    const title = titleEl.value.trim();
    if (title === meeting.title) return;
    try {
      await api.setMeetingTitle(meeting.id, title);
      detail = detail && { ...detail, meeting: { ...detail.meeting, title } };
      app.patchMeeting(meeting.id, { title });
    } catch (e) {
      titleEl.value = meeting.title;
      app.error(`Could not rename the meeting: ${errorText(e)}`);
    }
  }

  async function togglePin() {
    if (!meeting) return;
    const keep = !meeting.audio_keep;
    try {
      await api.setAudioKeep(meeting.id, keep);
      detail = detail && { ...detail, meeting: { ...detail.meeting, audio_keep: keep } };
      app.patchMeeting(meeting.id, { audio_keep: keep });
      app.toast("info", keep ? "Audio pinned: kept regardless of retention" : "Audio unpinned");
    } catch (e) {
      app.error(`Could not change the pin: ${errorText(e)}`);
    }
  }

  async function deleteMeeting() {
    if (!meeting) return;
    const m = meeting;
    const ok = await app.confirm({
      title: "Delete meeting?",
      message: `"${m.title || "Untitled meeting"}" and its transcript will be deleted. Audio files are removed too.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!ok) return;
    try {
      await api.deleteMeeting(m.id);
      app.openId = null;
      await app.refreshMeetings();
      app.toast("info", `Deleted "${m.title || "Untitled meeting"}"`);
    } catch (e) {
      app.error(`Delete failed: ${errorText(e)}`);
    }
  }

  async function exportAs(format: ExportFormat) {
    if (!meeting) return;
    try {
      const path = await api.exportMeeting(meeting.id, format);
      if (path) app.toast("success", `Exported to ${path}`);
    } catch (e) {
      app.error(`Export failed: ${errorText(e)}`);
    }
  }

  function exportMenu(x?: number, y?: number) {
    const btn = document.getElementById("export-btn")?.getBoundingClientRect();
    menu = {
      label: "Export transcript",
      x: x ?? btn?.left ?? 300,
      y: y ?? (btn ? btn.bottom + 4 : 80),
      items: [
        { kind: "header", label: "Export transcript as" },
        ...EXPORTS.map(
          (x, i): MenuItem => ({ kind: "item", label: x.label, hint: String(i + 1), run: () => void exportAs(x.format) }),
        ),
      ],
    };
  }

  function startRename(speakerId: SpeakerId, anchor: SegmentId | null) {
    renaming = { speakerId, anchor };
  }

  async function commitRename(name: string) {
    const r = renaming;
    renaming = null;
    if (!r) return;
    const sp = speakers.find((s) => s.id === r.speakerId);
    if (sp && name.trim() === (sp.display_name ?? "")) {
      refocus(r.anchor);
      return;
    }
    try {
      await api.renameSpeaker(r.speakerId, name, r.anchor);
      await reload(r.anchor ?? undefined);
    } catch (e) {
      app.error(`Rename failed: ${errorText(e)}`);
    }
    refocus(r.anchor);
  }

  function refocus(anchor: SegmentId | null) {
    if (anchor !== null) transcript?.focus();
  }

  async function edit(what: string, fn: () => Promise<void>) {
    try {
      await fn();
      await reload();
    } catch (e) {
      app.error(`${what} failed: ${errorText(e)}`);
    }
  }

  function markReviewed(sp: SpeakerView) {
    void edit("Mark reviewed", () => api.markReviewed(sp.id));
  }

  function speakerMenu(sp: SpeakerView, x: number, y: number) {
    const others = roots.filter((r) => r.id !== sp.id);
    const items: MenuItem[] = [
      { kind: "item", label: `Rename ${sp.name}…`, hint: "R", run: () => startRename(sp.id, null) },
      ...(sp.needs_review
        ? [{ kind: "item", label: "Mark reviewed", run: () => markReviewed(sp) } as MenuItem]
        : []),
      {
        kind: "swatches",
        label: "Colour",
        colors: PALETTE.includes(sp.color as (typeof PALETTE)[number]) ? PALETTE : [...PALETTE, sp.color],
        current: sp.color,
        pick: (c) => void edit("Colour change", () => api.setSpeakerColor(sp.id, c)),
      },
    ];
    if (others.length > 0) {
      items.push({ kind: "header", label: `Merge ${sp.name} into` });
      for (const r of others) {
        items.push({
          kind: "item",
          label: r.name,
          color: r.color,
          run: () => void edit("Merge", () => api.mergeSpeakers(sp.id, r.id)),
        });
      }
    }
    menu = { items, x, y, label: `Actions for ${sp.name}` };
  }

  // ---- Keyboard --------------------------------------------------------------

  function onkeydown(e: KeyboardEvent) {
    if (!active || e.defaultPrevented || menu || renaming || app.confirmRequest || app.helpOpen) return;
    const key = e.key;
    if (e.ctrlKey && !e.shiftKey && !e.altKey) {
      if (key === "e" || key === "E") exportMenu();
      else if (key === "Delete") void deleteMeeting();
      else if (key === "2") transcript?.focus();
      else return;
      e.preventDefault();
      return;
    }
    if (e.ctrlKey || e.altKey || e.metaKey || isTyping(e)) return;
    const onButton = e.target instanceof HTMLButtonElement || e.target instanceof HTMLAnchorElement;
    switch (key) {
      case " ":
        if (onButton) return;
        void player.toggle();
        break;
      case "ArrowLeft":
        player.skip(e.shiftKey ? -15_000 : -5_000);
        break;
      case "ArrowRight":
        player.skip(e.shiftKey ? 15_000 : 5_000);
        break;
      case "[":
        player.stepRate(-1);
        break;
      case "]":
        player.stepRate(1);
        break;
      case "m":
      case "M":
        if (stereo) player.cycleMode();
        break;
      case "F2":
        titleEl?.focus();
        titleEl?.select();
        break;
      case "p":
      case "P":
        void togglePin();
        break;
      case ".":
        transcript?.revealPlayhead();
        break;
      case "g":
      case "G":
        document.querySelector<HTMLElement>(".legend .spk-chip")?.focus();
        break;
      default:
        return;
    }
    e.preventDefault();
  }
</script>

<svelte:window {onkeydown} />

<!-- svelte-ignore a11y_media_has_caption -->
<audio bind:this={audioEl} crossorigin="anonymous" preload="metadata" src={player.url ?? undefined}></audio>

<section class="meeting" aria-label={meeting?.title || "Meeting"}>
  {#if loadError}
    <p class="state error" role="alert">Could not open this meeting: {loadError}</p>
  {:else if !loaded || !meeting}
    <p class="state">Loading…</p>
  {:else}
    <header class="m-head">
      <input
          bind:this={titleEl}
          class="title"
          value={meeting.title}
          placeholder="Untitled meeting"
          aria-label="Meeting title (F2)"
          spellcheck="false"
          onkeydown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
            else if (e.key === "Escape") {
              e.currentTarget.value = meeting.title;
              e.currentTarget.blur();
            } else return;
            e.preventDefault();
            e.stopPropagation();
          }}
          onblur={commitTitle}
        />
      <div class="meta-row">
        <div class="meta">
          <span>{formatDate(meeting.started_at)}</span>
          <span>{formatDuration(meeting.duration_ms)}</span>
          <span>{meeting.source === "live" ? "Recorded live" : "Imported file"}</span>
          <span>{meeting.language}</span>
          {#if detail?.current_run}<span>{detail.current_run.kind.replace("_", "-")} run</span>{/if}
          {#if meeting.status !== "done"}<span class="chip status-{meeting.status}">{meeting.status}</span>{/if}
          {#if (meeting.duration_ms ?? 0) > 3 * 3_600_000}
            <span class="chip warn" title="The diarizer keeps at most 8 labels per recording; past that, labels are reused">Over 3 h</span>
          {/if}
          {#if labelsFull}<span class="chip warn">All 8 speaker labels in use</span>{/if}
        </div>
        <div class="actions">
          <button
            type="button"
            class="btn"
            class:on={meeting.audio_keep}
            aria-pressed={meeting.audio_keep}
            onclick={togglePin}
            title="Keep this meeting's audio regardless of retention (P)"
          >
            <Icon name="pin" /><span class="btn-label">{meeting.audio_keep ? "Pinned" : "Pin audio"}</span>
          </button>
          <button type="button" id="export-btn" class="btn" onclick={() => exportMenu()} title="Export transcript (Ctrl+E)">
            <Icon name="export" /><span class="btn-label">Export</span>
          </button>
          <button type="button" class="btn btn-icon" onclick={deleteMeeting} title="Delete meeting (Ctrl+Delete)" aria-label="Delete meeting">
            <Icon name="trash" />
          </button>
        </div>
      </div>
    </header>

    <SpeakerLegend
      {speakers}
      renaming={renaming?.speakerId ?? null}
      oncommit={commitRename}
      oncancel={() => {
        const r = renaming;
        renaming = null;
        refocus(r?.anchor ?? null);
      }}
      onstartrename={(id) => startRename(id, null)}
      onmenu={speakerMenu}
      onunmerge={(sp) => void edit("Unmerge", () => api.unmergeSpeaker(sp.id))}
      onreviewed={markReviewed}
    />

    <Transcript
      bind:this={transcript}
      {meetingId}
      {segments}
      {roots}
      {player}
      onchanged={reload}
      onrename={startRename}
    />
  {/if}

  <AudioBar {player} {waveform} {stereo} noAudio={loaded ? noAudio : null} fallbackMs={meeting?.duration_ms ?? 0} />
</section>

{#if menu}
  <Menu items={menu.items} x={menu.x} y={menu.y} label={menu.label} onclose={() => (menu = null)} />
{/if}

<style>
  audio {
    display: none;
  }
  .meeting {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-width: 0;
    container: meeting / inline-size;
  }
  .state {
    flex: 1;
    margin: 24px;
    color: var(--fg-muted);
  }
  .state.error {
    color: var(--danger);
  }
  .m-head {
    padding: 10px 14px 6px;
  }
  .meta-row {
    display: flex;
    gap: 10px;
    align-items: center;
    justify-content: space-between;
    margin-top: 2px;
  }
  .title {
    display: block;
    width: calc(100% + 6px);
    margin-left: -6px;
    padding: 2px 6px;
    border: 1px solid transparent;
    border-radius: var(--radius);
    background: transparent;
    font-size: 19px;
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .title:hover {
    border-color: var(--border);
  }
  .title:focus {
    border-color: var(--focus);
    background: var(--bg);
  }
  .actions {
    display: flex;
    flex: none;
    gap: 6px;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 10px;
    align-items: center;
    min-width: 0;
    color: var(--fg-muted);
    font-size: 12px;
  }
  @container meeting (max-width: 620px) {
    .btn-label {
      display: none;
    }
  }
  .warn {
    border-color: var(--warning-border);
    background: var(--warning-bg);
    color: var(--warning-fg);
  }
</style>
