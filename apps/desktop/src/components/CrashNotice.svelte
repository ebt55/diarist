<script lang="ts">
  // The quiet notice after a crash (docs/plans/11-hardening.md 8a): once per
  // report, until the user acts on it or dismisses it. It sits at the bottom
  // of the toast stack (Overlays.svelte), so it never covers a page header.
  import { onMount } from "svelte";
  import { api, errorText } from "../lib/api";
  import { crashApi, DUMP_NOTE, type CrashReport } from "../lib/crash";
  import { formatDate } from "../lib/format";
  import { app } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  let report = $state.raw<CrashReport | null>(null);

  onMount(() => {
    crashApi
      .notice()
      .then((r) => (report = r))
      .catch((e) => void api.log("warn", `crash notice: ${errorText(e)}`));
  });

  function acknowledge(r: CrashReport) {
    crashApi.acknowledge(r.name).catch((e) => void api.log("warn", `crash notice not saved: ${errorText(e)}`));
  }

  async function openFolder(r: CrashReport) {
    acknowledge(r);
    try {
      await crashApi.reveal(r.name);
    } catch (e) {
      app.error(`Could not open the log folder (${errorText(e)}); Diagnostics has Open log folder`);
    }
  }

  async function copy(r: CrashReport) {
    acknowledge(r);
    try {
      await navigator.clipboard.writeText(await crashApi.text(r.name));
      app.toast("info", "Report copied, with your user folder shown as ~. Nothing was sent; paste it into an issue if you like");
    } catch (e) {
      app.error(`Could not copy the report (${errorText(e)}); open the folder and attach the .txt file instead`);
    }
  }

  function dismiss(r: CrashReport) {
    acknowledge(r);
    report = null;
  }
</script>

{#if report}
  {@const r = report}
  <div class="crash-notice" role="status" aria-live="polite">
    <p class="msg">Diarist closed unexpectedly at {formatDate(r.at_ms)}. A report is on your disk.</p>
    {#if r.dump}<p class="note">{DUMP_NOTE}</p>{/if}
    <div class="actions">
      <button type="button" class="btn" onclick={() => openFolder(r)}><Icon name="folder" /> Open folder</button>
      {#if r.name.endsWith(".txt")}
        <button type="button" class="btn" onclick={() => copy(r)}><Icon name="copy" /> Copy report</button>
      {/if}
      <button type="button" class="btn" onclick={() => dismiss(r)}>Dismiss</button>
    </div>
  </div>
{/if}

<style>
  .crash-notice {
    max-width: 440px;
    padding: 10px 12px;
    border: 1px solid var(--border-strong);
    border-left: 4px solid var(--warning-border);
    border-radius: var(--radius);
    background: var(--bg-raised);
    box-shadow: var(--shadow);
    pointer-events: auto;
  }
  .msg {
    margin: 0 0 8px;
  }
  .note {
    margin: 0 0 8px;
    color: var(--fg-muted);
    font-size: 12px;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
</style>
