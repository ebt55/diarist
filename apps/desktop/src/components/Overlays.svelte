<script lang="ts">
  // Toasts, the confirm dialog, the shortcut sheet and the drop overlay.
  import { app } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  let dialog: HTMLDialogElement | undefined = $state();
  let help: HTMLDialogElement | undefined = $state();

  $effect(() => {
    if (app.confirmRequest && dialog && !dialog.open) dialog.showModal();
  });
  $effect(() => {
    if (!help) return;
    if (app.helpOpen && !help.open) help.showModal();
    if (!app.helpOpen && help.open) help.close();
  });

  const SHORTCUTS: [string, [string, string][]][] = [
    [
      "Anywhere",
      [
        ["Ctrl K", "Search transcripts"],
        ["Ctrl O", "Import audio or video"],
        ["R", "Record, or stop the recording"],
        ["Ctrl Alt R", "Record or stop from any app"],
        ["Ctrl 1 / Ctrl 2", "Focus meeting list / transcript"],
        ["Ctrl ,", "Settings"],
        ["Ctrl Shift D", "Diagnostics"],
        ["Ctrl R", "Refresh the current view"],
        ["? or F1", "This sheet"],
        ["Esc", "Close menu, dialog or page"],
      ],
    ],
    [
      "Library",
      [
        ["↑ ↓  Home End", "Move in the list"],
        ["Enter", "Open meeting or search hit"],
        ["Delete", "Delete meeting"],
        ["/", "Back to search"],
      ],
    ],
    [
      "Playback",
      [
        ["Space", "Play or pause"],
        ["← →", "Skip 5 s (Shift: 15 s)"],
        ["[ ]", "Slower / faster"],
        ["M", "Channels: mix, mic, system"],
        [".", "Jump to the playhead"],
      ],
    ],
    [
      "Meeting",
      [
        ["↑ ↓", "Move between segments"],
        ["Enter", "Play from the segment"],
        ["R", "Rename the segment's speaker"],
        ["1 – 9", "Give the segment to speaker N"],
        ["S", "Split the segment at the playhead"],
        ["Shift F10", "Segment menu (reassign, split)"],
        ["G", "Go to the speaker legend"],
        ["F2", "Edit the title"],
        ["P", "Pin or unpin the audio"],
        ["F", "Follow playback on or off"],
        ["Ctrl E", "Export"],
        ["Ctrl Delete", "Delete meeting"],
      ],
    ],
    [
      "Recording",
      [
        ["Ctrl .", "Stop recording"],
        ["↑ ↓", "Move between live lines"],
        ["R", "Rename the line's speaker"],
        ["End", "Follow the live text again"],
        ["Ctrl 2", "Focus the live transcript"],
      ],
    ],
  ];
</script>

