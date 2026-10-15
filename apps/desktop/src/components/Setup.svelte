<script lang="ts">
  // First run: GPU and disk checks, then the engine and the recommended
  // models with per-item progress (resumed and SHA-checked by the backend).
  // Shown at launch while no engine, speech model or diarizer is present.
  import { onMount } from "svelte";
  import { api, errorText } from "../lib/api";
  import { downloads, sizeMb } from "../lib/downloads.svelte";
  import { formatBytes, formatVram, plural } from "../lib/format";
  import type { DownloadItem } from "../lib/generated/DownloadItem";
  import type { SetupStatus } from "../lib/generated/SetupStatus";
  import { app } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  let status = $state.raw<SetupStatus | null>(null);
  let error = $state<string | null>(null);
  let checking = $state(false);
  let manual = $state(false);
  // Re-render progress speeds once a second while something downloads.
  let tick = $state(0);

  const engine = $derived(status?.items.find((i) => i.kind === "engine") ?? null);
  const models = $derived(status?.items.filter((i) => i.kind === "model" && i.required) ?? []);
  const modelsBytes = $derived(models.reduce((n, m) => n + m.size_bytes, 0));
  const modelsMissing = $derived(models.filter((m) => !m.installed));
  const gpu = $derived(status?.gpu ?? null);
  const diskShort = $derived(
    status !== null &&
      status.disk_free_bytes !== null &&
      status.disk_free_bytes < status.disk_needed_bytes + 500 * 1048576,
  );

  async function refresh(): Promise<void> {
    checking = true;
    try {
      status = await api.setupStatus();
      error = null;
      app.setupNeeded = status.needs_setup;
    } catch (e) {
      error = errorText(e);
    } finally {
      checking = false;
    }
  }

  onMount(() => {
    void refresh();
    const onRefresh = () => void refresh();
    window.addEventListener("app:refresh", onRefresh);
    const t = setInterval(() => downloads.busy && tick++, 1000);
    return () => {
      window.removeEventListener("app:refresh", onRefresh);
      clearInterval(t);
    };
  });

  async function install(items: DownloadItem[]): Promise<void> {
    const failed = await downloads.install(items);
    await refresh();
    if (failed > 0) {
      app.error(`${plural(failed, "download")} failed; the setup screen shows the address and hash to fetch it by hand`);
    } else if (status && !status.needs_setup) {
      app.toast("success", "meet-assist is ready: import a recording with Ctrl+O, or press R to record");
    }
  }

  async function haveFiles(): Promise<void> {
    manual = true;
    try {
      await api.openEngineFolder();
      await api.openModelsFolder();
    } catch (e) {
      app.error(`Could not open the folders: ${errorText(e)}`);
    }
  }

  function pct(item: DownloadItem): number | null {
    const p = downloads.of(item.name);
    const total = p?.total ?? item.size_bytes;
    if (!p || !total) return null;
    return Math.min(100, Math.floor((p.done / total) * 100));
  }

  function speedText(item: DownloadItem): string {
    void tick;
    const s = downloads.speed(item.name);
    return s === null ? "" : ` · ${(s / 1048576).toFixed(1)} MB/s`;
  }
</script>

