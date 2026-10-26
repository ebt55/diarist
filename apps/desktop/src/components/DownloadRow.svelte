<script lang="ts">
  // One download (runtime package or model) with everything needed to get
  // it some way or other: progress, Retry (resumes), Copy link, the expected
  // SHA-256 and size, and Open folder for a file fetched by hand.
  import type { Snippet } from "svelte";
  import { api, errorText } from "../lib/api";
  import { copyLinks, downloads, failureHint, sizeMb } from "../lib/downloads.svelte";
  import { formatBytes } from "../lib/format";
  import type { DownloadItem } from "../lib/generated/DownloadItem";
  import { app } from "../lib/state.svelte";

  interface Props {
    item: DownloadItem;
    /** Retry after a failure (and Install for a row that has no own button). */
    onretry: () => void;
    /** Opens the folder the file belongs in (default: the item's folder). */
    onopen?: () => void;
    /** A button shown while the item is not installed (Install, Download). */
    action?: Snippet;
    /** Hide the address, hash and folder line while installed. */
    tidy?: boolean;
  }
  let { item, onretry, onopen, action, tidy = true }: Props = $props();

  let now = $state(Date.now());
  $effect(() => {
    if (downloads.of(item.name)?.state !== "running") return;
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });

  const p = $derived(downloads.of(item.name));
  const installed = $derived(item.installed || p?.state === "done");
  const total = $derived(p?.total ?? (item.size_bytes || null));
  const pct = $derived(p && total ? Math.min(100, Math.floor((p.done / total) * 100)) : 0);
  const idle = $derived.by(() => {
    void now;
    return downloads.idleSecs(item.name);
  });
  const speed = $derived.by(() => {
    void now;
    return downloads.speed(item.name);
  });

  /** The files of a multi-file download (a punctuation model), in download order; empty for one file. */
  const files = $derived(item.files ?? []);

  async function copyLink() {
    try {
      if (files.length > 0) {
        const n = await copyLinks([item]);
        app.toast(
          "info",
          `${n} links copied. Download each file with a browser into the folder its row shows (keep the subfolders), then press Refresh.`,
        );
        return;
      }
      await navigator.clipboard.writeText(item.url);
      app.toast("info", "Link copied. Download it with a browser, put it into the folder Open folder shows, then press Refresh.");
    } catch (e) {
      app.error(`Could not copy the link (${errorText(e)}); select the address and copy it by hand`);
    }
  }

  async function copyFileLink(url: string) {
    try {
      await navigator.clipboard.writeText(url);
      app.toast("info", "Link copied. Download it with a browser into the folder its row shows, then press Refresh.");
    } catch (e) {
      app.error(`Could not copy the link (${errorText(e)}); select the address and copy it by hand`);
    }
  }

  function open() {
    if (onopen) return onopen();
    api.openDownloadFolder(item.name).catch((e) =>
      app.error(`Could not open the folder (${errorText(e)}); it is ${item.dest}`),
    );
  }
</script>

