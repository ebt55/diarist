<script lang="ts">
  // Audio retention warnings in the library (diarist_core::retention, issue
  // 22). Nothing here deletes without the user's click:
  // - the first-deletion hold, a dialog: Delete as set, Keep everything,
  //   Review and pin (Esc or Ask later leaves it held, asked again next launch);
  // - the heads-up: audio that goes within a few days, Pin beside each. Only
  //   while the window is in front does it count as seen (the backend then
  //   allows that deletion), so a window left in the tray warns nobody;
  // - held: audio past the limit that nobody was warned about, kept until
  //   the user deletes it or pins it;
  // - the one-time storage prompt past 10 GB.
  import { onMount, tick } from "svelte";
  import { api, errorText, onEvent } from "../lib/api";
  import { formatBytes, formatShortDate, plural } from "../lib/format";
  import type { MeetingId } from "../lib/generated/MeetingId";
  import {
    RETENTION_EVENT,
    RETENTION_UI_EVENT,
    deletedAsSetText,
    headsUpText,
    headsUpTitle,
    heldText,
    heldTitle,
    holdText,
    meetingBytes,
    notShown,
    retentionApi,
    retentionChanged,
    stillDeleted,
    storageText,
    toReport,
    whenText,
    withoutPinned,
    type RetentionStatus,
  } from "../lib/retention";
  import { app } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  let status = $state.raw<RetentionStatus | null>(null);
  let now = $state(Date.now());
  /** This session: the hold was put off. */
  let holdLater = $state(false);
  let reviewing = $state(false);
  /** This session: meetings a card's Dismiss hid. Meetings listed later still show. */
  let dismissed = $state.raw(new Set<MeetingId>());
  /** Meetings deleted in this window (the list menu, the meeting view), hidden at once. */
  let deleted = $state.raw(new Set<MeetingId>());
  /** The storage prompt stays up after it was marked seen, until dismissed. */
  let storageShown = $state<number | null>(null);
  let busy = $state(false);
  let dialog: HTMLDialogElement | undefined = $state();
  /** Heads-up meetings reported as seen since the last refresh (the backend keeps the first sighting). */
  const reported = new Set<MeetingId>();
  let lastFetch = 0;

  const pinned = $derived(new Set(app.meetings.filter((m) => m.audio_keep).map((m) => m.id)));
  const isPinned = (id: MeetingId) => pinned.has(id);
  const holdList = $derived(status?.first_hold ? notShown(status.due.meetings, deleted) : []);
  const holdUnpinned = $derived(withoutPinned(holdList, pinned));
  const upcoming = $derived(status ? notShown(status.upcoming, pinned, dismissed, deleted) : []);
  const held = $derived(status ? notShown(status.held, pinned, dismissed, deleted) : []);
  const showHold = $derived(holdList.length > 0 && !holdLater);

  async function refresh() {
    lastFetch = Date.now();
    const asked = deleted;
    try {
      const s = await retentionApi.status();
      now = Date.now();
      reported.clear();
      status = s;
      deleted = stillDeleted(deleted, asked);
      if (s.storage_prompt && storageShown === null) storageShown = s.recordings_bytes;
    } catch (e) {
      void api.log("warn", `retention status: ${errorText(e)}`);
    }
  }

  /** The window is in front: what the notices list could have been seen. */
  function inFront(): boolean {
    return document.visibilityState === "visible" && document.hasFocus();
  }

  function reportSeen() {
    if (!inFront()) return;
    const ids = toReport(upcoming, reported);
    if (ids.length > 0) {
      for (const id of ids) reported.add(id);
      retentionApi.act({ kind: "seen", ids }).catch((e) => {
        for (const id of ids) reported.delete(id);
        void api.log("warn", `retention heads-up not recorded: ${errorText(e)}`);
      });
    }
    if (status?.storage_prompt && storageShown !== null) {
      status = { ...status, storage_prompt: false };
      retentionApi.act({ kind: "storage_prompt_seen" }).catch((e) => void api.log("warn", `storage prompt not recorded: ${errorText(e)}`));
    }
  }

  // Report once the cards are on screen.
  $effect(() => {
    void upcoming;
    void storageShown;
    void tick().then(reportSeen);
  });

  $effect(() => {
    if (!dialog) return;
    if (showHold && !dialog.open) dialog.showModal();
    else if (!showHold && dialog.open) dialog.close();
  });

  onMount(() => {
    void refresh();
    const onFocus = () => {
      if (Date.now() - lastFetch > 60_000) void refresh();
      else reportSeen();
    };
    const onUi = () => void refresh();
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onFocus);
    window.addEventListener(RETENTION_UI_EVENT, onUi);
    const off = onEvent<null>(RETENTION_EVENT, () => void refresh());
    // A meeting deleted from the list menu or the meeting view leaves the
    // notices at once; the refetch (here and in Settings) updates the sizes.
    const offDeleted = app.onMeetingDeleted((id) => {
      deleted = new Set([...deleted, id]);
      retentionChanged();
    });
    return () => {
      offDeleted();
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onFocus);
      window.removeEventListener(RETENTION_UI_EVENT, onUi);
      void off.then((f) => f());
    };
  });

  async function togglePin(id: MeetingId) {
    const keep = !isPinned(id);
    try {
      await api.setAudioKeep(id, keep);
      app.patchMeeting(id, { audio_keep: keep });
    } catch (e) {
      app.error(`Could not ${keep ? "pin" : "unpin"} (${errorText(e)}). Try again with P in the meeting`);
    }
  }

  async function deleteAsSet() {
    if (!status) return;
    const headsUpDays = status.heads_up_days;
    busy = true;
    try {
      const r = await retentionApi.act({ kind: "delete_as_set", ids: holdUnpinned.map((m) => m.meeting_id) });
      app.toast("info", deletedAsSetText(r.meetings, r.bytes, headsUpDays));
      if (r.errors.length > 0) app.error(`Some audio could not be deleted (${r.errors[0]}). Close programs using those files`);
    } catch (e) {
      app.error(`Could not delete the audio (${errorText(e)}). Nothing else changed, try again`);
    } finally {
      busy = false;
      reviewing = false;
      await refresh();
      retentionChanged();
    }
  }

  async function keepEverything() {
    busy = true;
    if (await app.setSetting("audio_retention_days", "0")) {
      app.toast("info", "Audio is kept forever. Change it in Settings under Keep audio for");
    }
    busy = false;
    reviewing = false;
    await refresh();
    retentionChanged();
  }

  async function review() {
    reviewing = true;
    await tick();
    dialog?.querySelector<HTMLButtonElement>("button.pin-btn")?.focus();
  }

  function askLater() {
    holdLater = true;
    reviewing = false;
  }

  async function deleteHeld() {
    const list = held;
    if (list.length === 0) return;
    const bytes = list.reduce((sum, n) => sum + n.bytes, 0);
    const ok = await app.confirm({
      title: "Delete this audio now?",
      message: `The audio of ${plural(list.length, "meeting")} (${formatBytes(bytes)}) is deleted. Transcripts stay.`,
      confirmLabel: "Delete audio",
      danger: true,
    });
    if (!ok) return;
    busy = true;
    try {
      const r = await retentionApi.act({ kind: "delete", ids: list.map((n) => n.meeting_id) });
      app.toast("info", `Freed ${formatBytes(r.bytes)} (${plural(r.files, "file")})`);
      if (r.errors.length > 0) app.error(`Some audio could not be deleted (${r.errors[0]}). Close programs using those files`);
    } catch (e) {
      app.error(`Could not delete the audio (${errorText(e)}). Nothing else changed, try again`);
    } finally {
      busy = false;
      await refresh();
      retentionChanged();
    }
  }

  function dismiss(list: { meeting_id: MeetingId }[]) {
    dismissed = new Set([...dismissed, ...list.map((n) => n.meeting_id)]);
  }

  function openSettings(anchor: "retention" | "storage") {
    app.settingsAnchor = anchor;
    app.route = "settings";
  }
