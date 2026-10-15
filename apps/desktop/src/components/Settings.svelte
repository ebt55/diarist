<script lang="ts">
  import { onMount } from "svelte";
  import { api, errorText, onEvent } from "../lib/api";
  import { formatBytes, formatVram, plural } from "../lib/format";
  import type { AudioDevice } from "../lib/generated/AudioDevice";
  import type { DownloadItem } from "../lib/generated/DownloadItem";
  import type { DownloadProgress } from "../lib/generated/DownloadProgress";
  import type { EngineStatus } from "../lib/generated/EngineStatus";
  import type { ModelFile } from "../lib/generated/ModelFile";
  import type { ModelRole } from "../lib/generated/ModelRole";
  import type { ModelSource } from "../lib/generated/ModelSource";
  import { SETTING_DEFAULTS, type SettingKey } from "../lib/generated/settingDefaults";
  import { LANGUAGES, languageLabel } from "../lib/languages";
  import { app, type Theme } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  let status = $state.raw<EngineStatus | null>(null);
  let statusError = $state<string | null>(null);
  let rescanning = $state(false);

  const ROLES: { role: ModelRole; label: string; help: string }[] = [
    { role: "asr", label: "Speech to text", help: "One model serves live and file mode." },
    { role: "diarizer", label: "Speaker diarization", help: "Who spoke when, up to 8 speakers per recording." },
    { role: "embedder", label: "Speaker identity", help: "Cross-meeting voiceprints (later)." },
    { role: "punctuation", label: "Punctuation", help: "Punctuation restoration (later)." },
    { role: "llm", label: "Language model", help: "Notes and summaries (optional, later)." },
  ];
  const SOURCES: Record<ModelSource, string> = {
    manifest: "recommended",
    pull: "pulled",
    user: "added by you",
  };
  /** Capture switches; defaults come from `meet_core::settings::DEFAULTS`. */
  const CAPTURE_TOGGLES: { key: SettingKey; label: string; help: string }[] = [
    {
      key: "capture.loopback_agc",
      label: "Automatic gain on system audio",
      help: "Raises quiet system audio (an output at low Windows volume) before it is recorded and transcribed.",
    },
    {
      key: "capture.mic_agc",
      label: "Automatic gain on the microphone",
      help: "Usually not needed: Windows and most headsets already level the mic.",
    },
    {
      key: "capture.autoconvert",
      label: "Let Windows convert to 16 kHz",
      help: "Turn off only if recordings sound wrong on this machine; the app then resamples itself.",
    },
  ];

  type DownloadRow = DownloadProgress & { state: "waiting" | "running" | "done" | "failed" };

  const pinned = $derived(app.meetings.filter((m) => m.audio_keep));
  const unknown = $derived(status?.models_by_role.unknown ?? []);

  let devices = $state.raw<AudioDevice[]>([]);
  let devicesError = $state<string | null>(null);
  let downloads = $state.raw<DownloadItem[] | null>(null);
  let progress = $state.raw<Record<string, DownloadRow>>({});
  let downloading = $state(false);

  const mics = $derived(devices.filter((d) => d.flow === "capture"));
  const outputs = $derived(devices.filter((d) => d.flow === "render"));

  const boolSetting = (key: SettingKey): boolean => app.settingBool(key);

  async function loadDevices() {
    try {
      devices = await api.listAudioDevices();
      devicesError = null;
    } catch (e) {
      devicesError = errorText(e);
    }
  }

  onMount(() => {
    void loadDevices();
    const offProgress = onEvent<DownloadProgress>("models:progress", (p) => {
      if (!progress[p.name]) return;
      progress = { ...progress, [p.name]: { ...p, state: p.finished ? (p.error ? "failed" : "done") : "running" } };
    });
    void loadStatus();
    const onRefresh = () => void rescan();
    window.addEventListener("app:refresh", onRefresh);
    return () => {
      window.removeEventListener("app:refresh", onRefresh);
      void offProgress.then((off) => off());
    };
  });

  async function loadStatus() {
    try {
      status = await api.engineStatus();
      statusError = null;
    } catch (e) {
      statusError = errorText(e);
    }
  }

  async function rescan() {
    rescanning = true;
    try {
      const files = await api.rescanModels();
      await loadStatus();
      app.toast("info", `Models folder scanned: ${plural(files.length, "file")}`);
    } catch (e) {
      app.error(`Rescan failed: ${errorText(e)}`);
    } finally {
      rescanning = false;
    }
  }

  function selectedFor(role: ModelRole): number | null {
    if (!status) return null;
    if (role === "asr") return status.selected.asr;
    if (role === "diarizer") return status.selected.diarizer;
    if (role === "engine") return status.selected.engine;
    return null;
  }

  function selectable(role: ModelRole): boolean {
    return role === "asr" || role === "diarizer" || role === "engine";
  }

  async function select(role: ModelRole, id: number, name: string) {
    try {
      await api.selectModel(role, id);
      await loadStatus();
      app.toast("info", `${name} selected`);
    } catch (e) {
      app.error(`Could not select ${name}: ${errorText(e)}`);
    }
  }

  function vram(f: ModelFile): { mb: number; measured: boolean } | null {
    const c = f.capabilities;
    if (c.vram_mb_measured !== null) return { mb: c.vram_mb_measured, measured: true };
    if (c.vram_mb_estimate !== null) return { mb: c.vram_mb_estimate, measured: false };
    return null;
  }

  function fit(f: ModelFile): { cls: string; mark: string; text: string } {
    const v = vram(f);
    const free = status?.free_vram_mb ?? null;
    if (!v || free === null) return { cls: "unknown", mark: "?", text: "VRAM unknown" };
    if (v.mb <= free * 0.85) return { cls: "fits", mark: "✓", text: `fits in ${formatVram(free)} free` };
    if (v.mb <= free) return { cls: "tight", mark: "!", text: `tight: ${formatVram(free)} free` };
    return { cls: "no", mark: "✕", text: `needs more than the ${formatVram(free)} free` };
  }

  async function saveRetention(e: Event) {
    const el = e.currentTarget as HTMLInputElement;
    const days = Math.max(0, Math.min(3650, Math.round(Number(el.value) || 0)));
    el.value = String(days);
    if (await app.setSetting("audio_retention_days", String(days))) {
      app.toast("info", days === 0 ? "Audio is kept forever" : `Audio is kept for ${plural(days, "day")}`);
    }
  }

  async function unpin(id: number) {
    try {
      await api.setAudioKeep(id, false);
      app.patchMeeting(id, { audio_keep: false });
    } catch (e) {
      app.error(`Could not unpin: ${errorText(e)}`);
    }
  }

  function setRow(name: string, patch: Partial<DownloadRow>) {
    const cur = progress[name];
    if (cur) progress = { ...progress, [name]: { ...cur, ...patch } };
  }

  /** Installs the default engine, then pulls the required models, one at a time. */
  async function downloadRecommended() {
    try {
      downloads = await api.recommendedDownloads();
    } catch (e) {
      app.error(`Could not read the recommended list: ${errorText(e)}`);
      return;
    }
    const missing = downloads.filter((d) => !d.installed);
    if (missing.length === 0) {
      app.toast("info", "Everything recommended is already installed.");
      return;
    }
    downloading = true;
    progress = Object.fromEntries(
      missing.map((d): [string, DownloadRow] => [
        d.name,
        { kind: d.kind, name: d.name, done_bytes: 0, total_bytes: d.size_bytes, finished: false, error: null, state: "waiting" },
      ]),
    );
    // The engine first: `pull_model` uses the installed runtime's own downloader.
    for (const d of missing) {
      setRow(d.name, { state: "running" });
      try {
        if (d.kind === "engine") await api.installEngine(d.name);
        else await api.pullModel(d.name);
        setRow(d.name, { state: "done", finished: true });
      } catch (e) {
        setRow(d.name, { state: "failed", finished: true, error: errorText(e) });
        if (d.kind === "engine") break;
      }
    }
    downloading = false;
    downloads = await api.recommendedDownloads().catch(() => downloads);
    await loadStatus();
    const failed = Object.values(progress).filter((p) => p.state === "failed").length;
    if (failed > 0) app.error(`${plural(failed, "download")} failed; see the list in Settings`);
    else app.toast("success", "Recommended engine and models installed");
  }
