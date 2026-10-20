<script lang="ts">
  import { onMount, tick } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { api, errorText, langApi, onEvent } from "../lib/api";
  import { copyLinks, downloads, failureHint, sizeMb } from "../lib/downloads.svelte";
  import DownloadRow from "./DownloadRow.svelte";
  import RuntimeReleases from "./RuntimeReleases.svelte";
  import { formatBytes, formatVram, plural } from "../lib/format";
  import type { AudioDevice } from "../lib/generated/AudioDevice";
  import type { DataRootInfo } from "../lib/generated/DataRootInfo";
  import type { DownloadItem } from "../lib/generated/DownloadItem";
  import type { EngineStatus } from "../lib/generated/EngineStatus";
  import type { MoveProgress } from "../lib/generated/MoveProgress";
  import type { RetentionPlan } from "../lib/generated/RetentionPlan";
  import type { UpdateCheck } from "../lib/generated/UpdateCheck";
  import type { ModelFile } from "../lib/generated/ModelFile";
  import type { ModelRole } from "../lib/generated/ModelRole";
  import type { ModelSource } from "../lib/generated/ModelSource";
  import { ECHO_GATE, SETTING_DEFAULTS, type SettingKey } from "../lib/generated/settingDefaults";
  import { ECHO_MODE_HELP, ECHO_MODES, ECHO_THRESHOLD_HELP, echoMode, echoThreshold } from "../lib/echo";
  import {
    LANGUAGE_TIERS,
    TIER_EXPECTATIONS,
    TIER_TITLES,
    canonical,
    offeredLanguages,
    type LanguageTable,
  } from "../lib/languages";
  import LanguageField from "./LanguageField.svelte";
  import { live } from "../lib/live.svelte";
  import { app, type Theme } from "../lib/state.svelte";
  import { updateBlocker } from "../lib/updateBlocker";
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
  /** Capture switches; defaults come from `diarist_core::settings::DEFAULTS`. */
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

  const pinned = $derived(app.meetings.filter((m) => m.audio_keep));
  // Default language: what the installed speech-to-text models cover.
  let defaultLanguage = $state(canonical(app.setting("default_language")));
  let languageTable = $state.raw<LanguageTable>(LANGUAGE_TIERS);
  /** "Portuguese (Brazil, Portugal)": locale entries grouped under their language. */
  const tierNames = (tier: "ready" | "broad") => {
    const groups = new Map<string, string[]>();
    for (const l of offeredLanguages(languageTable).filter((x) => x.tier === tier)) {
      const [base = l.name, region] = l.name.split(", ");
      const g = groups.get(base) ?? [];
      if (region) g.push(region);
      groups.set(base, g);
    }
    return [...groups].map(([base, regions]) => (regions.length ? `${base} (${regions.join(", ")})` : base)).join(", ");
  };
  const unknown = $derived(status?.models_by_role.unknown ?? []);

  let devices = $state.raw<AudioDevice[]>([]);
  let devicesError = $state<string | null>(null);
  let items = $state.raw<DownloadItem[] | null>(null);
  let root = $state.raw<DataRootInfo | null>(null);
  let moving = $state<MoveProgress | null>(null);
  let reclaim = $state.raw<RetentionPlan | null>(null);
  let freeing = $state(false);
  let autostart = $state<boolean | null>(null);
  let update = $state.raw<UpdateCheck | null>(null);
  let checkingUpdate = $state(false);
  let installingUpdate = $state(false);
  let version = $state("");
  /** Why Install and restart waits (a recording, an import or a post-pass). */
  const updateWait = $derived(updateBlocker(live.active || live.phase === "starting", app.jobs));

  const required = $derived(items?.filter((d) => d.required) ?? []);
  const optional = $derived(items?.filter((d) => !d.required) ?? []);
  const missingRequired = $derived(required.filter((d) => !d.installed));
  const engineItems = $derived(items?.filter((d) => d.kind === "engine") ?? []);
  const missingAll = $derived(
    (items ?? []).filter((d) => !d.installed && (d.kind === "model" || d.required)),
  );

  // Add from URL / file.
  let addUrlText = $state("");
  let addShaText = $state("");
  let addLink = $state(false);
  let urlRows = $state.raw<DownloadItem[]>([]);

  async function addUrl(url: string, sha: string) {
    const u = url.trim();
    if (!u) return;
    let name: string;
    try {
      name = decodeURIComponent(new URL(u).pathname.split("/").pop() || "");
    } catch {
      app.error("That is not a web address; paste the direct link to the .gguf file");
      return;
    }
    if (!name) {
      app.error("The link does not end in a file name; use the direct download link of the .gguf file");
      return;
    }
    const row: DownloadItem = {
      kind: "model",
      name,
      label: name,
      size_bytes: 0,
      installed: false,
      required: false,
      experimental: false,
      url: u,
      sha256: sha.trim(),
      dest: `${status?.data_root ?? ""}\\models\\user`,
      licence: "",
      backend: null,
      tested: true,
    };
    urlRows = [...urlRows.filter((r) => r.name !== name), row];
    const err = await downloads.run(name, null, () => api.addModelUrl(u, sha.trim() || null));
    if (err) {
      app.error(`${name} was not added: ${err}. ${failureHint("engine", err)}`);
    } else {
      app.toast("success", `${name} added; select it below if it is not selected`);
      addUrlText = "";
      addShaText = "";
      await loadStatus();
    }
  }

  async function addFile() {
    let picked: string | null;
    try {
      picked = (await openDialog({
        title: "Add a model file",
        multiple: false,
        directory: false,
        filters: [
          { name: "Models", extensions: ["gguf", "onnx"] },
          { name: "All files", extensions: ["*"] },
        ],
      })) as string | null;
    } catch (e) {
      app.error(`Could not open the file dialog (${errorText(e)}); copy the file into the models folder instead`);
      return;
    }
    if (!picked) return;
    try {
      const dest = await api.addModelFile(picked, addLink);
      app.toast("success", `Added ${dest.split(/[\\/]/).pop()}; select it below if it is not selected`);
      await loadStatus();
    } catch (e) {
      app.error(errorText(e));
    }
  }

  async function copyAll() {
    try {
      const n = await copyLinks(missingAll);
      app.toast("info", `${plural(n, "link")} copied. Put each file into the folder its row shows, then press Refresh.`);
    } catch (e) {
      app.error(`Could not copy the links (${errorText(e)}); copy them from each row`);
    }
  }

  /** Uses an unrecognised file for a role anyway (the engine decides). */
  async function forceRole(role: ModelRole, id: number, name: string) {
    const ok = await app.confirm({
      title: `Use ${name} for ${role === "asr" ? "speech to text" : "speaker diarization"}?`,
      message:
        "Diarist does not recognise this file's architecture. It will try it; if the engine cannot run it, its own " +
        "error message is shown and you can switch back here.",
      confirmLabel: "Use it",
      danger: false,
    });
    if (ok) await select(role, id, name);
  }

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
    langApi
      .languageTable()
      .then((t) => (languageTable = t))
      .catch(() => {});
    void loadDevices();
    void loadStatus();
    void loadItems();
    void loadReclaim();
    api.dataRootInfo().then((r) => (root = r)).catch(() => {});
    getVersion().then((v) => (version = v)).catch(() => {});
    api.getAutostart().then((v) => (autostart = v)).catch(() => (autostart = null));
    const offMove = onEvent<MoveProgress>("data:move_progress", (p) => {
      if (moving) moving = p;
    });
    const onRefresh = () => void rescan();
    window.addEventListener("app:refresh", onRefresh);
    return () => {
      window.removeEventListener("app:refresh", onRefresh);
      void offMove.then((off) => off());
    };
  });

  async function loadItems() {
    try {
      items = await api.recommendedDownloads();
    } catch (e) {
      app.error(`Could not read the download list (${errorText(e)}); press Refresh, and check models.user.json in the data folder if you made one`);
    }
  }

  async function loadReclaim() {
    try {
      reclaim = await api.retentionPreview();
    } catch {
      reclaim = null;
    }
  }

  /** "Move…": pick an empty folder, confirm, copy and verify, restart. */
  async function moveData() {
    if (!root || root.move_blocked) return;
    let picked: string | null;
    try {
      picked = (await openDialog({ title: "Move the data folder to…", directory: true, multiple: false })) as
        | string
        | null;
    } catch (e) {
      app.error(`Could not open the folder dialog (${errorText(e)}); restart Diarist and try again`);
      return;
    }
    if (!picked) return;
    const ok = await app.confirm({
      title: "Move the data folder?",
      message:
        `Everything in ${root.root} (database, recordings, engine and models) is copied to ${picked} and checked. ` +
        "Diarist then restarts from the new folder and deletes the old copy. Choose an empty folder.",
      confirmLabel: "Move and restart",
      danger: false,
    });
    if (!ok) return;
    moving = { done_bytes: 0, total_bytes: 0 };
    try {
      const r = await api.moveDataRoot(picked);
      app.toast("success", `Copied ${plural(Number(r.files), "file")} (${formatBytes(Number(r.bytes))}). Restarting…`);
      setTimeout(() => void api.restartApp(), 900);
    } catch (e) {
      moving = null;
      app.error(`The data folder was not moved (${errorText(e)}). Nothing changed; choose an empty folder with enough free space and try again`);
    }
  }

  async function freeUp() {
    if (!reclaim || reclaim.files === 0) return;
    const ok = await app.confirm({
      title: "Delete old audio now?",
      message:
        `The audio of ${plural(reclaim.meetings.length, "meeting")} older than ${plural(reclaim.days, "day")} ` +
        `(${formatBytes(reclaim.bytes)}) is deleted. Their transcripts stay; they can no longer be played or re-run.`,
      confirmLabel: "Delete audio",
      danger: true,
    });
    if (!ok) return;
    freeing = true;
    try {
      const r = await api.applyRetention();
      app.toast("info", `Freed ${formatBytes(r.bytes)} (${plural(r.files, "file")})`);
      await loadReclaim();
    } catch (e) {
      app.error(`Could not delete old audio (${errorText(e)}); close programs using those files and press Free up now again`);
    } finally {
      freeing = false;
    }
  }

  async function toggleAutostart(e: Event) {
    const el = e.currentTarget as HTMLInputElement;
    try {
      autostart = await api.setAutostart(el.checked);
    } catch (err) {
      el.checked = !el.checked;
      app.error(`Could not change start with Windows (${errorText(err)}); Windows Settings, Apps, Startup has the same switch`);
    }
  }

  async function saveEchoThreshold(e: Event) {
    const el = e.currentTarget as HTMLInputElement;
    const v = echoThreshold(el.value);
    if (v === null) {
      el.value = app.setting("echo.gate_threshold");
      return;
    }
    el.value = v;
    if (await app.setSetting("echo.gate_threshold", v)) {
      app.toast("info", `Echo gate threshold ${v}, from the next recording`);
    }
  }

  // A checklist fix asked for one field (Diagnostics, `settings:<name>`).
  $effect(() => {
    const anchor = app.settingsAnchor;
    if (!anchor) return;
    void tick().then(() => {
      const el = document.getElementById(`set-${anchor}`);
      el?.scrollIntoView({ block: "start" });
      el?.querySelector<HTMLInputElement>("input:checked, input")?.focus({ preventScroll: true });
      app.settingsAnchor = null;
    });
  });

  async function saveIdle(e: Event) {
    const el = e.currentTarget as HTMLInputElement;
    const mins = Math.max(0, Math.min(1440, Math.round(Number(el.value) || 0)));
    el.value = String(mins);
    if (await app.setSetting("engine.idle_minutes", String(mins))) {
      app.toast("info", mins === 0 ? "The engine stays loaded" : `The engine unloads after ${plural(mins, "minute")} without recording`);
    }
  }

  async function checkUpdate() {
    checkingUpdate = true;
    try {
      update = await api.checkForUpdate();
      if (!update.configured) app.toast("info", "This build has no update address; see the project page for new versions.");
      else if (!update.available) app.toast("info", `Diarist ${update.current} is the newest version`);
    } catch (e) {
      app.error(`${errorText(e)}. Check the connection, or get the newest installer from the project page`);
    } finally {
      checkingUpdate = false;
    }
  }

  async function installUpdate() {
    if (updateWait) {
      app.toast("info", updateWait);
      return;
    }
    installingUpdate = true;
    try {
      await api.installUpdate();
    } catch (e) {
      installingUpdate = false;
      // The backend refuses while the job queue is busy (a job this window
      // did not list yet): its message says what runs.
      if (/still running|in the queue|Stop the recording/.test(errorText(e))) app.toast("info", errorText(e));
      else app.error(`${errorText(e)}. Download the installer from the project page and run it; your data is kept`);
    }
  }

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
      app.error(`Rescan failed (${errorText(e)}); check the folder with Open models folder and press Refresh again`);
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
      app.error(`Could not select ${name} (${errorText(e)}); press Refresh and pick it again`);
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
      void loadReclaim();
    }
  }

  async function unpin(id: number) {
    try {
      await api.setAudioKeep(id, false);
      app.patchMeeting(id, { audio_keep: false });
    } catch (e) {
      app.error(`Could not unpin (${errorText(e)}); try again with P in the meeting`);
    }
  }

  /** Downloads `list` (engine first), then refreshes the lists. */
  async function download(list: DownloadItem[], what: string) {
    const engineMissing = items?.find((d) => d.kind === "engine" && !d.installed);
    const todo = list.some((d) => d.kind === "model") && engineMissing ? [engineMissing, ...list] : list;
    if (todo.every((d) => d.installed)) {
      app.toast("info", `${what} already installed.`);
      return;
    }
    const failed = await downloads.install(todo);
    await loadItems();
    await loadStatus();
    if (failed > 0) app.error(`${plural(failed, "download")} failed; see Models in Settings`);
    else app.toast("success", `${what} installed`);
  }

  function downloadRecommended() {
    if (items) void download(required, "Recommended engine and models");
  }

  async function downloadOptional(d: DownloadItem) {
    if (d.experimental) {
      const ok = await app.confirm({
        title: `Download ${d.name}?`,
        message:
          "Experimental: Parakeet punctuates well but silently skipped whole spans of speech in our tests. " +
          "Use it only for short clips you check yourself.",
        confirmLabel: "Download",
        danger: false,
      });
      if (!ok) return;
    }
    await download([d], d.label.split(":")[0] ?? d.name);
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
        <code class="path" title={root?.root ?? status?.data_root}>{root?.root ?? status?.data_root ?? "…"}</code>
        <button type="button" class="btn" onclick={() => api.openDataFolder().catch((e) => app.error(`${errorText(e)}; the folder is ${root?.root ?? status?.data_root}`))}>
          <Icon name="folder" /> Open folder
        </button>
        <button
          type="button"
          class="btn"
          onclick={moveData}
          disabled={!root || root.move_blocked !== null || moving !== null}
          title={root?.move_blocked ?? "Copy everything to another folder or drive"}
        >
          Move…
        </button>
      </div>
      {#if moving}
        {@const pct = moving.total_bytes ? Math.floor((moving.done_bytes / moving.total_bytes) * 100) : 0}
        <p class="help">
          <span class="dl-bar"><span class="dl-fill" style="width: {pct}%"></span></span>
          Copying and checking: {formatBytes(moving.done_bytes)} of {formatBytes(moving.total_bytes)}. Keep Diarist open.
        </p>
      {/if}
      <p class="help">
        Database, recordings, engine and models live here{root?.source === "relocated" ? " (moved from the default)" : ""}.
        {#if root?.move_blocked}{root.move_blocked}{:else}Move copies everything, checks it, restarts, then deletes the old copy.{/if}
      </p>
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
        After this, a meeting's recording (FLAC and WAV files) is deleted; its transcript stays. Re-running the
        post-pass or the speaker detection needs the audio, so it only works while the audio exists. 0 keeps audio
        forever. Pinned meetings keep their audio regardless. Old audio is deleted at launch and once a day.
      </p>
      {#if reclaim && reclaim.files > 0}
        <p class="help row">
          <span>
            {formatBytes(reclaim.bytes)} reclaimable now: audio of {plural(reclaim.meetings.length, "meeting")} older than
            {plural(reclaim.days, "day")}{reclaim.pinned_skipped > 0 ? ` (${reclaim.pinned_skipped} pinned kept)` : ""}.
          </span>
          <button type="button" class="link" onclick={freeUp} disabled={freeing}>{freeing ? "Deleting…" : "Free up now"}</button>
        </p>
      {:else if reclaim && reclaim.days > 0}
        <p class="help">Nothing to reclaim: no unpinned audio is older than {plural(reclaim.days, "day")}.</p>
      {/if}
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
      <LanguageField
        id="language"
        label="Default language"
        bind:language={defaultLanguage}
        active={true}
        onchange={(v) => void app.setSetting("default_language", v)}
      />
      <p class="help">Preselected for new recordings and imports; each sheet can change it.</p>
    </div>
  </section>

  <section>
    <h2>Startup and engine</h2>
    <label class="radio block">
      <input type="checkbox" checked={autostart === true} disabled={autostart === null} onchange={toggleAutostart} />
      <span><strong>Start with Windows</strong>, in the notification area (the window opens from the tray icon or Ctrl+Alt+R)</span>
    </label>
    <label class="radio block">
      <input
        type="checkbox"
        checked={boolSetting("engine.preload")}
        onchange={(e) => app.setSetting("engine.preload", (e.currentTarget as HTMLInputElement).checked ? "true" : "false")}
      />
      <span>
        <strong>Load the live engine at launch</strong>. Off (default): it loads when you open the record sheet or
        press Ctrl+Alt+R, which takes a few seconds and uses about 1 GB of VRAM only while needed.
      </span>
    </label>
    <div class="field">
      <label class="label" for="idle">Unload the live engine after</label>
      <div class="row">
        <input
          id="idle"
          type="number"
          min="0"
          max="1440"
          class="num-input"
          value={app.setting("engine.idle_minutes")}
          onchange={saveIdle}
        />
        <span>minutes without recording (0 keeps it loaded; applies the next time it starts)</span>
      </div>
    </div>
    <label class="radio block">
      <input
        type="checkbox"
        checked={boolSetting("engine.low_vram")}
        onchange={(e) => app.setSetting("engine.low_vram", (e.currentTarget as HTMLInputElement).checked ? "true" : "false")}
      />
      <span>
        <strong>Low-VRAM mode</strong>: imports use Nemotron 3.5 when it is installed (peaks at about 1.8 GB instead of
        2.6 GB for the English model). For cards with 4 to 6 GB.
      </span>
    </label>
    <details class="howto">
      <summary>Out-of-memory errors or a very slow transcription?</summary>
      <p class="help">
        When GPU memory runs out, the NVIDIA driver may quietly move work to system memory, which makes transcription
        many times slower instead of failing. To get a clear error instead: NVIDIA Control Panel → Manage 3D settings →
        Program Settings → Add → choose <code>nemo-speech.exe</code> in the engine folder's <code>bin</code> →
        CUDA - Sysmem Fallback Policy → <strong>Prefer No Sysmem Fallback</strong> → Apply. Then close other GPU-heavy
        apps, or turn on low-VRAM mode.
      </p>
    </details>
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
        {#if devicesError}<span class="error">Could not list devices: {devicesError}. Check the Windows sound settings, then Refresh devices.</span>{/if}
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
      <p class="help">
        Only on issues keeps the live transcript, which uses the lower-latency setting; Always re-transcribes with the
        file settings.
      </p>
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
    <fieldset class="field" id="set-echo">
      <legend class="label">Recording with speakers (echo)</legend>
      <div class="row radios">
        {#each ECHO_MODES as [value, label] (value)}
          <label class="radio">
            <input
              type="radio"
              name="echo_mode"
              {value}
              checked={echoMode(app.setting("echo.mode")) === value}
              onchange={() => app.setSetting("echo.mode", value)}
            />
            {label}{value === "auto" ? " (default)" : ""}
          </label>
        {/each}
      </div>
      <p class="help">{ECHO_MODE_HELP}</p>
      <details class="howto">
        <summary>Advanced: echo gate threshold</summary>
        <div class="row">
          <input
            id="echo-threshold"
            type="number"
            min={ECHO_GATE.min}
            max={ECHO_GATE.max}
            step="0.05"
            class="num-input"
            aria-label="Echo gate threshold"
            value={app.setting("echo.gate_threshold")}
            onchange={saveEchoThreshold}
          />
          <span>{ECHO_GATE.min} to {ECHO_GATE.max}, default {SETTING_DEFAULTS["echo.gate_threshold"]}</span>
        </div>
        <p class="help">{ECHO_THRESHOLD_HELP}</p>
      </details>
    </fieldset>
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
        <button type="button" class="btn" onclick={() => api.openModelsFolder().catch((e) => app.error(`${errorText(e)}; the models folder is under ${status?.data_root}`))}>
          <Icon name="folder" /> Open models folder
        </button>
        <button
          type="button"
          class="btn"
          onclick={downloadRecommended}
          disabled={downloads.busy || missingRequired.length === 0}
          title={missingRequired.length === 0 ? "The engine and the recommended models are installed" : ""}
        >
          <Icon name="download" />{downloads.busy ? "Downloading…" : "Download recommended"}
        </button>
      </div>
    </div>
    {#if items}
      <ul class="dl-list" aria-label="Downloads">
        {#each [...required.filter((d) => d.kind === "model"), ...optional.filter((d) => d.kind === "model")] as d (d.name)}
          <DownloadRow item={d} onretry={() => (d.required ? download([d], d.label) : downloadOptional(d))}>
            {#snippet action()}
              <button type="button" class="link" onclick={() => downloadOptional(d)} disabled={downloads.busy}>Download</button>
            {/snippet}
          </DownloadRow>
        {/each}
        {#each urlRows as d (d.name)}
          <DownloadRow item={d} onretry={() => addUrl(d.url, d.sha256)} onopen={() => api.openModelsFolder().catch(() => {})} tidy={false} />
        {/each}
      </ul>
      <div class="row link-row">
        {#if missingAll.length > 0}
          <button type="button" class="link" onclick={copyAll}>Copy all links ({missingAll.length})</button>
        {/if}
        <button type="button" class="link" onclick={() => (app.route = "setup")}>Open the first-run screen</button>
      </div>
      <p class="help">
        The EN model (default) transcribes English only. Nemotron 3.5 covers the other languages and needs less VRAM;
        once it is installed, a meeting in a language the selected model does not cover runs on it automatically, so
        there is no need to select it. Expected quality on Nemotron 3.5:
      </p>
      <ul class="help tiers">
        <li><strong>{TIER_TITLES.ready}</strong>, {TIER_EXPECTATIONS.ready}: {tierNames("ready")}.</li>
        <li><strong>{TIER_TITLES.broad}</strong>, {TIER_EXPECTATIONS.broad}: {tierNames("broad")}.</li>
      </ul>
      <p class="help">
        A model that lists no languages (Parakeet, most added files) is not checked: it gets every language as
        selected. Power users can list more downloads in <code>models.user.json</code> in the data folder.
      </p>
      <details class="add">
        <summary>Add a model the app does not list</summary>
        <form
          class="add-form"
          onsubmit={(e) => {
            e.preventDefault();
            void addUrl(addUrlText, addShaText);
          }}
        >
          <label class="label" for="add-url">Direct link to a .gguf file</label>
          <input id="add-url" type="url" placeholder="https://huggingface.co/…/resolve/…/model.gguf" bind:value={addUrlText} />
          <label class="label" for="add-sha">SHA-256 (optional; checked when given, recorded either way)</label>
          <input id="add-sha" type="text" spellcheck="false" placeholder="64 hexadecimal characters" bind:value={addShaText} />
          <div class="row">
            <button type="submit" class="btn" disabled={!addUrlText.trim() || downloads.busy}>
              <Icon name="download" /> Add from URL
            </button>
            <button type="button" class="btn" onclick={addFile}><Icon name="folder" /> Add from file…</button>
            <label class="radio"><input type="checkbox" bind:checked={addLink} /> Link instead of copy (same drive)</label>
          </div>
        </form>
        <p class="help">
          Files go to <code>models\user\</code>. The app reads the header to find the role; a file it does not recognise
          is listed under Unrecognised and can still be used, and the engine says so if it cannot run it.
        </p>
      </details>
    {/if}
    {#if statusError}
      <p class="help error">Could not read the engine status: {statusError}. Press Refresh; Diagnostics shows what is wrong.</p>
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
          <legend><span class="group-title">Unrecognised</span> <span class="help inline">The header names no architecture the app knows. You can still use one; the engine says so if it cannot run it. A .meet.json sidecar sets the role for good.</span></legend>
          {#each unknown as f (f.id)}
            {@const inUse = status.selected.asr === f.id ? "asr" : status.selected.diarizer === f.id ? "diarizer" : null}
            <div class="model">
              <span class="warn-mark" aria-hidden="true">!</span>
              <span class="model-main">
                <span class="model-name">{f.display_name}{inUse ? ` (in use for ${inUse === "asr" ? "speech to text" : "speakers"})` : ""}</span>
                <span class="model-path mono">architecture "{f.architecture ?? "unknown"}" · {f.path}</span>
              </span>
              <span class="facts">
                <span class="cell s mono">{formatBytes(f.size_bytes)}</span>
                <button type="button" class="link" onclick={() => forceRole("asr", f.id, f.display_name)} disabled={inUse === "asr"}>Use for speech to text</button>
                <button type="button" class="link" onclick={() => forceRole("diarizer", f.id, f.display_name)} disabled={inUse === "diarizer"}>Use for speakers</button>
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
          <p class="help">No engine yet. Install one of the packages below, or put an extracted runtime (or its zip) into the engine folder and press Refresh.</p>
        {/each}
        <ul class="dl-list" aria-label="Runtime packages">
          {#each engineItems as d (d.name)}
            <DownloadRow item={d} onretry={() => download([d], d.label)}>
              {#snippet action()}
                <button type="button" class="link" onclick={() => download([d], d.label)} disabled={downloads.busy}>
                  Install{d.required ? " (recommended for this PC)" : ""}
                </button>
              {/snippet}
            </DownloadRow>
          {/each}
        </ul>
        <div class="row link-row">
          <button type="button" class="link" onclick={() => api.openEngineFolder().catch((e) => app.error(`${errorText(e)}; the engine folder is under ${status?.data_root}`))}>Open engine folder</button>
        </div>
        <RuntimeReleases backend={engineItems.find((d) => d.required)?.backend ?? "cuda"} oninstalled={() => void Promise.all([loadStatus(), loadItems()])} />
      </fieldset>
    {:else}
      <p class="help">Reading models…</p>
    {/if}
  </section>

  <section>
    <h2>About</h2>
    <p class="help">
      Diarist {version} · MIT licence. The speech engine (Apache-2.0) and the models (OpenMDW-1.1,
      NVIDIA Open Model License, CC-BY-4.0) are downloaded separately under their own licences. Nothing leaves this
      computer except those downloads and an update check you start yourself.
    </p>
    <div class="row">
      <button type="button" class="btn" onclick={checkUpdate} disabled={checkingUpdate || installingUpdate}>
        <Icon name="refresh" />{checkingUpdate ? "Checking…" : "Check for updates"}
      </button>
      {#if update?.available}
        <span>Version {update.available} is available.</span>
        <button
          type="button"
          class="btn btn-primary"
          onclick={installUpdate}
          disabled={installingUpdate || updateWait !== null}
          title={updateWait ?? undefined}
        >
          <Icon name="download" />{installingUpdate ? "Installing…" : "Install and restart"}
        </button>
      {/if}
    </div>
    {#if update?.available && updateWait}<p class="help">{updateWait}</p>{/if}
    {#if update?.notes}<pre class="notes">{update.notes}</pre>{/if}
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
  .help.tiers {
    padding-left: 18px;
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
  .dl-list {
    margin: 6px 0 4px;
    padding: 0;
    list-style: none;
  }
  .link-row {
    gap: 14px;
    margin: 4px 0;
  }
  .add {
    margin-top: 10px;
  }
  .add summary {
    cursor: default;
    font-weight: 600;
  }
  .add-form {
    display: grid;
    gap: 4px;
    max-width: 640px;
    margin-top: 6px;
  }
  .warn-mark {
    display: inline-flex;
    justify-content: center;
    align-items: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--warning-bg);
    color: var(--warning-fg);
    font-weight: 700;
    font-size: 12px;
  }
  .sel {
    user-select: all;
  }
  .howto {
    margin-top: 8px;
  }
  .howto summary {
    cursor: default;
  }
  .notes {
    max-height: 160px;
    overflow: auto;
    white-space: pre-wrap;
    font-size: 12px;
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