{#snippet row(item: DownloadItem)}
  {@const p = downloads.of(item.name)}
  {@const percent = pct(item)}
  <li class="item" class:failed={p?.state === "failed"}>
    <div class="item-main">
      <span class="item-name">{item.label}</span>
      <span class="item-meta mono">{sizeMb(item.size_bytes)} · {item.licence}</span>
    </div>
    <div class="item-state">
      {#if item.installed || p?.state === "done"}
        <span class="ok-text">✓ Installed</span>
      {:else if p?.state === "running"}
        <span class="bar" role="progressbar" aria-valuenow={percent ?? 0} aria-valuemin="0" aria-valuemax="100"
          ><span class="fill" style="width: {percent ?? 0}%"></span></span
        >
        <span class="mono small"
          >{percent ?? 0}% · {formatBytes(p.done)}{speedText(item)}{percent === 100 ? " · verifying" : ""}</span
        >
      {:else if p?.state === "waiting"}
        <span class="muted">Waiting</span>
      {:else if p?.state === "failed"}
        <span class="error-text">Failed</span>
      {:else}
        <span class="muted">Not installed</span>
      {/if}
    </div>
    {#if p?.state === "failed"}
      <div class="item-error">
        <p class="error-text">{p.error}</p>
        <p class="help">
          Download it by hand and put it in the folder below, then press Refresh. A partial download resumes when you
          try again.
        </p>
        <dl class="fetch">
          <dt>Address</dt>
          <dd><code class="sel">{item.url}</code></dd>
          <dt>SHA-256</dt>
          <dd><code class="sel">{item.sha256}</code></dd>
          <dt>{item.kind === "engine" ? "Extract into" : "Folder"}</dt>
          <dd><code class="sel">{item.dest}</code></dd>
        </dl>
      </div>
    {/if}
  </li>
{/snippet}

<div class="page">
  <header class="page-head">
    <h1>Set up meet-assist</h1>
    <span class="hint">Esc goes to the library · Ctrl+R checks again</span>
  </header>
  <p class="lead">
    Transcription runs on this computer's GPU. The speech engine and the models download once, from GitHub and
    Hugging Face, and every file is checked against its published SHA-256 before it is used. Nothing else leaves the
    machine.
  </p>

  {#if error}
    <p class="error-text">Could not check this computer: {error}</p>
  {/if}

  {#if status}
    <section class="checks" aria-label="Checks">
      <div class="check" class:warn={!gpu || !gpu.cuda_ok}>
        <h2>GPU</h2>
        {#if gpu}
          <p>
            <strong>{gpu.name}</strong> · {formatVram(gpu.vram_total_mb)} VRAM ({formatVram(gpu.vram_free_mb)} free) · driver
            {gpu.driver_version || "?"}{gpu.cuda_version ? ` · CUDA ${gpu.cuda_version}` : ""}
          </p>
          {#if !gpu.cuda_ok}
            <p class="warn-text">
              The engine needs a driver with CUDA 13 (NVIDIA R580 or newer). Update the driver from nvidia.com, then
              press Check again.
            </p>
          {:else if gpu.vram_total_mb < 5000}
            <p class="help">
              With {formatVram(gpu.vram_total_mb)}, file transcription fits; live recording is tight next to other
              GPU apps. Settings has a low-VRAM mode.
            </p>
          {:else}
            <p class="ok-text">✓ Fits file transcription and live recording.</p>
          {/if}
        {:else}
          <p class="warn-text">
            No NVIDIA GPU found. This build's engine is the CUDA package and needs an NVIDIA GPU with a current
            driver. The runtime's Vulkan and CPU packages exist but are untested (docs/INSTALL.md).
          </p>
        {/if}
      </div>
      <div class="check" class:warn={diskShort}>
        <h2>Disk</h2>
        <p>
          {status.disk_free_bytes === null ? "Free space unknown" : `${formatBytes(status.disk_free_bytes)} free`} for
          <code class="path" title={status.data_root}>{status.data_root}</code>
        </p>
        {#if status.disk_needed_bytes > 0}
          <p class={diskShort ? "warn-text" : "help"}>
            The downloads below need about {sizeMb(status.disk_needed_bytes)}{diskShort
              ? ". Free some space or move the data folder in Settings first."
              : "."}
          </p>
        {/if}
      </div>
    </section>

    <section class="step">
      <div class="step-head">
        <span class="num" class:done={engine?.installed}>1</span>
        <div class="step-text">
          <h2>Speech engine</h2>
          <p class="help">NVIDIA NeMo-Speech.cpp, the CUDA build: a portable folder, no CUDA toolkit or Python needed.</p>
        </div>
        {#if engine && !engine.installed}
          <button
            type="button"
            class="btn btn-primary"
            onclick={() => engine && install([engine])}
            disabled={downloads.busy}
          >
            <Icon name="download" /> Install engine ({sizeMb(engine.size_bytes)})
          </button>
        {/if}
      </div>
      {#if engine}<ul class="items">{@render row(engine)}</ul>{/if}
    </section>

    <section class="step">
      <div class="step-head">
        <span class="num" class:done={modelsMissing.length === 0}>2</span>
        <div class="step-text">
          <h2>Speech and speaker models</h2>
          <p class="help">
            Who spoke when (up to 8 speakers), and English speech to text. For other or mixed languages add Nemotron
            3.5 in Settings, Models.
          </p>
        </div>
        {#if modelsMissing.length > 0}
          <button
            type="button"
            class="btn btn-primary"
            onclick={() => install(engine && !engine.installed ? [engine, ...models] : models)}
            disabled={downloads.busy}
            title={engine && !engine.installed ? "Installs the engine first" : ""}
          >
            <Icon name="download" /> Install recommended models ({sizeMb(modelsBytes)}: diarizer and the EN model)
          </button>
        {/if}
      </div>
      <ul class="items">
        {#each models as m (m.name)}{@render row(m)}{/each}
      </ul>
    </section>

    {#if !status.needs_setup}
      <section class="ready">
        <p class="ok-text"><strong>✓ Ready.</strong> Import a recording with <kbd>Ctrl O</kbd> or drop files anywhere; press <kbd>R</kbd> to record a meeting.</p>
        <button type="button" class="btn btn-primary" onclick={() => (app.route = "library")}>Open the library</button>
      </section>
    {/if}

    <section class="manual">
      <div class="row">
        <button type="button" class="btn" onclick={haveFiles}><Icon name="folder" /> I already have the files</button>
        <button type="button" class="btn" onclick={refresh} disabled={checking}>
          <Icon name="refresh" />{checking ? "Checking…" : "Check again"}
        </button>
        {#if status.needs_setup}
          <button type="button" class="link" onclick={() => (app.route = "library")}>Skip for now</button>
        {/if}
      </div>
      {#if manual}
        <p class="help">
          Extract the runtime zip so that <code>bin\nemo-speech.exe</code> sits in a folder under
          <code class="sel">{status.engine_dir}</code>, and put the GGUF model files anywhere under
          <code class="sel">{status.models_dir}</code> (subfolders are fine). Then press Check again: the app reads each
          file's header to know what it is.
        </p>
      {/if}
    </section>
  {:else if !error}
    <p class="muted">Checking this computer…</p>
  {/if}
</div>

<style>
  .page {
    height: 100%;
    overflow-y: auto;
    padding: 16px 24px 48px;
  }
  .page > * {
    max-width: 860px;
  }
  .page-head {
    display: flex;
    gap: 12px;
    align-items: baseline;
  }
  h1 {
    margin: 0 0 6px;
    font-size: 20px;
  }
  h2 {
    margin: 0 0 4px;
    font-size: 15px;
  }
  .hint,
  .muted {
    color: var(--fg-muted);
    font-size: 12px;
  }
  .lead {
    margin: 0 0 14px;
    color: var(--fg-muted);
  }
  .checks {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: 10px;
    margin-bottom: 14px;
  }
  .check {
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .check.warn {
    border-color: var(--warning-border);
    background: var(--warning-bg);
  }
  .check p {
    margin: 2px 0;
  }
  .step {
    margin: 0 0 14px;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .step-head {
    display: flex;
    flex-wrap: wrap;
    gap: 10px 12px;
    align-items: center;
  }
  .step-text {
    flex: 1;
    min-width: 220px;
  }
  .step-text p {
    margin: 0;
  }
  .num {
    display: inline-flex;
    flex: none;
    justify-content: center;
    align-items: center;
    width: 26px;
    height: 26px;
    border-radius: 50%;
    background: var(--bg-hover);
    font-weight: 700;
  }
  .num.done {
    background: var(--ok-bg);
    color: var(--ok-fg);
  }
  .items {
    margin: 10px 0 0;
    padding: 0;
    list-style: none;
  }
  .item {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(180px, 0.9fr);
    gap: 4px 12px;
    align-items: center;
    padding: 6px 0;
    border-top: 1px solid var(--border);
  }
  .item-main {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .item-name {
    font-weight: 600;
  }
  .item-meta {
    color: var(--fg-muted);
    font-size: 11px;
  }
  .item-state {
    display: flex;
    gap: 8px;
    align-items: center;
    font-size: 12px;
  }
  .item-error {
    grid-column: 1 / -1;
    padding: 6px 10px;
    border-radius: var(--radius);
    background: var(--danger-bg);
  }
  .item-error p {
    margin: 2px 0;
  }
  .fetch {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 2px 10px;
    margin: 6px 0 2px;
    font-size: 12px;
  }
  .fetch dt {
    color: var(--fg-muted);
  }
  .fetch dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .sel {
    user-select: all;
  }
  .bar {
    display: inline-block;
    flex: 1;
    max-width: 160px;
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
  .ready {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 14px;
    padding: 10px 12px;
    border-radius: var(--radius);
    background: var(--ok-bg);
  }
  .ready p {
    margin: 0;
  }
  .manual .row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }
  .path {
    overflow-wrap: anywhere;
  }
  .help {
    margin: 6px 0 0;
    color: var(--fg-muted);
    font-size: 12px;
  }
  .ok-text {
    color: var(--ok-fg);
  }
  .warn-text {
    color: var(--warning-fg);
  }
  .error-text {
    color: var(--danger);
  }
</style>