<li class="dl" class:failed={p?.state === "failed"}>
  <div class="main">
    <span class="name">{item.label}</span>
    <span class="meta">
      {item.size_bytes ? sizeMb(item.size_bytes) : "size unknown"}{item.licence ? ` · ${item.licence}` : ""}{item.backend &&
      !item.tested
        ? " · untested by us"
        : ""}{item.experimental ? " · experimental" : ""}
    </span>
  </div>
  <div class="state">
    {#if installed}
      <span class="ok">✓ Installed</span>
    {:else if p?.state === "running"}
      <span class="bar" role="progressbar" aria-label="{item.label} download" aria-valuenow={pct} aria-valuemin="0" aria-valuemax="100"
        ><span class="fill" style="width: {pct}%"></span></span
      >
      <span class="mono small">
        {pct}% · {formatBytes(p.done)}{speed !== null ? ` · ${(speed / 1048576).toFixed(1)} MB/s` : ""}{pct === 100
          ? " · checking"
          : ""}
      </span>
      {#if idle >= 20 && pct < 100}<span class="warn small">no data for {idle} s</span>{/if}
    {:else if p?.state === "waiting"}
      <span class="muted">Waiting</span>
    {:else if p?.state === "failed"}
      <span class="err">Failed</span>
      <button type="button" class="btn small-btn" onclick={onretry} disabled={downloads.busy}>Retry</button>
    {:else if action}
      {@render action()}
    {:else}
      <span class="muted">Not installed</span>
    {/if}
  </div>
  {#if p?.state === "failed"}
    <div class="error-box">
      <p class="err">{p.error}</p>
      <p class="hint">{failureHint(item.kind, p.error)}</p>
    </div>
  {/if}
  {#if !(installed && tidy) && files.length > 0}
    <div class="fetch">
      <span class="label">Files</span>
      <span class="val">{files.length} files, {item.size_bytes.toLocaleString()} bytes in all → <code class="sel">{item.dest}</code></span>
      <span class="buttons">
        <button type="button" class="link" onclick={copyLink}>Copy all links</button>
        <button type="button" class="link" onclick={open}>Open folder</button>
      </span>
    </div>
    <ul class="files" aria-label="Files of {item.label}">
      {#each files as f (f.path)}
        <li class="fetch file">
          <span class="label">File</span>
          <span class="val"><code class="sel">{f.path}</code>{#if f.present} <span class="ok">✓ there</span>{/if}</span>
          <span class="buttons">
            <button type="button" class="link" onclick={() => copyFileLink(f.url)} aria-label="Copy link of {f.path}">Copy link</button>
          </span>
          <span class="label">Address</span>
          <span class="val"><code class="sel" title={f.url}>{f.url}</code></span>
          <span></span>
          <span class="label">SHA-256</span>
          <span class="val"><code class="sel">{f.sha256 || "not given (recorded after the download)"}</code></span>
          <span></span>
          <span class="label">Size</span>
          <span class="val mono">{f.size_bytes.toLocaleString()} bytes → <code class="sel">{f.dest}</code></span>
          <span></span>
        </li>
      {/each}
    </ul>
  {:else if !(installed && tidy)}
    <div class="fetch">
      <span class="label">Address</span>
      <span class="val"><code class="sel" title={item.url}>{item.url}</code></span>
      <span class="buttons">
        <button type="button" class="link" onclick={copyLink}>Copy link</button>
        <button type="button" class="link" onclick={open}>Open folder</button>
      </span>
      <span class="label">SHA-256</span>
      <span class="val"><code class="sel">{item.sha256 || "not given (recorded after the download)"}</code></span>
      <span></span>
      <span class="label">Size</span>
      <span class="val mono">{item.size_bytes ? `${item.size_bytes.toLocaleString()} bytes` : "unknown"} → <code class="sel">{item.dest}</code></span>
      <span></span>
    </div>
  {/if}
</li>

<style>
  .dl {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(200px, auto);
    gap: 4px 12px;
    align-items: center;
    padding: 7px 0;
    border-top: 1px solid var(--border);
  }
  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .name {
    font-weight: 600;
  }
  .meta,
  .muted,
  .hint {
    color: var(--fg-muted);
    font-size: 12px;
  }
  .state {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 8px;
    align-items: center;
    justify-content: flex-end;
    font-size: 12px;
  }
  .bar {
    display: inline-block;
    width: 140px;
    height: 5px;
    overflow: hidden;
    border-radius: 3px;
    background: var(--bg-hover);
  }
  .fill {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.25s linear;
  }
  .small {
    font-size: 11px;
  }
  .small-btn {
    padding: 2px 10px;
  }
  .ok {
    color: var(--ok-fg);
  }
  .err {
    margin: 0;
    color: var(--danger);
  }
  .warn {
    color: var(--warning-fg);
  }
  .error-box {
    grid-column: 1 / -1;
    padding: 6px 10px;
    border-radius: var(--radius);
    background: var(--danger-bg);
  }
  .error-box p {
    margin: 2px 0;
  }
  .fetch {
    display: grid;
    grid-column: 1 / -1;
    grid-template-columns: max-content minmax(0, 1fr) auto;
    gap: 1px 10px;
    font-size: 11px;
  }
  .label {
    color: var(--fg-muted);
  }
  .val {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .buttons {
    display: flex;
    gap: 10px;
  }
  .sel {
    user-select: all;
  }
  .files {
    grid-column: 1 / -1;
    margin: 2px 0 0;
    padding: 0;
    list-style: none;
  }
  .file {
    padding: 3px 0 3px 10px;
    border-left: 2px solid var(--border);
  }
  .file + .file {
    margin-top: 3px;
  }
</style>