<div class="toasts" aria-live="polite">
  {#each app.toasts as t (t.id)}
    <div class="toast {t.kind}" role={t.kind === "error" ? "alert" : "status"}>
      <span class="msg">{t.message}</span>
      {#if t.action}
        {@const action = t.action}
        <button
          type="button"
          class="link"
          onclick={() => {
            app.dismiss(t.id);
            action.run();
          }}>{action.label}</button
        >
      {/if}
      <button type="button" class="x" aria-label="Dismiss" onclick={() => app.dismiss(t.id)}>
        <Icon name="close" size={14} />
      </button>
    </div>
  {/each}
</div>

<dialog
  bind:this={dialog}
  class="modal"
  aria-labelledby="confirm-title"
  oncancel={(e) => {
    e.preventDefault();
    dialog?.close();
    app.confirmRequest?.resolve(false);
  }}
>
  {#if app.confirmRequest}
    {@const req = app.confirmRequest}
    <h2 id="confirm-title">{req.title}</h2>
    <p>{req.message}</p>
    <div class="buttons">
      <button
        type="button"
        class="btn"
        onclick={() => {
          dialog?.close();
          req.resolve(false);
        }}>Cancel</button
      >
      <!-- svelte-ignore a11y_autofocus -->
      <button
        type="button"
        class="btn {req.danger ? 'btn-danger' : 'btn-primary'}"
        autofocus
        onclick={() => {
          dialog?.close();
          req.resolve(true);
        }}>{req.confirmLabel}</button
      >
    </div>
  {/if}
</dialog>

<dialog
  bind:this={help}
  class="modal help"
  aria-label="Keyboard shortcuts"
  oncancel={(e) => {
    e.preventDefault();
    app.helpOpen = false;
  }}
  onclose={() => (app.helpOpen = false)}
>
  <div class="help-head">
    <h2>Keyboard shortcuts</h2>
    <button type="button" class="btn btn-icon" aria-label="Close" onclick={() => (app.helpOpen = false)}>
      <Icon name="close" />
    </button>
  </div>
  <div class="help-grid">
    {#each SHORTCUTS as [group, rows] (group)}
      <section>
        <h3>{group}</h3>
        <dl>
          {#each rows as [keys, what] (keys)}
            <dt><kbd>{keys}</kbd></dt>
            <dd>{what}</dd>
          {/each}
        </dl>
      </section>
    {/each}
  </div>
</dialog>

{#if app.dragging}
  <div class="drop" aria-hidden="true">
    <div class="drop-box">
      <Icon name="import" size={28} />
      <strong>Drop to import</strong>
      <span>Audio or video files become new meetings</span>
    </div>
  </div>
{/if}

<style>
  .toasts {
    position: fixed;
    right: 16px;
    bottom: 72px;
    z-index: 60;
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: flex-end;
    pointer-events: none;
  }
  .toast {
    display: flex;
    gap: 10px;
    align-items: center;
    max-width: 460px;
    padding: 8px 8px 8px 12px;
    border: 1px solid var(--border-strong);
    border-left: 4px solid var(--accent);
    border-radius: var(--radius);
    background: var(--bg-raised);
    box-shadow: var(--shadow);
    pointer-events: auto;
  }
  .toast.success {
    border-left-color: var(--ok-fg);
  }
  .toast.error {
    border-left-color: var(--danger);
  }
  .msg {
    flex: 1;
    overflow-wrap: anywhere;
  }
  .x {
    display: inline-flex;
    padding: 4px;
    border: 0;
    border-radius: 4px;
    background: none;
    color: var(--fg-muted);
  }
  .x:hover {
    background: var(--bg-hover);
  }
  .modal {
    max-width: 440px;
    padding: 18px 20px 16px;
    border: 1px solid var(--border-strong);
    border-radius: 10px;
    background: var(--bg-raised);
    color: var(--fg);
    box-shadow: var(--shadow);
  }
  .modal::backdrop {
    background: rgb(0 0 0 / 0.35);
  }
  .modal h2 {
    margin: 0 0 8px;
    font-size: 16px;
  }
  .modal p {
    margin: 0 0 16px;
    color: var(--fg-muted);
  }
  .buttons {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
  .help {
    max-width: 860px;
    width: calc(100vw - 80px);
  }
  .help-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .help-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: 4px 24px;
  }
  h3 {
    margin: 10px 0 4px;
    color: var(--fg-muted);
    font-size: 12px;
    letter-spacing: 0.03em;
    text-transform: uppercase;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 12px;
    margin: 0;
  }
  dt {
    white-space: nowrap;
  }
  dd {
    margin: 0;
  }
  .drop {
    position: fixed;
    inset: 0;
    z-index: 70;
    display: flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in srgb, var(--bg) 70%, transparent);
  }
  .drop-box {
    display: flex;
    flex-direction: column;
    gap: 6px;
    align-items: center;
    padding: 32px 48px;
    border: 2px dashed var(--accent);
    border-radius: 14px;
    background: var(--bg-raised);
    color: var(--accent);
  }
  .drop-box span {
    color: var(--fg-muted);
  }
</style>
