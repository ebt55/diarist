<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { listMeetings } from "./lib/api";
  import { formatDate, formatDuration } from "./lib/format";
  import type { MeetingId } from "./lib/generated/MeetingId";
  import type { MeetingSummary } from "./lib/generated/MeetingSummary";

  let meetings = $state<MeetingSummary[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let selectedId = $state<MeetingId | null>(null);

  const selected = $derived(meetings.find((m) => m.id === selectedId) ?? null);

  onMount(() => {
    // The window starts hidden (tauri.conf.json) so the user never sees an
    // unstyled white frame; reveal it now that the styled shell is in the DOM.
    // Not via requestAnimationFrame: a hidden WebView2 never fires it.
    getCurrentWindow()
      .show()
      .catch((e: unknown) => console.error("window.show failed", e));

    listMeetings()
      .then((list) => (meetings = list))
      .catch((e: unknown) => (error = String(e)))
      .finally(() => (loading = false));
  });
</script>

<div class="shell">
  <aside class="sidebar" aria-label="Meetings">
    <header class="sidebar-header">
      <h1>Meetings</h1>
    </header>

    {#if loading}
      <p class="sidebar-note">Loading…</p>
    {:else if error}
      <p class="sidebar-note error" role="alert">Could not load meetings: {error}</p>
    {:else if meetings.length === 0}
      <p class="sidebar-note">No meetings yet.</p>
    {:else}
      <ul class="meeting-list">
        {#each meetings as meeting (meeting.id)}
          <li>
            <button
              type="button"
              class="meeting"
              aria-current={meeting.id === selectedId ? "true" : undefined}
              onclick={() => (selectedId = meeting.id)}
            >
              <span class="meeting-title">{meeting.title || "Untitled meeting"}</span>
              <span class="meeting-meta">
                <time datetime={new Date(meeting.started_at).toISOString()}>
                  {formatDate(meeting.started_at)}
                </time>
                <span aria-hidden="true">·</span>
                <span>{formatDuration(meeting.duration_ms)}</span>
                <span aria-hidden="true">·</span>
                <span>{meeting.speaker_count} {meeting.speaker_count === 1 ? "speaker" : "speakers"}</span>
                {#if meeting.status !== "done"}
                  <span class="status status-{meeting.status}">{meeting.status}</span>
                {/if}
              </span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </aside>

  <main class="content">
    {#if selected}
      <h2>{selected.title || "Untitled meeting"}</h2>
      <p class="preview">{selected.preview || "No transcript yet."}</p>
      <p class="placeholder">Transcript view arrives in Phase 2.</p>
    {:else}
      <p class="placeholder">Select a meeting</p>
    {/if}
  </main>
</div>
