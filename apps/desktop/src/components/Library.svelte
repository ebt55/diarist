<script lang="ts">
  import { tick, untrack } from "svelte";
  import { VList, type VListHandle } from "virtua/svelte";
  import { api, errorText } from "../lib/api";
  import { formatClock, formatDuration, formatShortDate, plural } from "../lib/format";
  import type { SearchHit } from "../lib/generated/SearchHit";
  import type { MeetingSummary } from "../lib/generated/MeetingSummary";
  import type { JobStage } from "../lib/generated/JobStage";
  import { live } from "../lib/live.svelte";
  import { app } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  interface Props {
    onimport: () => void;
  }
  let { onimport }: Props = $props();

  let query = $state("");
  let hits = $state.raw<SearchHit[]>([]);
  let searchedQuery = $state("");
  let searching = $state(false);
  let cursor = $state(0);
  let hitCursor = $state(0);
  let vlist: VListHandle | undefined = $state();
  let resultsEl: HTMLDivElement | undefined = $state();
  let now = $state(Date.now());

  const showingResults = $derived(query.trim() !== "");
  const cursorMeeting = $derived(app.meetings[cursor]);

  // Debounced search; stale responses are dropped.
  $effect(() => {
    const q = query.trim();
    if (!q) {
      hits = [];
      searchedQuery = "";
      return;
    }
    const timer = setTimeout(async () => {
      searching = true;
      try {
        const result = await api.search(q, 100);
        if (query.trim() === q) {
          hits = result;
          searchedQuery = q;
          hitCursor = 0;
        }
      } catch (e) {
        app.error(`Search failed (${errorText(e)}); try plainer words (no quotes or symbols needed)`);
      } finally {
        searching = false;
      }
    }, 120);
    return () => clearTimeout(timer);
  });

  // Elapsed clock for running imports.
  $effect(() => {
    if (app.jobs.length === 0) return;
    const t = setInterval(() => (now = Date.now()), 500);
    return () => clearInterval(t);
  });

  // Keep the cursor on the open meeting when it changes elsewhere.
  $effect(() => {
    const id = app.openId;
    const list = app.meetings;
    const i = id === null ? -1 : list.findIndex((m) => m.id === id);
    untrack(() => {
      if (i >= 0) cursor = i;
      else if (cursor >= list.length) cursor = Math.max(0, list.length - 1);
    });
  });

  const STAGES: Record<JobStage, string> = {
    queued: "Queued",
    decoding: "Decoding",
    transcribing: "Transcribing",
    diarizing: "Finding speakers",
    saving: "Saving",
  };

  function moveCursor(next: number) {
    const n = app.meetings.length;
    if (n === 0) return;
    cursor = Math.max(0, Math.min(n - 1, next));
    vlist?.scrollToIndex(cursor, { align: "nearest" });
  }

  function moveHit(next: number) {
    const n = hits.length;
    if (n === 0) return;
    hitCursor = Math.max(0, Math.min(n - 1, next));
    void tick().then(() =>
      resultsEl?.querySelector(`[data-i="${hitCursor}"]`)?.scrollIntoView({ block: "nearest" }),
    );
  }

  function openHit(hit: SearchHit | undefined) {
    if (!hit) return;
    app.open(hit.meeting_id, { segmentId: hit.segment_id, startMs: hit.start_ms });
  }

  async function deleteMeeting(m: MeetingSummary | undefined) {
    if (!m) return;
    const ok = await app.confirm({
      title: "Delete meeting?",
      message: `"${m.title || "Untitled meeting"}" and its transcript will be deleted. Audio files are removed too.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!ok) return;
    try {
      await api.deleteMeeting(m.id);
      if (app.openId === m.id) app.openId = null;
      await app.refreshMeetings();
      app.toast("info", `Deleted "${m.title || "Untitled meeting"}"`);
    } catch (e) {
      app.error(`Delete failed (${errorText(e)}); close any program using its audio and try again`);
    }
  }

  function listKeys(e: KeyboardEvent, kind: "meetings" | "hits") {
    const move = kind === "meetings" ? moveCursor : moveHit;
    const at = kind === "meetings" ? cursor : hitCursor;
    const n = kind === "meetings" ? app.meetings.length : hits.length;
    switch (e.key) {
      case "ArrowDown":
      case "j":
        move(at + 1);
        break;
      case "ArrowUp":
      case "k":
        if (at === 0 && e.key === "ArrowUp") document.getElementById("search-input")?.focus();
        else move(at - 1);
        break;
      case "PageDown":
        move(at + 10);
        break;
      case "PageUp":
        move(at - 10);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(n - 1);
        break;
      case "Enter":
        if (kind === "meetings") {
          const m = app.meetings[cursor];
          if (m) app.open(m.id);
        } else {
          openHit(hits[hitCursor]);
        }
        break;
      case "Delete":
        if (kind === "meetings") void deleteMeeting(app.meetings[cursor]);
        break;
      case "/":
        document.getElementById("search-input")?.focus();
        break;
      default:
        return;
    }
    e.preventDefault();
  }

  /** Rows are options, not tab stops: a click on one keeps focus on the listbox. */
  function keepFocusOnList(e: FocusEvent) {
    const list = e.currentTarget as HTMLElement;
    if (e.target !== list) list.focus({ preventScroll: true });
  }

  function searchKeys(e: KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (showingResults) {
        hitCursor = 0;
        resultsEl?.focus();
      } else {
        document.getElementById("meeting-list")?.focus();
      }
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (showingResults) openHit(hits[hitCursor]);
    } else if (e.key === "Escape") {
      if (query) {
        e.preventDefault();
        e.stopPropagation();
        query = "";
      } else {
        (e.target as HTMLElement).blur();
      }
    }
  }
</script>

<aside class="sidebar" aria-label="Library">
  <header class="side-head">
    <span class="brand">Diarist</span>
    <div class="head-actions">
      {#if live.active}
        <button
          type="button"
          class="btn btn-rec"
          disabled={live.phase === "stopping"}
          onclick={() => live.toggle()}
          title="Stop recording (Ctrl+.; Ctrl+Alt+R from any app; both ask first)"
        >
          <span class="rec-dot live" aria-hidden="true"></span>
          <span class="mono">{formatClock(live.elapsedMs)}</span>
          <span class="sr-only">recording,</span>{live.phase === "stopping" ? "…" : "Stop"}
        </button>
      {:else}
        <button
          type="button"
          class="btn btn-rec"
          disabled={live.phase === "starting"}
          onclick={() => live.toggle()}
          title="Record a meeting (R; Ctrl+Alt+R from any app)"
        >
          <span class="rec-dot" aria-hidden="true"></span>{live.phase === "starting" ? "Starting…" : "Record"}
        </button>
      {/if}
      <button type="button" class="btn" onclick={onimport} title="Import audio or video (Ctrl+O)" aria-label="Import">
        <Icon name="import" /><span class="import-label">Import</span>
      </button>
      <button
        type="button"
        class="btn btn-icon"
        class:on={app.route === "settings"}
        onclick={() => (app.route = app.route === "settings" ? "library" : "settings")}
        title="Settings (Ctrl+,)"
        aria-label="Settings"
      >
        <Icon name="settings" />
      </button>
      <button
        type="button"
        class="btn btn-icon"
        class:on={app.route === "diagnostics"}
        onclick={() => (app.route = app.route === "diagnostics" ? "library" : "diagnostics")}
        title="Diagnostics (Ctrl+Shift+D)"
        aria-label="Diagnostics"
      >
        <Icon name="log" />
      </button>
    </div>
  </header>

  <div class="search">
    <Icon name="search" />
    <input
      id="search-input"
      type="search"
      placeholder="Search transcripts"
      autocomplete="off"
      spellcheck="false"
      aria-label="Search transcripts"
      aria-controls={showingResults ? "search-results" : "meeting-list"}
      bind:value={query}
      onkeydown={searchKeys}
    />
    {#if !query}<kbd class="search-kbd">Ctrl K</kbd>{/if}
  </div>

  {#if app.jobs.length > 0}
    <section class="jobs" aria-label="Imports and finalizing in progress">
      {#each app.jobs as job (job.id)}
        {@const pct = job.totalMs ? Math.min(100, Math.round((job.doneMs / job.totalMs) * 100)) : 0}
        {@const waiting = job.stage === "queued" && live.queuePaused}
        <div class="job" class:postpass={job.kind === "postpass"}>
          <div class="job-line">
            <span class="job-name" title={job.name}>
              {#if job.kind === "postpass"}
                <button type="button" class="link name-link" onclick={() => job.meetingId !== null && app.open(job.meetingId)}
                  >Finalizing {job.name}</button
                >
              {:else}{job.name}{/if}
            </span>
            <span class="mono">{pct}%</span>
          </div>
          <div
            class="bar"
            role="progressbar"
            aria-valuenow={pct}
            aria-valuemin="0"
            aria-valuemax="100"
            aria-label="{job.kind === 'postpass' ? 'Finalizing' : 'Import'} {job.name}"
          >
            <div class="bar-fill" style="width: {pct}%"></div>
          </div>
          <div class="job-line job-meta">
            <span
              >{job.cancelling
                ? "Cancelling…"
                : waiting
                  ? "Waiting for the recording to stop"
                  : STAGES[job.stage]}{waiting ? "" : ` · ${formatClock(now - job.startedAt)} elapsed`}</span
            >
            <button type="button" class="link" disabled={job.cancelling} onclick={() => app.cancelJob(job.id)}>Cancel</button>
          </div>
        </div>
      {/each}
    </section>
  {/if}

  <div class="list-meta" aria-live="polite">
    {#if showingResults}
      {#if searching && searchedQuery !== query.trim()}
        Searching…
      {:else}
        {plural(hits.length, "result")}{hits.length === 100 ? "+" : ""} for "{searchedQuery}"
      {/if}
    {:else if app.meetingsLoaded}
      {plural(app.meetings.length, "meeting")}
    {/if}
  </div>

  {#if showingResults}
    <div
      id="search-results"
      class="results"
      role="listbox"
      tabindex="0"
      aria-label="Search results"
      aria-activedescendant={hits[hitCursor] ? `hit-${hitCursor}` : undefined}
      bind:this={resultsEl}
      onfocusin={keepFocusOnList}
      onkeydown={(e) => listKeys(e, "hits")}
    >
      {#each hits as hit, i (hit.segment_id)}
        <div
          id="hit-{i}"
          data-i={i}
          class="row hit"
          class:cursor={i === hitCursor}
          role="option"
          tabindex="-1"
          aria-selected={i === hitCursor}
          onclick={() => {
            hitCursor = i;
            openHit(hit);
          }}
          onkeydown={() => {}}
        >
          <div class="row-title">{hit.meeting_title || "Untitled meeting"}</div>
          <div class="row-meta">
            <span>{formatShortDate(hit.meeting_started_at)}</span>
            <span class="mono">{formatClock(hit.start_ms)}</span>
            <span>{hit.speaker_name}</span>
          </div>
          <p class="snippet">
            {#each hit.snippet as part, k (k)}{#if part.hit}<mark>{part.text}</mark>{:else}{part.text}{/if}{/each}
          </p>
        </div>
      {:else}
        {#if !searching && searchedQuery}
          <p class="empty">No matches. Every word must appear in the same segment.</p>
        {/if}
      {/each}
    </div>
  {:else if app.meetingsError}
    <p class="empty error" role="alert">Could not load meetings: {app.meetingsError}. Restart Diarist; if it keeps failing, Diagnostics shows the log.</p>
  {:else if app.meetingsLoaded && app.meetings.length === 0}
    <p class="empty">No meetings yet. Press R to record one, import a recording with Ctrl+O, or drop files here.</p>
  {:else}
    <VList
      id="meeting-list"
      class="mlist"
      role="listbox"
      tabindex={0}
      aria-label="Meetings, newest first"
      aria-activedescendant={cursorMeeting ? `mtg-${cursorMeeting.id}` : undefined}
      data={app.meetings}
      getKey={(m) => m.id}
      itemSize={56}
      bind:this={vlist}
      onfocusin={keepFocusOnList}
      onkeydown={(e: KeyboardEvent) => listKeys(e, "meetings")}
    >
      {#snippet children(m: MeetingSummary, i: number)}
        <div
          id="mtg-{m.id}"
          class="row"
          class:cursor={i === cursor}
          class:open={m.id === app.openId}
          role="option"
          tabindex="-1"
          aria-selected={m.id === app.openId}
          onclick={() => {
            cursor = i;
            app.open(m.id);
          }}
          onkeydown={() => {}}
        >
          <div class="row-top">
            <span class="row-title">{m.title || "Untitled meeting"}</span>
            {#if m.audio_keep}<span class="pin" title="Audio pinned"><Icon name="pin" size={12} /></span>{/if}
            {#if m.status !== "done"}
              <span class="chip status-{m.status}"
                >{m.status === "processing" && m.source === "live" ? "finalizing" : m.status}</span
              >
            {/if}
          </div>
          <div class="row-meta">
            <span>{formatShortDate(m.started_at)}</span>
            <span>{formatDuration(m.duration_ms)}</span>
            <span>{plural(m.speaker_count, "speaker")}</span>
          </div>
        </div>
      {/snippet}
    </VList>
  {/if}
</aside>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg-sidebar);
    border-right: 1px solid var(--border);
  }
  .side-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 10px 10px 6px 14px;
  }
  .brand {
    overflow: hidden;
    font-weight: 700;
    letter-spacing: -0.01em;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .head-actions {
    display: flex;
    flex: none;
    gap: 4px;
  }
  .head-actions .btn-rec {
    min-width: 0;
  }
  @media (max-width: 1000px) {
    .import-label {
      display: none;
    }
  }
  .search {
    position: relative;
    display: flex;
    align-items: center;
    margin: 4px 10px 6px;
    color: var(--fg-muted);
  }
  .search :global(svg) {
    position: absolute;
    left: 9px;
    pointer-events: none;
  }
  .search input {
    width: 100%;
    padding: 6px 54px 6px 30px;
  }
  .search-kbd {
    position: absolute;
    right: 7px;
    pointer-events: none;
  }
  .jobs {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 2px 10px 6px;
  }
  .job {
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg);
  }
  .job-line {
    display: flex;
    gap: 8px;
    justify-content: space-between;
    align-items: baseline;
  }
  .job-name {
    overflow: hidden;
    font-weight: 600;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .job-meta {
    color: var(--fg-muted);
    font-size: 12px;
  }
  .job.postpass {
    border-color: var(--accent-border);
  }
  .name-link {
    overflow: hidden;
    max-width: 100%;
    color: inherit;
    font-weight: 600;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .bar {
    height: 4px;
    margin: 6px 0;
    overflow: hidden;
    border-radius: 2px;
    background: var(--bg-hover);
  }
  .bar-fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.25s linear;
  }
  .list-meta {
    padding: 2px 14px 4px;
    color: var(--fg-muted);
    font-size: 12px;
  }
  .results,
  :global(.mlist) {
    flex: 1;
    min-height: 0;
    padding: 0 6px 6px;
    outline: none;
  }
  .results {
    overflow-y: auto;
  }
  .results:focus-visible,
  :global(.mlist:focus-visible) {
    outline: none;
  }
  .row {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin: 0 4px;
    padding: 7px 10px;
    border-radius: var(--radius);
    cursor: default;
  }
  :global(.mlist) .row {
    height: 52px;
    margin-top: 2px;
    margin-bottom: 2px;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  .row.open {
    background: var(--bg-selected);
  }
  .row.cursor {
    box-shadow: inset 0 0 0 1px var(--border-strong);
  }
  :global(.mlist:focus-visible) .row.cursor,
  .results:focus-visible .row.cursor {
    box-shadow: inset 0 0 0 2px var(--focus);
  }
  .row-top {
    display: flex;
    gap: 6px;
    align-items: center;
    min-width: 0;
  }
  .row-top .row-title {
    flex: 1;
    min-width: 0;
  }
  .row-top .chip,
  .row-top .pin {
    flex: none;
  }
  .row-title {
    overflow: hidden;
    font-weight: 600;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .row-meta {
    display: flex;
    flex-wrap: nowrap;
    gap: 8px;
    align-items: center;
    overflow: hidden;
    color: var(--fg-muted);
    font-size: 12px;
    white-space: nowrap;
  }
  .pin {
    display: inline-flex;
    color: var(--accent);
  }
  .snippet {
    margin: 2px 0 0;
    color: var(--fg);
    font-size: 13px;
    line-height: 1.4;
  }
  .empty {
    margin: 8px 14px;
    color: var(--fg-muted);
  }
  .empty.error {
    color: var(--danger);
  }
</style>