</script>

<dialog
  bind:this={dialog}
  class="hold"
  aria-labelledby="hold-title"
  aria-describedby="hold-text"
  onkeydown={(e) => e.stopPropagation()}
  oncancel={(e) => {
    e.preventDefault();
    askLater();
  }}
>
  {#if status && holdList.length > 0}
    <h2 id="hold-title">Delete old audio?</h2>
    <p id="hold-text">{holdText(status.days, holdUnpinned.length, holdUnpinned.reduce((s, m) => s + meetingBytes(m), 0))}</p>
    {#if reviewing}<p class="hint">Pin the meetings whose audio you want to keep, then choose.</p>{/if}
    <ul class="list" aria-label="Meetings whose audio would be deleted">
      {#each holdList as m (m.meeting_id)}
        <li class:kept={isPinned(m.meeting_id)}>
          <span class="title">{m.title || "Untitled meeting"}</span>
          <span class="meta">{formatShortDate(m.ended_at)} · {formatBytes(meetingBytes(m))}</span>
          {#if reviewing}
            <button
              type="button"
              class="btn pin-btn"
              class:on={isPinned(m.meeting_id)}
              aria-pressed={isPinned(m.meeting_id)}
              onclick={() => togglePin(m.meeting_id)}
            >
              <Icon name="pin" size={12} />{isPinned(m.meeting_id) ? "Pinned" : "Pin"}
            </button>
          {/if}
        </li>
      {/each}
    </ul>
    <div class="buttons">
      <button type="button" class="link later" onclick={askLater}>Ask later</button>
      <button type="button" class="btn btn-danger" disabled={busy} onclick={deleteAsSet}>Delete as set</button>
      <button type="button" class="btn" disabled={busy} onclick={keepEverything}>Keep everything</button>
      {#if !reviewing}
        <!-- svelte-ignore a11y_autofocus -->
        <button type="button" class="btn btn-primary" autofocus disabled={busy} onclick={review}>Review and pin</button>
      {/if}
    </div>
  {/if}
</dialog>

{#if held.length > 0 && status}
  <section class="card" aria-labelledby="held-title">
    <h3 id="held-title">{heldTitle(status.days)}</h3>
    <p class="text">{heldText(status.days)}</p>
    <ul class="list">
      {#each held as n (n.meeting_id)}
        <li>
          <button type="button" class="link title" onclick={() => app.open(n.meeting_id)}>{n.title || "Untitled meeting"}</button>
          <span class="meta">{formatBytes(n.bytes)}</span>
          <button type="button" class="btn pin-btn" onclick={() => togglePin(n.meeting_id)}><Icon name="pin" size={12} />Pin audio</button>
        </li>
      {/each}
    </ul>
    <div class="actions">
      <button type="button" class="btn btn-danger" disabled={busy} onclick={deleteHeld}>Delete audio</button>
      <button type="button" class="btn" title="Hide this until Diarist starts again" onclick={() => dismiss(held)}>Not now</button>
    </div>
  </section>
{/if}

{#if upcoming.length > 0 && status}
  <section class="card" aria-labelledby="headsup-title">
    <h3 id="headsup-title">{headsUpTitle()}</h3>
    <p class="text">{headsUpText(status.days)}</p>
    <ul class="list">
      {#each upcoming as n (n.meeting_id)}
        <li>
          <button type="button" class="link title" onclick={() => app.open(n.meeting_id)}>{n.title || "Untitled meeting"}</button>
          <span class="meta">{whenText(n.due_at, now)}</span>
          <button type="button" class="btn pin-btn" onclick={() => togglePin(n.meeting_id)}><Icon name="pin" size={12} />Pin audio</button>
        </li>
      {/each}
    </ul>
    <div class="actions">
      <button type="button" class="btn" onclick={() => openSettings("retention")}>Change setting</button>
      <button type="button" class="btn" title="Hide this until Diarist starts again" onclick={() => dismiss(upcoming)}>Not now</button>
    </div>
  </section>
{/if}

{#if storageShown !== null && status}
  <section class="card" aria-labelledby="storage-title">
    <h3 id="storage-title">Recordings are getting big</h3>
    <p class="text">{storageText(storageShown, status.days)}</p>
    <div class="actions">
      <button type="button" class="btn" onclick={() => { storageShown = null; openSettings("retention"); }}>Set a day limit</button>
      <button type="button" class="btn" onclick={() => { storageShown = null; openSettings("storage"); }}>Review</button>
      <button type="button" class="btn" onclick={() => (storageShown = null)}>Dismiss</button>
    </div>
  </section>
{/if}

<style>
  .card {
    margin: 4px 10px 6px;
    padding: 8px 10px;
    border: 1px solid var(--border-strong);
    border-left: 4px solid var(--warning-border);
    border-radius: var(--radius);
    background: var(--bg-raised);
  }
  h3 {
    margin: 0 0 4px;
    font-size: 13px;
  }
  .text {
    margin: 0 0 6px;
    color: var(--fg-muted);
    font-size: 12px;
  }
  .list {
    max-height: 160px;
    margin: 0 0 8px;
    padding: 0;
    overflow-y: auto;
    list-style: none;
  }
  .list li {
    display: flex;
    gap: 6px;
    align-items: center;
    padding: 2px 0;
    font-size: 12px;
  }
  .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-align: left;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    color: var(--fg-muted);
    white-space: nowrap;
  }
  .pin-btn {
    padding: 1px 6px;
    font-size: 12px;
  }
  .kept .title,
  .kept .meta {
    color: var(--fg-faint);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .hold {
    width: min(520px, calc(100vw - 32px));
    padding: 18px 20px 16px;
    border: 1px solid var(--border-strong);
    border-radius: 10px;
    background: var(--bg-raised);
    color: var(--fg);
    box-shadow: var(--shadow);
  }
  .hold::backdrop {
    background: rgb(0 0 0 / 0.35);
  }
  .hold h2 {
    margin: 0 0 8px;
    font-size: 16px;
  }
  .hold p {
    margin: 0 0 10px;
    color: var(--fg-muted);
  }
  .hold .list {
    max-height: 240px;
    margin-bottom: 14px;
  }
  .hold .list li {
    font-size: 13px;
  }
  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    justify-content: flex-end;
  }
  .later {
    margin-right: auto;
  }
</style>
