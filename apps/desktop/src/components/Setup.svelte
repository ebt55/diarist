<script lang="ts">
  // First run (docs/00, "nobody fights the app"): GPU and disk checks, the
  // runtime package for this GPU (CUDA, Vulkan or CPU, overridable), the
  // recommended models, each with progress, Retry, Copy link, hash and
  // folder. Nothing here is a gate: "Set up later" goes to the library, and
  // Import and Record say again what they miss when they are used.
  import { onMount } from "svelte";
  import { api, errorText, langApi } from "../lib/api";
  import { copyLinks, downloads, sizeMb } from "../lib/downloads.svelte";
  import { formatBytes, formatVram, plural } from "../lib/format";
  import type { DownloadItem } from "../lib/generated/DownloadItem";
  import type { SetupStatus } from "../lib/generated/SetupStatus";
  import { SETTING_DEFAULTS } from "../lib/generated/settingDefaults";
  import {
    LANGUAGE_TIERS,
    MULTILINGUAL_MODEL,
    canonical,
    offeredLanguages,
    primarySubtag,
    systemLanguage,
    type LanguageTable,
  } from "../lib/languages";
  import { app } from "../lib/state.svelte";
  import DownloadRow from "./DownloadRow.svelte";
  import Icon from "./Icon.svelte";
  import RuntimeReleases from "./RuntimeReleases.svelte";

  let status = $state.raw<SetupStatus | null>(null);
  let error = $state<string | null>(null);
  let checking = $state(false);
  let manual = $state(false);
  let chosen = $state<string | null>(null);
  let showReleases = $state(false);
  /** The meetings' usual language: English gets the EN model, any other Nemotron 3.5. */
  let meetingLanguage = $state(canonical(app.setting("default_language")));
  let languageTable = $state.raw<LanguageTable>(LANGUAGE_TIERS);
  /** Manifest name of the default English model (`manifests/models.json`). */
  const EN_MODEL = "nemotron-en";
  const english = $derived(primarySubtag(meetingLanguage) === "en");

  const engines = $derived(status?.items.filter((i) => i.kind === "engine") ?? []);
  const engine = $derived(
    engines.find((e) => e.installed) ??
      engines.find((e) => e.backend === (chosen ?? status?.recommended_backend)) ??
      engines[0] ??
      null,
  );
  const engineInstalled = $derived(status?.engine_ready ?? false);
  const models = $derived(
    status?.items.filter(
      (i) =>
        i.kind === "model" &&
        (english ? i.required : (i.required && i.name !== EN_MODEL) || i.name === MULTILINGUAL_MODEL),
    ) ?? [],
  );
  const modelsBytes = $derived(models.reduce((n, m) => n + m.size_bytes, 0));
  /** Required models besides the diarizer and the speech model (from beta.5, punctuation), by short name. */
  const DIARIZER_MODEL = "nemotron-3-diarization";
  const extras = $derived(
    models
      .filter((m) => m.name !== EN_MODEL && m.name !== MULTILINGUAL_MODEL && m.name !== DIARIZER_MODEL)
      .map((m) => m.label.split(":")[0]?.trim() || m.name),
  );
  const modelsMissing = $derived(models.filter((m) => !m.installed));
  const missingAll = $derived([...(engine && !engineInstalled ? [engine] : []), ...modelsMissing]);
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
      error = `${errorText(e)}. Press Check again; if it keeps failing, Diagnostics lists what is wrong.`;
    } finally {
      checking = false;
    }
  }

  onMount(() => {
    void langApi
      .languageTable()
      .then((t) => (languageTable = t))
      .catch(() => {})
      .finally(() => void refresh().then(seedLanguage));
    const onRefresh = () => void refresh();
    window.addEventListener("app:refresh", onRefresh);
    return () => window.removeEventListener("app:refresh", onRefresh);
  });

  async function install(items: DownloadItem[]): Promise<void> {
    const failed = await downloads.install(items);
    if (!english && items.some((i) => i.name === MULTILINGUAL_MODEL)) await selectMultilingual();
    await refresh();
    if (failed > 0) {
      app.error(`${plural(failed, "download")} failed. Each row has Retry, Copy link and Open folder`);
    } else if (status && !status.needs_setup) {
      app.toast("success", "Diarist is ready: import a recording with Ctrl+O, or press R to record");
    }
  }

  async function haveFiles(): Promise<void> {
    manual = true;
    try {
      await api.openEngineFolder();
      await api.openModelsFolder();
    } catch (e) {
      app.error(`Could not open the folders (${errorText(e)}); they are listed below`);
    }
  }

  async function copyAll(): Promise<void> {
    try {
      const n = await copyLinks(missingAll);
      app.toast("info", `${plural(n, "link")} copied. Put each file into the folder its row shows, then press Check again.`);
    } catch (e) {
      app.error(`Could not copy the links (${errorText(e)}); copy them from each row`);
    }
  }

  /**
   * Once, on a first run (nothing installed yet): the meeting language from
   * the system language, unless the default was changed already.
   */
  async function seedLanguage(): Promise<void> {
    await app.ready;
    if (!status?.needs_setup || app.settingBool("setup.language_seeded")) return;
    if (app.setting("default_language") === SETTING_DEFAULTS.default_language) {
      meetingLanguage = systemLanguage(
        navigator.languages?.length ? navigator.languages : [navigator.language],
        languageTable,
      );
      await app.setSetting("default_language", meetingLanguage);
    }
    await app.setSetting("setup.language_seeded", "true");
  }

  /** Meetings in another language: Nemotron 3.5 becomes the selected speech-to-text model. */
  async function selectMultilingual(): Promise<void> {
    try {
      const n35 = (await api.listModelFiles()).find((f) => f.manifest_name === MULTILINGUAL_MODEL && !f.missing);
      if (n35) await api.selectModel("asr", n35.id);
    } catch (e) {
      void api.log("warn", `setup: selecting Nemotron 3.5 failed: ${errorText(e)}`);
    }
  }

  function later(): void {
    void app.setSetting("setup.later", "true");
    app.route = "library";
  }

  const BACKENDS: Record<string, string> = {
    cuda: "CUDA: NVIDIA GPUs with a current driver (tested)",
    vulkan: "Vulkan: AMD, Intel or older NVIDIA drivers (untested by us)",
    cpu: "CPU: no GPU needed, much slower (untested by us)",
  };
