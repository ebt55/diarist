<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { api, errorText, type ExportFormat, type Waveform } from "../lib/api";
  import { formatClock, formatDate, formatDuration } from "../lib/format";
  import type { MeetingDetail } from "../lib/generated/MeetingDetail";
  import type { MeetingId } from "../lib/generated/MeetingId";
  import type { SegmentId } from "../lib/generated/SegmentId";
  import type { SegmentView } from "../lib/generated/SegmentView";
  import type { SpeakerId } from "../lib/generated/SpeakerId";
  import type { SpeakerView } from "../lib/generated/SpeakerView";
  import { SPEAKER_PALETTE as PALETTE } from "../lib/generated/palette";
  import { live } from "../lib/live.svelte";
  import { Player } from "../lib/player.svelte";
  import { app, isTyping } from "../lib/state.svelte";
  import AudioBar from "./AudioBar.svelte";
  import Icon from "./Icon.svelte";
  import LiveView from "./LiveView.svelte";
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
  const STAGE_TEXT: Record<string, string> = {
    queued: "Waiting for the GPU",
    decoding: "Reading the recording",
    transcribing: "Transcribing the recorded channels again",
    diarizing: "Finding speakers",
    saving: "Saving the final transcript",
  };

  const player = new Player();
  let audioEl: HTMLAudioElement | undefined = $state();
  let titleEl: HTMLInputElement | undefined = $state();
  let transcript: ReturnType<typeof Transcript> | undefined = $state();
  let liveView: ReturnType<typeof LiveView> | undefined = $state();

  let detail = $state.raw<MeetingDetail | null>(null);
  let segments = $state.raw<SegmentView[]>([]);
  let loaded = $state(false);
  let fullyLoaded = $state(false);
  let loadError = $state<string | null>(null);
  let waveform = $state.raw<Waveform | null>(null);
  let noAudio = $state<string | null>(null);
  /** The speaker being renamed and its name anchor: a segment, or a live line's time range. */
  interface Rename {
    speakerId: SpeakerId;
    anchor: SegmentId | null;
    range: { start_ms: number; end_ms: number } | null;
  }
  let renaming = $state.raw<Rename | null>(null);
  let menu = $state.raw<{ items: MenuItem[]; x: number; y: number; label: string } | null>(null);
  let retrying = $state(false);
  let now = $state(Date.now());

  const meeting = $derived(detail?.meeting ?? null);
  const speakers = $derived(detail?.speakers ?? []);
  const roots = $derived(speakers.filter((s) => s.merged_into === null));
  const stereo = $derived(
    waveform ? waveform.channels === 2 : meeting?.source === "live" && player.ready,
  );
  const labelsFull = $derived(roots.filter((s) => s.channel !== "mic").length >= 8);

  // ---- Live recording and the post-pass ---------------------------------------

  /** This meeting is the one recording now. */
  const isLive = $derived(live.meetingId === meetingId && live.active);
  const postPass = $derived(app.postPassOf(meetingId));
  const postPassJob = $derived(postPass ? (app.jobs.find((j) => j.id === postPass.job_id) ?? null) : null);
  const finalizing = $derived(!isLive && (postPass !== null || meeting?.status === "processing"));
  const postPassError = $derived(
    !isLive && !postPass && meeting?.source === "live" && meeting.status === "failed"
      ? (app.postPassErrors.get(meetingId) ?? "The post-pass did not finish.")
      : null,
  );

  /**
   * While recording, talk time comes from the live words (there are no
   * segments yet), and speakers without words are hidden: a label the engine
   * revised away (the "Unknown speaker 0:00" of a first partial) has a row
   * but nothing to show. A root stays while a speaker merged into it has words.
   */
  const shownSpeakers = $derived.by((): SpeakerView[] => {
    if (!isLive) return speakers;
    const talk = new Map<number, number>();
    for (const r of live.rows) {
      if (r.speakerId !== null) talk.set(r.speakerId, (talk.get(r.speakerId) ?? 0) + (r.end_ms - r.start_ms));
    }
    const rootOf = (s: SpeakerView) => s.merged_into ?? s.id;
    const rootTalk = new Map<number, number>();
    for (const s of speakers) {
      const t = talk.get(s.id);
      if (t !== undefined) rootTalk.set(rootOf(s), (rootTalk.get(rootOf(s)) ?? 0) + t);
    }
    return speakers
      .filter((s) => talk.has(s.id) || (s.merged_into === null && rootTalk.has(s.id)))
      .map((s) => ({ ...s, talk_ms: s.merged_into === null ? (rootTalk.get(s.id) ?? 0) : (talk.get(s.id) ?? 0) }));
  });

  $effect(() => {
    if (!finalizing) return;
    const t = setInterval(() => (now = Date.now()), 500);
    return () => clearInterval(t);
  });

  // New provisional labels get speaker rows as they appear: fetch them.
  let speakerFetch: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    if (!isLive || !detail) return;
    const known = new Set(speakers.map((s) => s.id));
    const missing = live.rows.some((r) => r.speakerId !== null && !known.has(r.speakerId));
    if (missing && speakerFetch === undefined) {
      speakerFetch = setTimeout(() => {
        speakerFetch = undefined;
        api
          .getMeeting(meetingId)
          .then((d) => (detail = d))
          .catch(() => {});
      }, 300);
    }
  });

  // Stop finished: the live run is now a stored transcript; open it where the user was.
  let wasLive = false;
  $effect(() => {
    const now = isLive;
    untrack(() => {
      if (wasLive && !now) {
        const a = liveView?.anchor() ?? { follow: true, ms: null };
        void refreshAll(a.follow || a.ms === null ? "end" : a.ms);
      }
      wasLive = now;
    });
  });

  /** Scroll for a Transcript that mounts after a reload (live view to transcript). */
  let pendingScroll = $state<number | "end" | null>(null);
  $effect(() => {
    const p = pendingScroll;
    const t = transcript;
    if (!t || p === null || segments.length === 0) return;
    untrack(() => {
      pendingScroll = null;
      t.scrollTo(p);
    });
  });

  onMount(() => {
    if (audioEl) player.attach(audioEl);
    void load();
    const off = app.onJob((e) => {
      if (e.type === "done" && e.meeting_id === meetingId) {
        const t0 = performance.now();
        void refreshAll(transcript?.anchorMs() ?? null).then(() => {
          void api.log(
            "info",
            `meeting ${meetingId}: post-pass run shown ${Math.round(performance.now() - t0)} ms after job:done`,
          );
          if (active) app.toast("success", "Final transcript ready: the post-pass replaced the live one.");
        });
      } else if ((e.type === "error" || e.type === "cancelled") && e.meeting_id === meetingId) {
        void refreshAll(transcript?.anchorMs() ?? null);
      }
    });
    return () => {
      off();
      clearTimeout(speakerFetch);
      player.destroy();
    };
  });

  async function load() {
    const id = meetingId;
    try {
      const [d, first] = await Promise.all([api.getMeeting(id), api.getSegments(id, 0, FIRST_PAGE)]);
      detail = d;
      segments = first;
      loaded = true;
      if (d.meeting.status === "recording" || d.meeting.status === "processing") {
        noAudio = d.meeting.status === "recording" ? "Recording…" : "Playback is ready when finalizing finishes.";
      } else {
        void loadAudio(id);
      }
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
      const url = await api.getAudioUrl(id);
      noAudio = null;
      player.error = null;
      player.url = url;
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
      app.error(`Could not reload the meeting (${errorText(e)}); press F5 to try again`);
    }
  }

  /**
   * Refetches everything (after Stop, a finished post-pass, a retry) and
   * scrolls to `scroll`: a meeting time, the end, or nowhere in particular.
   */
  async function refreshAll(scroll: number | "end" | null) {
    try {
      const [d, segs] = await Promise.all([api.getMeeting(meetingId), api.getSegments(meetingId, 0, 1_000_000)]);
      detail = d;
      segments = segs;
      fullyLoaded = true;
      pendingScroll = scroll;
      void app.refreshMeetings();
      if (d.meeting.status === "processing") {
        noAudio = "Playback is ready when finalizing finishes.";
      } else if (d.meeting.status !== "recording") {
        void loadAudio(meetingId);
      }
    } catch (e) {
      app.error(`Could not reload the meeting (${errorText(e)}); press F5 to try again`);
    }
  }

  async function retryPostPass() {
    retrying = true;
    try {
      const job = await api.rerunPostPass(meetingId);
      app.trackPostPass(job, meetingId, meeting?.title ?? "");
      await refreshAll(transcript?.anchorMs() ?? null);
    } catch (e) {
      app.error(`Retry failed (${errorText(e)}); Diagnostics, Why isn't it working, shows what is missing`);
    } finally {
      retrying = false;
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
      app.error(`Could not rename the meeting (${errorText(e)}); try again, or restart Diarist if it keeps failing`);
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
      app.error(`Could not change the pin (${errorText(e)}); try again, or restart Diarist if it keeps failing`);
    }
  }

  async function deleteMeeting() {
    if (meeting) await app.deleteMeeting(meeting);
  }

  async function exportAs(format: ExportFormat) {
    if (!meeting) return;
    try {
      const path = await api.exportMeeting(meeting.id, format);
      if (path) app.toast("success", `Exported to ${path}`);
    } catch (e) {
      app.error(`Export failed (${errorText(e)}); choose a folder you can write to and try again`);
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

  function startRename(
    speakerId: SpeakerId,
    anchor: SegmentId | null,
    range: { start_ms: number; end_ms: number } | null = null,
  ) {
    renaming = { speakerId, anchor, range };
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
      if (r.range) {
        // While recording there are no segments: the live line's range is the anchor.
        await api.renameSpeakerAt(r.speakerId, name, r.range.start_ms, r.range.end_ms);
      } else {
        await api.renameSpeaker(r.speakerId, name, r.anchor);
      }
      await reload(r.anchor ?? undefined);
    } catch (e) {
      app.error(`Rename failed (${errorText(e)}); try again, or restart Diarist if it keeps failing`);
    }
    refocus(r.anchor);
  }

  function refocus(anchor: SegmentId | null) {
    if (isLive) liveView?.focus();
    else if (anchor !== null) transcript?.focus();
  }

  async function edit(what: string, fn: () => Promise<void>) {
    try {
      await fn();
      await reload();
    } catch (e) {
      app.error(`${what} failed (${errorText(e)}); try again, or restart Diarist if it keeps failing`);
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
    if (!active || e.defaultPrevented || menu || renaming || app.confirmRequest || app.helpOpen || live.sheetOpen) {
      return;
    }
    const key = e.key;
    if (e.ctrlKey && !e.shiftKey && !e.altKey) {
      if ((key === "e" || key === "E") && !isLive) exportMenu();
      else if (key === "Delete" && !isLive) void deleteMeeting();
      else if (key === "2") (isLive ? liveView : transcript)?.focus();
      else return;
      e.preventDefault();
      return;
    }
    if (e.ctrlKey || e.altKey || e.metaKey || isTyping(e)) return;
    const onButton = e.target instanceof HTMLButtonElement || e.target instanceof HTMLAnchorElement;
    // No audio to play while recording.
    if (isLive && !["F2", "p", "P", "g", "G"].includes(key)) return;
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
    <p class="state error" role="alert">Could not open this meeting: {loadError}. Press F5 to try again.</p>
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
          {#if !isLive}<span>{formatDuration(meeting.duration_ms)}</span>{/if}
          <span>{meeting.source === "live" ? "Recorded live" : "Imported file"}</span>
          <span>{meeting.language}</span>
          {#if detail?.current_run}<span>{detail.current_run.kind.replace("_", "-")} run</span>{/if}
          {#if isLive}
            <span class="chip status-recording">recording</span>
          {:else if finalizing}
            <span class="chip status-processing">finalizing</span>
          {:else if meeting.status !== "done"}
            <span class="chip status-{meeting.status}">{meeting.status}</span>
          {/if}
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
          {#if !isLive}
            <button type="button" id="export-btn" class="btn" onclick={() => exportMenu()} title="Export transcript (Ctrl+E)">
              <Icon name="export" /><span class="btn-label">Export</span>
            </button>
            <button type="button" class="btn btn-icon" onclick={deleteMeeting} title="Delete meeting (Ctrl+Delete)" aria-label="Delete meeting">
              <Icon name="trash" />
            </button>
          {/if}
        </div>
      </div>
    </header>

    <SpeakerLegend
      speakers={shownSpeakers}
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

    {#if isLive}
      <LiveView bind:this={liveView} speakers={shownSpeakers} onrename={(id, range) => startRename(id, null, range)} />
    {:else}
      {#if finalizing}
        {@const job = postPassJob}
        {@const pct = job?.totalMs ? Math.min(100, Math.round((job.doneMs / job.totalMs) * 100)) : null}
        <div class="banner info" role="status">
          <div class="b-main">
            <strong>Finalizing.</strong>
            <span>
              {job ? STAGE_TEXT[job.stage] : "Waiting for the post-pass"}{pct !== null ? ` · ${pct}%` : ""}.
              This is the live transcript; the post-pass replaces it when it finishes.
            </span>
            <div class="bar" role="progressbar" aria-label="Finalizing" aria-valuenow={pct ?? undefined} aria-valuemin={0} aria-valuemax={100}>
              <div class="bar-fill" class:indeterminate={pct === null} style="width: {pct ?? 30}%"></div>
            </div>
          </div>
          {#if job}<span class="mono elapsed">{formatClock(now - job.startedAt)}</span>{/if}
          {#if postPass}
            <button type="button" class="link" onclick={() => app.cancelJob(postPass.job_id)} title="Keep the live transcript">Cancel</button>
          {/if}
        </div>
      {:else if postPassError}
        <div class="banner error" role="alert">
          <div class="b-main">
            <strong>Finalizing failed.</strong>
            <span>{postPassError} The live transcript stays.</span>
          </div>
          <button type="button" class="btn" disabled={retrying} onclick={retryPostPass}>{retrying ? "Queuing…" : "Retry"}</button>
        </div>
      {/if}
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
  {/if}

  {#if !isLive}
    <AudioBar {player} {waveform} {stereo} noAudio={loaded ? noAudio : null} fallbackMs={meeting?.duration_ms ?? 0} />
  {/if}
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
  .banner {
    display: flex;
    gap: 12px;
    align-items: center;
    margin: 0 14px 8px;
    padding: 8px 10px;
    border: 1px solid var(--accent-border);
    border-radius: var(--radius);
    background: var(--bg-selected);
  }
  .banner.error {
    border-color: var(--danger);
    background: var(--danger-bg);
    color: var(--danger);
  }
  .b-main {
    flex: 1;
    min-width: 0;
  }
  .elapsed {
    color: var(--fg-muted);
    font-size: 12px;
  }
  .bar {
    height: 4px;
    margin-top: 6px;
    overflow: hidden;
    border-radius: 2px;
    background: var(--bg-hover);
  }
  .bar-fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.25s linear;
  }
  .bar-fill.indeterminate {
    animation: slide 1.4s ease-in-out infinite;
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(340%);
    }
  }
</style>
