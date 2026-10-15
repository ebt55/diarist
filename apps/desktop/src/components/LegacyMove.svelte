<script lang="ts">
  // First launch after the rename: asks once whether to move the data of the
  // app's earlier name (meet-assist) into Diarist's data folder
  // (`diarist_core::models::legacy`). Both answers restart the app.
  import { api, errorText, onEvent } from "../lib/api";
  import { formatBytes, plural } from "../lib/format";
  import { app } from "../lib/state.svelte";
  import type { MoveProgress } from "../lib/generated/MoveProgress";

  let dialog: HTMLDialogElement | undefined = $state();
  let busy = $state<"move" | "fresh" | null>(null);
  let progress = $state<MoveProgress | null>(null);
  let done = $state<string | null>(null);
  let failure = $state<string | null>(null);
  const legacyRoot = $derived(app.startup?.legacy_root ?? null);

  $effect(() => {
    if (legacyRoot && dialog && !dialog.open) dialog.showModal();
  });

  $effect(() => {
    const off = onEvent<MoveProgress>("data:move_progress", (p) => {
      if (busy === "move") progress = p;
    });
    return () => void off.then((u) => u());
  });

  async function move() {
    busy = "move";
    failure = null;
    progress = { done_bytes: 0, total_bytes: 0 };
    try {
      const r = await api.migrateLegacyData();
      done = `Copied ${plural(Number(r.files), "file")} (${formatBytes(Number(r.bytes))}). Restarting…`;
      setTimeout(() => void api.restartApp(), 900);
    } catch (e) {
      busy = null;
      progress = null;
      failure = `Nothing was moved (${errorText(e)}). The meet-assist data is unchanged; try again, or start fresh.`;
    }
  }

  async function fresh() {
    busy = "fresh";
    failure = null;
    try {
      await api.startFresh();
      done = "Starting fresh. Restarting…";
      await api.restartApp();
    } catch (e) {
      busy = null;
      failure = `Could not start fresh (${errorText(e)}); restart Diarist and try again.`;
    }
  }
</script>

<dialog
  bind:this={dialog}
  class="modal"
  aria-labelledby="legacy-title"
  oncancel={(e) => {
    // An answer is needed: the app runs without a database until then.
    e.preventDefault();
  }}
>
  {#if legacyRoot}
    <h2 id="legacy-title">Move your existing meet-assist data to Diarist?</h2>
    <p>
      meet-assist is now called Diarist. Its meetings, recordings, engine and models are in
      <code>{legacyRoot}</code>.
    </p>
    <p>
      Move copies everything into Diarist's data folder, checks the copy, restarts, then deletes the old copy.
      Start fresh leaves the old folder as it is.
    </p>
    {#if progress && !done}
      {@const pct = progress.total_bytes ? Math.floor((progress.done_bytes / progress.total_bytes) * 100) : 0}
      <p class="progress">
        <span class="bar"><span class="fill" style="width: {pct}%"></span></span>
        Copying and checking: {formatBytes(progress.done_bytes)} of {formatBytes(progress.total_bytes)}. Keep Diarist
        open.
      </p>
    {/if}
    {#if done}<p class="status" role="status">{done}</p>{/if}
    {#if failure}<p class="failure" role="alert">{failure}</p>{/if}
    <div class="buttons">
      <button type="button" class="btn" disabled={busy !== null || done !== null} onclick={fresh}>Start fresh</button>
      <!-- svelte-ignore a11y_autofocus -->
      <button
        type="button"
        class="btn btn-primary"
        autofocus
        disabled={busy !== null || done !== null}
        onclick={move}>Move my data</button
      >
    </div>
  {/if}
</dialog>

<style>
  .modal {
    max-width: 480px;
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
  h2 {
    margin: 0 0 8px;
    font-size: 16px;
  }
  p {
    margin: 0 0 12px;
    color: var(--fg-muted);
  }
  code {
    overflow-wrap: anywhere;
  }
  .status {
    color: var(--fg);
  }
  .failure {
    color: var(--danger);
  }
  .bar {
    display: inline-block;
    width: 120px;
    height: 4px;
    overflow: hidden;
    border-radius: 2px;
    background: var(--bg-hover);
    vertical-align: middle;
  }
  .fill {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .buttons {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    margin-top: 4px;
  }
</style>