</script>

<div class="page">
  <header class="page-head">
    <h1>Settings</h1>
    <span class="hint">Esc closes · Ctrl+R rescans models</span>
  </header>

  <section>
    <h2>Storage</h2>
    <div class="field">
      <span class="label">Data directory</span>
      <div class="row">
        <code class="path" title={status?.data_root}>{status?.data_root ?? "…"}</code>
        <button type="button" class="btn" onclick={() => api.openDataFolder().catch((e) => app.error(errorText(e)))}>
          <Icon name="folder" /> Open folder
        </button>
      </div>
      <p class="help">Database, recordings, engine and models live here. Transcripts are never deleted by retention.</p>
    </div>
    <div class="field">
      <label class="label" for="retention">Keep audio for</label>
      <div class="row">
        <input
          id="retention"
          type="number"
          min="0"
          max="3650"
          class="num-input"
          value={app.settings.audio_retention_days ?? SETTING_DEFAULTS.audio_retention_days}
          onchange={saveRetention}
        />
        <span>days</span>
      </div>
      <p class="help">
        After this, a meeting's audio is deleted and re-transcription is no longer possible. 0 keeps audio forever.
        Pinned meetings keep their audio regardless.
      </p>
      <details class="pins">
        <summary>{plural(pinned.length, "pinned meeting")}</summary>
        {#if pinned.length === 0}
          <p class="help">Pin a meeting with P in the meeting view.</p>
        {:else}
          <ul>
            {#each pinned as m (m.id)}
              <li>
                <button type="button" class="link" onclick={() => app.open(m.id)}>{m.title || "Untitled meeting"}</button>
                <button type="button" class="link" onclick={() => unpin(m.id)}>Unpin</button>
              </li>
            {/each}
          </ul>
        {/if}
      </details>
    </div>
  </section>

  <section>
    <h2>Appearance and transcription</h2>
    <fieldset class="field">
      <legend class="label">Theme</legend>
      <div class="row radios">
        {#each [["system", "Follow Windows"], ["light", "Light"], ["dark", "Dark"]] as [value, label] (value)}
          <label class="radio">
            <input
              type="radio"
              name="theme"
              {value}
              checked={app.theme === value}
              onchange={() => app.setSetting("theme", value as Theme)}
            />
            {label}
          </label>
        {/each}
      </div>
    </fieldset>
    <div class="field">
      <label class="label" for="language">Default language</label>
      <select
        id="language"
        value={app.settings.default_language ?? SETTING_DEFAULTS.default_language}
        onchange={(e) => app.setSetting("default_language", (e.currentTarget as HTMLSelectElement).value)}
      >
        {#each LANGUAGES as [code, name] (code)}<option value={code}>{languageLabel(code, name)}</option>{/each}
      </select>
      <p class="help">Used for new recordings and imports. Automatic detection loses about a second of the first utterance.</p>
    </div>

  </section>

  <section>
    <h2>Recording</h2>
    <div class="field">
      <label class="label" for="rec-mic">Microphone</label>
      <select
        id="rec-mic"
        value={app.setting("record.mic_device")}
        onchange={(e) => app.setSetting("record.mic_device", (e.currentTarget as HTMLSelectElement).value)}
      >
        <option value="">Windows default communications microphone</option>
        {#each mics as d (d.id)}<option value={d.id}>{d.name}{d.is_default_comms ? " (default)" : ""}</option>{/each}
      </select>
    </div>
    <div class="field">
      <label class="label" for="rec-out">System audio</label>
      <select
        id="rec-out"
        value={app.setting("record.loopback_device")}
        onchange={(e) => app.setSetting("record.loopback_device", (e.currentTarget as HTMLSelectElement).value)}
      >
        <option value="">Default output, following the app that plays</option>
        {#each outputs as d (d.id)}<option value={d.id}>{d.name}{d.is_default ? " (default)" : ""}</option>{/each}
      </select>
      <p class="help">
        Preselected in the start sheet and used by Ctrl+Alt+R. System audio is recorded after the output's volume, so
        keep it at a normal level.
        {#if devicesError}<span class="error">Could not list devices: {devicesError}</span>{/if}
        <button type="button" class="link" onclick={loadDevices}>Refresh devices</button>
      </p>
    </div>
    <label class="radio block">
      <input
        type="checkbox"
        checked={boolSetting("record.diarize_mic")}
        onchange={(e) => app.setSetting("record.diarize_mic", (e.currentTarget as HTMLInputElement).checked ? "true" : "false")}
      />
      <span><strong>The mic hears a room with several people</strong>: find speakers on the mic channel too (the default in the start sheet)</span>
    </label>
    <fieldset class="field">
      <legend class="label">Post-pass after a live recording</legend>
      <label class="radio block">
        <input
          type="radio"
          name="post_pass"
          checked={(app.settings.post_pass_mode ?? SETTING_DEFAULTS.post_pass_mode) === "always"}
          onchange={() => app.setSetting("post_pass_mode", "always")}
        />
        <span><strong>Always</strong> re-run the models over the recorded channels when you stop (default)</span>
      </label>
      <label class="radio block">
        <input
          type="radio"
          name="post_pass"
          checked={app.settings.post_pass_mode === "on_issues"}
          onchange={() => app.setSetting("post_pass_mode", "on_issues")}
        />
        <span><strong>Only on issues</strong>: when the live session reported a degraded feed, engine restarts or gaps</span>
      </label>
    </fieldset>
    <label class="radio block">
      <input
        type="checkbox"
        checked={boolSetting("keep_live_runs")}
        onchange={(e) => app.setSetting("keep_live_runs", (e.currentTarget as HTMLInputElement).checked ? "true" : "false")}
      />
      <span><strong>Keep the live transcript</strong> next to the final one after the post-pass (for comparison)</span>
    </label>
    {#each CAPTURE_TOGGLES as t (t.key)}
      <label class="radio block">
        <input
          type="checkbox"
          checked={boolSetting(t.key)}
          onchange={(e) => app.setSetting(t.key, (e.currentTarget as HTMLInputElement).checked ? "true" : "false")}
        />
        <span><strong>{t.label}</strong>. {t.help}</span>
      </label>
    {/each}
    <p class="help">
      Recordings are kept as lossless FLAC (about 90 MB per hour) for
      {plural(Number(app.settings.audio_retention_days ?? SETTING_DEFAULTS.audio_retention_days), "day")} (Keep audio
      for, above), then deleted unless pinned; transcripts stay. Capture changes apply to the next recording.
    </p>
  </section>

  <section>
    <div class="section-head">
      <h2>Models</h2>
      <div class="row">
        <button type="button" class="btn" onclick={rescan} disabled={rescanning} title="Rescan the models folder (Ctrl+R)">
          <Icon name="refresh" />{rescanning ? "Scanning…" : "Refresh"}
        </button>
        <button type="button" class="btn" onclick={() => api.openModelsFolder().catch((e) => app.error(errorText(e)))}>
          <Icon name="folder" /> Open models folder
        </button>
        <button type="button" class="btn" onclick={downloadRecommended} disabled={downloading}>
          <Icon name="download" />{downloading ? "Downloading…" : "Download recommended"}
        </button>
      </div>
    </div>
    {#if downloads}
      <ul class="downloads" aria-label="Recommended downloads">
        {#each downloads as d (d.name)}
          {@const p = progress[d.name]}
          {@const pct = p && p.total_bytes ? Math.min(100, Math.round((p.done_bytes / p.total_bytes) * 100)) : null}
          <li>
            <span class="dl-name">{d.label}</span>
            <span class="mono dl-size">{formatBytes(d.size_bytes)}</span>
            <span class="dl-state">
              {#if p?.state === "running"}
                {#if d.kind === "engine" && pct !== null}
                  <span class="dl-bar"><span class="dl-fill" style="width: {pct}%"></span></span>
                  <span class="mono">{pct}%</span>
                {:else}
                  Downloading and verifying…
                {/if}
              {:else if p?.state === "waiting"}
                Waiting
              {:else if p?.state === "failed"}
                <span class="error" title={p.error ?? ""}>Failed: {p.error}</span>
              {:else if p?.state === "done" || d.installed}
                <span class="ok-text">Installed</span>
              {:else}
                Not installed
              {/if}
            </span>
          </li>
        {/each}
      </ul>
    {/if}
    {#if statusError}
      <p class="help error">Could not read the engine status: {statusError}</p>
    {:else if status}
      <p class="gpu">
        {#if status.gpu}
          <strong>{status.gpu.name}</strong>
          {#if status.gpu.vram_total_mb !== null}· {formatVram(status.free_vram_mb ?? 0)} free of {formatVram(status.gpu.vram_total_mb)}{/if}
          {#if status.gpu.driver_version}· driver {status.gpu.driver_version}{/if}
        {:else}
          No GPU detected; models run on the CPU package.
        {/if}
      </p>
      <p class="help">
        Drop any GGUF into the models folder (subfolders are fine) and press Refresh. The app reads the file header to
        find its role; pick one file per role.
      </p>

      {#each ROLES as { role, label, help } (role)}
        {@const files = status.models_by_role[role] ?? []}
        {#if files.length > 0 || role === "asr" || role === "diarizer"}
          <fieldset class="group">
            <legend><span class="group-title">{label}</span> <span class="help inline">{help}</span></legend>
            {#if files.length === 0}
              <p class="help">No file for this role yet.</p>
            {/if}
            {#each files as f (f.id)}
              {@const v = vram(f)}
              {@const ft = fit(f)}
              <label class="model" class:missing={f.missing}>
                <input
                  type="radio"
                  name="model-{role}"
                  checked={selectedFor(role) === f.id}
                  disabled={!selectable(role) || f.missing}
                  onchange={() => select(role, f.id, f.display_name)}
                />
                <span class="model-main">
                  <span class="model-name">{f.display_name}</span>
                  <span class="model-path mono">{f.path}</span>
                </span>
                <span class="facts">
                  <span class="cell q mono">{f.quant ?? "–"}</span>
                  <span class="cell s mono">{formatBytes(f.size_bytes)}</span>
                  <span class="cell v" title={v ? (v.measured ? "Measured on the reference GPU" : "Estimated from file size") : ""}>
                    {#if v}<span class="mono">{formatVram(v.mb)}</span> <span class="help inline">{v.measured ? "measured" : "est."}</span>{:else}–{/if}
                  </span>
                  <span class="fit {ft.cls}" title={ft.text}><span aria-hidden="true">{ft.mark}</span><span class="sr-only">{ft.text}</span></span>
                  <span class="chip source-{f.source}">{SOURCES[f.source]}</span>
                  {#if f.missing}<span class="chip warn">missing</span>{/if}
                </span>
              </label>
            {/each}
          </fieldset>
        {/if}
      {/each}

      {#if unknown.length > 0}
        <fieldset class="group unknown">
          <legend><span class="group-title">Unrecognised</span> <span class="help inline">Not a speech architecture the runtime knows; add a .meet.json sidecar to force a role.</span></legend>
          {#each unknown as f (f.id)}
            <div class="model">
              <input type="radio" disabled aria-label="{f.display_name} cannot be selected" />
              <span class="model-main">
                <span class="model-name">{f.display_name}</span>
                <span class="model-path mono">architecture "{f.architecture ?? "unknown"}" · {f.path}</span>
              </span>
              <span class="facts">
                <span class="cell q mono">{f.quant ?? "–"}</span>
                <span class="cell s mono">{formatBytes(f.size_bytes)}</span>
                <span class="cell v">–</span>
                <span class="fit unknown" aria-hidden="true">–</span>
                <span class="chip source-{f.source}">{SOURCES[f.source]}</span>
              </span>
            </div>
          {/each}
        </fieldset>
      {/if}

      <fieldset class="group">
        <legend><span class="group-title">Engine</span> <span class="help inline">Runtime folders under engine\. Drop in a newer extracted zip to try it.</span></legend>
        {#each status.engines as eng (eng.id)}
          <label class="model" class:missing={eng.missing}>
            <input
              type="radio"
              name="engine"
              checked={status.selected.engine === eng.id}
              disabled={eng.missing}
              onchange={() => select("engine", eng.id, eng.display_name)}
            />
            <span class="model-main">
              <span class="model-name">{eng.display_name}</span>
              <span class="model-path mono">version {eng.version || "unknown"} · {eng.path}</span>
            </span>
            <span class="facts">
              {#if eng.tested}
                <span class="chip ok">tested</span>
              {:else}
                <span class="chip warn" title="Works, but no fixtures recorded for this version">untested version</span>
              {/if}
              {#if eng.missing}<span class="chip warn">missing</span>{/if}
            </span>
          </label>
        {:else}
          <p class="help">No engine found. The first-run download installs one.</p>
        {/each}
      </fieldset>
    {:else}
      <p class="help">Reading models…</p>
    {/if}
  </section>
</div>

<style>
  .page {
    height: 100%;
    overflow-x: hidden;
    overflow-y: auto;
    padding: 16px 20px 48px;
    container: settings / inline-size;
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
    margin: 0 0 8px;
    font-size: 20px;
  }
  .hint {
    color: var(--fg-muted);
    font-size: 12px;
  }
  section {
    padding: 12px 0 16px;
    border-bottom: 1px solid var(--border);
  }
  h2 {
    margin: 0 0 10px;
    font-size: 15px;
  }
  .section-head {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 6px;
  }
  .section-head h2 {
    margin: 0;
  }
  .field {
    margin: 0 0 14px;
    padding: 0;
    border: 0;
  }
  .label {
    display: block;
    margin-bottom: 4px;
    padding: 0;
    font-weight: 600;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }
  .radios {
    gap: 16px;
  }
  .radio {
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
  .radio.block {
    display: flex;
    align-items: flex-start;
    margin: 4px 0;
  }
  .radio.block input {
    margin-top: 3px;
  }
  .radio.block + .field {
    margin-top: 12px;
  }
  .path {
    flex: 1;
    min-width: 200px;
    overflow: hidden;
    padding: 5px 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-sidebar);
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .num-input {
    width: 90px;
  }
  .help {
    margin: 4px 0 0;
    color: var(--fg-muted);
    font-size: 12px;
  }
  .help.inline {
    margin: 0;
    font-weight: 400;
  }
  .help.error {
    color: var(--danger);
  }
  .pins {
    margin-top: 6px;
  }
  .pins summary {
    cursor: default;
  }
  .pins ul {
    max-height: 160px;
    margin: 4px 0;
    padding-left: 18px;
    overflow-y: auto;
  }
  .pins li {
    display: flex;
    gap: 10px;
  }
  .gpu {
    margin: 6px 0 2px;
  }
  .group {
    margin: 12px 0 0;
    padding: 4px 0 0;
    border: 0;
  }
  .group legend {
    padding: 0;
    margin-bottom: 4px;
  }
  .group-title {
    font-weight: 650;
  }
  .model {
    display: grid;
    grid-template-columns: 20px minmax(0, 1fr) auto;
    gap: 2px 10px;
    align-items: center;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-top-width: 0;
    background: var(--bg);
  }
  .group legend + .model,
  .group legend + p + .model {
    border-top-width: 1px;
    border-radius: var(--radius) var(--radius) 0 0;
  }
  .model:last-child {
    border-radius: 0 0 var(--radius) var(--radius);
  }
  .model:hover {
    background: var(--bg-hover-soft);
  }
  .model.missing,
  .unknown .model {
    color: var(--fg-faint);
  }
  .unknown .model:hover {
    background: var(--bg);
  }
  .model-main {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .model-name {
    font-weight: 600;
  }
  .model-path {
    overflow: hidden;
    color: var(--fg-muted);
    font-size: 11px;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .unknown .model-path {
    color: var(--fg-faint);
  }
  .facts {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .facts > .chip {
    min-width: 92px;
    text-align: center;
  }
  .cell {
    font-size: 12px;
    white-space: nowrap;
  }
  .cell.q {
    width: 44px;
  }
  .cell.s {
    width: 64px;
    text-align: right;
  }
  .cell.v {
    width: 112px;
  }
  @container settings (max-width: 680px) {
    .facts {
      grid-column: 2 / -1;
      flex-wrap: wrap;
    }
    .cell.s {
      text-align: left;
    }
  }
  .fit {
    display: inline-flex;
    justify-content: center;
    align-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    font-size: 12px;
    font-weight: 700;
  }
  .fit.fits {
    background: var(--ok-bg);
    color: var(--ok-fg);
  }
  .fit.tight {
    background: var(--warning-bg);
    color: var(--warning-fg);
  }
  .fit.no {
    background: var(--danger-bg);
    color: var(--danger);
  }
  .fit.unknown {
    color: var(--fg-faint);
  }
  .source-manifest {
    border-color: var(--accent-border);
    color: var(--accent);
  }
  .ok {
    background: var(--ok-bg);
    color: var(--ok-fg);
  }
  .error {
    color: var(--danger);
  }
  .ok-text {
    color: var(--ok-fg);
  }
  .downloads {
    margin: 6px 0 10px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    list-style: none;
  }
  .downloads li {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 80px minmax(0, 1.2fr);
    gap: 10px;
    align-items: center;
    padding: 6px 10px;
    border-top: 1px solid var(--border);
  }
  .downloads li:first-child {
    border-top: 0;
  }
  .dl-name {
    font-weight: 600;
  }
  .dl-size {
    color: var(--fg-muted);
    font-size: 12px;
    text-align: right;
  }
  .dl-state {
    overflow: hidden;
    font-size: 12px;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .dl-bar {
    display: inline-block;
    width: 120px;
    height: 4px;
    overflow: hidden;
    border-radius: 2px;
    background: var(--bg-hover);
    vertical-align: middle;
  }
  .dl-fill {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .warn {
    border-color: var(--warning-border);
    background: var(--warning-bg);
    color: var(--warning-fg);
  }
</style>
