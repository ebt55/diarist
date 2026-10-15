<script lang="ts">
  // What Import or Record still needs, checked when the user asks for it
  // (never a dead button): each missing part with its one-click fix, Copy
  // link and Open folder. When everything is there, the action continues.
  import { api, errorText } from "../lib/api";
  import { copyLinks, downloads } from "../lib/downloads.svelte";
  import { plural } from "../lib/format";
  import type { DownloadItem } from "../lib/generated/DownloadItem";
  import { live } from "../lib/live.svelte";
  import { app } from "../lib/state.svelte";
  import DownloadRow from "./DownloadRow.svelte";
  import Icon from "./Icon.svelte";

  let checking = $state(false);
  const r = $derived(app.readiness);
  const items = $derived(
    (r?.missing.map((m) => m.item).filter((i): i is DownloadItem => i !== null) ?? []).filter(
      (i, n, all) => all.findIndex((x) => x.name === i.name) === n,
    ),
  );

  async function recheck(): Promise<void> {
    if (!r) return;
    checking = true;
    try {
      const next = await api.checkReady(r.action as "import" | "record");
      if (next.ok) {
        const pending = r.pending;
        app.readiness = null;
        if (r.action === "import" && pending.length > 0) await app.importPaths(pending);
        else if (r.action === "record") live.sheetOpen = true;
      } else {
        app.readiness = { ...r, missing: next.missing };
      }
    } catch (e) {
      app.error(`Could not check again (${errorText(e)}); see Diagnostics, Why isn't it working`);
    } finally {
      checking = false;
    }
  }

  async function install(list: DownloadItem[]): Promise<void> {
    const engine = list.find((i) => i.kind === "engine");
    const todo = engine ? [engine, ...list.filter((i) => i !== engine)] : list;
    const failed = await downloads.install(todo);
    if (failed === 0) await recheck();
  }

  async function copyAll(): Promise<void> {
    try {
      const n = await copyLinks(items);
      app.toast("info", `${plural(n, "link")} copied. Put each file into the folder its row shows, then press Check again.`);
    } catch (e) {
      app.error(`Could not copy the links (${errorText(e)}); copy them from each row`);
    }
  }
</script>

{#if r}
  <div class="panel" role="region" aria-label="What is missing">
    <div class="head">
      <h2>
        {r.action === "import"
          ? `To transcribe ${r.pending.length > 1 ? `${r.pending.length} files` : "this file"}, meet-assist still needs:`
          : "To record, meet-assist still needs:"}
      </h2>
      <button type="button" class="btn-icon" aria-label="Close" onclick={() => (app.readiness = null)}><Icon name="close" /></button>
    </div>
    <ul class="parts">
      {#each r.missing as m (m.what)}
        <li>
          <p>{m.message}</p>
          {#if m.item}
            {@const item = m.item}
            <ul class="rows">
              <DownloadRow {item} onretry={() => install([item])}>
                {#snippet action()}
                  <button type="button" class="btn btn-primary" onclick={() => install([item])} disabled={downloads.busy}>
                    <Icon name="download" />{item.kind === "engine" ? "Install" : "Download"}
                  </button>
                {/snippet}
              </DownloadRow>
            </ul>
          {:else if m.what === "streaming"}
            <button type="button" class="link" onclick={() => ((app.readiness = null), (app.route = "settings"))}>Open Settings, Models</button>
          {:else}
            <button type="button" class="link" onclick={() => api.openModelsFolder().catch(() => {})}>Open models folder</button>
          {/if}
        </li>
      {/each}
    </ul>
    <div class="actions">
      {#if items.length > 1}
        <button type="button" class="btn btn-primary" onclick={() => install(items)} disabled={downloads.busy}>
          <Icon name="download" /> Get all of it
        </button>
      {/if}
      {#if items.length > 0}
        <button type="button" class="btn" onclick={copyAll}><Icon name="copy" /> Copy all links</button>
      {/if}
      <button type="button" class="btn" onclick={recheck} disabled={checking}>
        <Icon name="refresh" />{checking ? "Checking…" : "Check again"}
      </button>
      <button type="button" class="link" onclick={() => ((app.readiness = null), (app.route = "diagnostics"))}>Why isn't it working?</button>
    </div>
  </div>
{/if}

<style>
  .panel {
    position: absolute;
    z-index: 5;
    top: 12px;
    right: 16px;
    left: 16px;
    max-height: calc(100% - 24px);
    overflow-y: auto;
    padding: 12px 14px;
    border: 1px solid var(--warning-border);
    border-radius: var(--radius);
    background: var(--bg-raised);
    box-shadow: var(--shadow);
  }
  .head {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    justify-content: space-between;
  }
  h2 {
    margin: 0 0 6px;
    font-size: 15px;
  }
  .parts {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .parts > li {
    padding: 6px 0;
  }
  .parts p {
    margin: 0 0 2px;
  }
  .rows {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    margin-top: 8px;
  }
</style>
