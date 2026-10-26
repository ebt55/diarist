<script lang="ts">
  // "Recommended downloads available" (plan 16 sections 3.8 and 4.3): once
  // per data root, after an update, what is new to download (punctuation;
  // chunk G adds the corrected speech models). The content comes from the
  // backend (`recommended_downloads_notice`); this only shows it. Upgraders
  // never see the setup screen, so this is where they hear of it. Download
  // now or Later answers it for good; Settings, Models keeps every download.
  // It sits in the toast stack (Overlays.svelte), like the crash notice;
  // Escape inside it means Later.
  import { api, errorText, punctApi } from "../lib/api";
  import { downloads, sizeMb } from "../lib/downloads.svelte";
  import type { DownloadsNotice } from "../lib/punctuation";
  import { app } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  let notice = $state.raw<DownloadsNotice | null>(null);
  let asked = false;
  let busy = $state(false);
  let failed = $state(false);

  // After the first-run check: not on a first run (the setup screen offers
  // everything) and not while the move of a pre-rename data root is offered.
  $effect(() => {
    if (app.setupNeeded !== false || asked || app.startup?.legacy_root) return;
    asked = true;
    punctApi
      .downloadsNotice()
      .then((n) => (notice = n))
      .catch((e) => void api.log("warn", `downloads notice: ${errorText(e)}`));
  });

  const items = $derived(notice?.parts.flatMap((p) => p.items) ?? []);
  const total = $derived(items.reduce((n, i) => n + i.size_bytes, 0));
  const running = $derived(items.find((i) => downloads.of(i.name)?.state === "running") ?? null);
  const pct = $derived.by(() => {
    const p = running ? downloads.of(running.name) : null;
    return p && p.total ? Math.min(100, Math.floor((p.done / p.total) * 100)) : 0;
  });

  function answer(n: DownloadsNotice) {
    void app.setSetting(n.key, "true");
  }

  async function download() {
    const n = notice;
    if (!n || busy) return;
    answer(n);
    busy = true;
    failed = false;
    try {
      const bad = await downloads.install(items);
      if (bad === 0) {
        notice = null;
        app.toast("success", "Downloaded and checked. New transcripts use it from now on.");
      } else {
        failed = true;
      }
    } catch (e) {
      failed = true;
      void api.log("error", `downloads notice: ${errorText(e)}`);
    } finally {
      busy = false;
    }
  }

  function later() {
    const n = notice;
    if (!n) return;
    answer(n);
    notice = null;
    app.toast("info", "Settings, Models has these downloads whenever you want them.");
  }

  function openSettings() {
    notice = null;
    app.route = "settings";
  }
</script>

{#if notice}
  <!-- Escape from any of its buttons means Later (the keys reach the buttons, not the region). -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="dl-notice"
    role="region"
    aria-label="Recommended downloads"
    onkeydown={(e) => {
      if (e.key === "Escape" && !busy) {
        e.preventDefault();
        e.stopPropagation();
        later();
      }
    }}
  >
    {#each notice.parts as part (part.id)}
      <p class="msg">
        <strong>{part.title}</strong>
        {part.text}
      </p>
      <ul class="items">
        {#each part.items as item (item.name)}
          <li>{item.label.split(":")[0]} · {sizeMb(item.size_bytes)}</li>
        {/each}
      </ul>
    {/each}
    {#if busy}
      <p class="note" role="status">
        Downloading{running ? ` ${running.label.split(":")[0]}` : ""}… {pct}%. You can keep working.
      </p>
    {:else if failed}
      <p class="note error" role="alert">
        The download did not finish. Retry resumes it; Settings, Models has Copy link and the folder for a download by hand.
      </p>
    {/if}
    <div class="actions">
      {#if failed}
        <button type="button" class="btn btn-primary" onclick={download} disabled={busy || downloads.busy}>Retry</button>
        <button type="button" class="btn" onclick={openSettings}>Open Settings</button>
        <button type="button" class="btn" onclick={() => (notice = null)}>Close</button>
      {:else}
        <button type="button" class="btn btn-primary" onclick={download} disabled={busy || downloads.busy}>
          <Icon name="download" /> Download now ({sizeMb(total)})
        </button>
        <button type="button" class="btn" onclick={later} disabled={busy}>Later</button>
      {/if}
    </div>
  </div>
{/if}

<style>
  .dl-notice {
    max-width: 440px;
    padding: 10px 12px;
    border: 1px solid var(--border-strong);
    border-left: 4px solid var(--accent);
    border-radius: var(--radius);
    background: var(--bg-raised);
    box-shadow: var(--shadow);
    pointer-events: auto;
  }
  .msg {
    margin: 0 0 4px;
  }
  .items {
    margin: 0 0 8px;
    padding-left: 18px;
    color: var(--fg-muted);
    font-size: 12px;
  }
  .note {
    margin: 0 0 8px;
    color: var(--fg-muted);
    font-size: 12px;
  }
  .note.error {
    color: var(--danger);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
</style>