</script>

<div class="page">
  <header class="page-head">
    <h1>Set up Diarist</h1>
    <span class="hint">Esc goes to the library · Ctrl+R checks again</span>
  </header>
  <p class="lead">
    Transcription runs on this computer. The speech engine and the models download once, from GitHub and Hugging Face,
    and every file is checked against its published SHA-256. You own the folders: anything you put there yourself is
    used after Check again. Nothing else leaves the machine.
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
              The CUDA engine needs a driver with CUDA 13 (NVIDIA R580 or newer). Update the driver from nvidia.com and
              press Check again, or use the Vulkan runtime below.
            </p>
          {:else if gpu.vram_total_mb < 5000}
            <p class="help">
              With {formatVram(gpu.vram_total_mb)}, file transcription fits; live recording is tight next to other GPU
              apps. Settings has a low-VRAM mode.
            </p>
          {:else}
            <p class="ok-text">✓ Fits file transcription and live recording.</p>
          {/if}
        {:else if status.adapters.length > 0}
          <p><strong>{status.adapters[0]?.name}</strong> (no NVIDIA driver found)</p>
          <p class="warn-text">The Vulkan runtime is preselected; it is untested by us. The CPU runtime always works, slowly.</p>
        {:else}
          <p class="warn-text">No GPU found. Only the CPU runtime works, much slower than real time.</p>
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
              ? ". Free some space, or move the data folder in Settings first."
              : "."}
          </p>
        {/if}
      </div>
    </section>

    <section class="step">
      <div class="step-head">
        <span class="num" class:done={engineInstalled}>1</span>
        <div class="step-text">
          <h2>Speech engine</h2>
          <p class="help">NVIDIA NeMo-Speech.cpp: a portable folder, no CUDA toolkit or Python needed.</p>
        </div>
        {#if engine && !engineInstalled}
          <button type="button" class="btn btn-primary" onclick={() => engine && install([engine])} disabled={downloads.busy}>
            <Icon name="download" /> Install engine ({sizeMb(engine.size_bytes)})
          </button>
        {/if}
      </div>
      {#if !engineInstalled}
        <fieldset class="packages">
          <legend class="help">Package (preselected for this PC; change it if you know better)</legend>
          {#each engines as e (e.name)}
            <label class="radio">
              <input
                type="radio"
                name="package"
                checked={engine?.name === e.name}
                onchange={() => (chosen = e.backend)}
              />
              <span>{BACKENDS[e.backend ?? ""] ?? e.label} · {sizeMb(e.size_bytes)}</span>
            </label>
          {/each}
        </fieldset>
      {/if}
      {#if engine}
        <ul class="items"><DownloadRow item={engine} onretry={() => engine && install([engine])} /></ul>
      {/if}
      <button type="button" class="link" onclick={() => (showReleases = !showReleases)}>
        {showReleases ? "Hide other runtime versions" : "Other runtime versions…"}
      </button>
      {#if showReleases}
        <RuntimeReleases backend={status.recommended_backend} oninstalled={refresh} />
      {/if}
    </section>

    <section class="step">
      <div class="step-head">
        <span class="num" class:done={modelsMissing.length === 0}>2</span>
        <div class="step-text">
          <h2>Speech and speaker models</h2>
          <label class="lang-step">
            <span>Meetings are mostly in</span>
            <select
              bind:value={meetingLanguage}
              onchange={() => void app.setSetting("default_language", meetingLanguage)}
            >
              {#each offeredLanguages(languageTable) as l (l.code)}<option value={l.code}>{l.name}</option>{/each}
            </select>
          </label>
          <p class="help">
            Who spoke when (up to 8 speakers), and speech to text: the EN model for English, Nemotron 3.5 (40+
            languages) for any other.
          </p>
        </div>
        {#if modelsMissing.length > 0}
          <button
            type="button"
            class="btn btn-primary"
            onclick={() => install(engine && !engineInstalled ? [engine, ...models] : models)}
            disabled={downloads.busy}
            title={engine && !engineInstalled ? "Installs the engine first: it fetches the models" : ""}
          >
            <Icon name="download" /> Install recommended models ({sizeMb(modelsBytes)}: {extras.length > 0
              ? `diarizer, ${english ? "the EN model" : "Nemotron 3.5"} and ${extras.join(", ")}`
              : `diarizer and ${english ? "the EN model" : "Nemotron 3.5"}`})
          </button>
        {/if}
      </div>
      <ul class="items">
        {#each models as m (m.name)}
          <DownloadRow item={m} onretry={() => install(engine && !engineInstalled ? [engine, m] : [m])} />
        {/each}
      </ul>
    </section>

    {#if !status.needs_setup}
      <section class="ready">
        <p class="ok-text">
          <strong>✓ Ready.</strong> Import a recording with <kbd>Ctrl O</kbd> or drop files anywhere; press <kbd>R</kbd> to record
          a meeting.
        </p>
        <button type="button" class="btn btn-primary" onclick={() => (app.route = "library")}>Open the library</button>
      </section>
    {/if}

    <section class="manual">
      <div class="row">
        <button type="button" class="btn" onclick={haveFiles}><Icon name="folder" /> I already have the files</button>
        {#if missingAll.length > 0}
          <button type="button" class="btn" onclick={copyAll}><Icon name="copy" /> Copy all links</button>
        {/if}
        <button type="button" class="btn" onclick={refresh} disabled={checking}>
          <Icon name="refresh" />{checking ? "Checking…" : "Check again"}
        </button>
        {#if status.needs_setup}
          <button type="button" class="link" onclick={later}>Set up later</button>
        {/if}
      </div>
      {#if manual}
        <p class="help">
          Put the runtime zip as it is (or its extracted folder, the one with <code>bin\nemo-speech.exe</code>) into
          <code class="sel">{status.engine_dir}</code>, and the .gguf model files anywhere under
          <code class="sel">{status.models_dir}</code> (subfolders are fine). Then press Check again: zips are checked and
          extracted, and each model's header and SHA-256 tell the app what it is.
        </p>
      {/if}
      <p class="help">
        A per-app firewall such as simplewall asks about <code>diarist.exe</code> on the first download (it fetches the
        engine and the models); allow it, or fetch the files by hand with Copy link.
      </p>
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
    max-width: 900px;
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
  .lang-step {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    margin: 4px 0;
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
  .packages {
    display: grid;
    gap: 2px;
    margin: 8px 0 0;
    padding: 0;
    border: 0;
  }
  .radio {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .items {
    margin: 10px 0 4px;
    padding: 0;
    list-style: none;
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
  .sel {
    user-select: all;
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
