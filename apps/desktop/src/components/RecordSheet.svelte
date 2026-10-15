<script lang="ts">
  // The start sheet: title, language, room mode and the two devices.
  // Enter starts, Esc cancels. Device choices are remembered in settings.
  import { tick } from "svelte";
  import { api, errorText } from "../lib/api";
  import { formatDate } from "../lib/format";
  import type { SettingKey } from "../lib/generated/settingDefaults";
  import { LANGUAGES, languageLabel } from "../lib/languages";
  import type { AudioDevice } from "../lib/generated/AudioDevice";
  import { live } from "../lib/live.svelte";
  import { app } from "../lib/state.svelte";

  const MIC_KEY: SettingKey = "record.mic_device";
  const LOOPBACK_KEY: SettingKey = "record.loopback_device";
  const DIARIZE_KEY: SettingKey = "record.diarize_mic";

  let dialog: HTMLDialogElement | undefined = $state();
  let titleEl: HTMLInputElement | undefined = $state();
  let title = $state("");
  let language = $state("en");
  let diarizeMic = $state(false);
  let mic = $state("");
  let loopback = $state("");
  let devices = $state.raw<AudioDevice[]>([]);
  let devicesError = $state<string | null>(null);
  let error = $state<string | null>(null);
  let starting = $state(false);

  const mics = $derived(devices.filter((d) => d.flow === "capture"));
  const outputs = $derived(devices.filter((d) => d.flow === "render"));
  const defaultMic = $derived(mics.find((d) => d.is_default_comms) ?? mics.find((d) => d.is_default));
  const defaultOutput = $derived(outputs.find((d) => d.is_default));

  $effect(() => {
    if (!dialog) return;
    if (live.sheetOpen && !dialog.open) {
      void openSheet();
    } else if (!live.sheetOpen && dialog.open) {
      dialog.close();
    }
  });

  async function openSheet() {
    title = `Meeting ${formatDate(Date.now())}`;
    language = app.setting("default_language");
    diarizeMic = app.settingBool(DIARIZE_KEY);
    error = null;
    starting = false;
    dialog?.showModal();
    await tick();
    titleEl?.focus();
    titleEl?.select();
    // Warm the live engine while the user decides.
    void api.prepareRecording().catch((e: unknown) => void api.log("warn", `prepare_recording: ${errorText(e)}`));
    try {
      devices = await api.listAudioDevices();
      devicesError = null;
    } catch (e) {
      devices = [];
      devicesError = errorText(e);
    }
    const remembered = (key: SettingKey, list: AudioDevice[]) => {
      const id = app.setting(key);
      return list.some((d) => d.id === id) ? id : "";
    };
    mic = remembered(MIC_KEY, mics);
    loopback = remembered(LOOPBACK_KEY, outputs);
  }

  function close() {
    live.sheetOpen = false;
  }

  async function remember(key: SettingKey, value: string) {
    if (app.setting(key) !== value) await app.setSetting(key, value);
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (starting) return;
    starting = true;
    error = null;
    await Promise.all([
      remember(MIC_KEY, mic),
      remember(LOOPBACK_KEY, loopback),
      remember(DIARIZE_KEY, diarizeMic ? "true" : "false"),
    ]);
    const failure = await live.start({
      title: title.trim() || null,
      language,
      diarize_mic: diarizeMic,
      mic: mic || null,
      loopback: loopback || null,
    });
    starting = false;
    if (failure) {
      error = failure;
      return;
    }
    close();
    app.toast("info", "Recording. Ctrl+. or Stop ends it; Ctrl+Alt+R works from any app.");
  }
</script>

<dialog
  bind:this={dialog}
  class="modal sheet"
  aria-labelledby="rec-title"
  oncancel={(e) => {
    e.preventDefault();
    if (!starting) close();
  }}
  onclose={() => {
    if (live.sheetOpen) live.sheetOpen = false;
  }}
>
  <form onsubmit={submit}>
    <h2 id="rec-title"><span class="dot" aria-hidden="true"></span> New recording</h2>

    <label class="field">
      <span class="label">Title</span>
      <input bind:this={titleEl} type="text" bind:value={title} spellcheck="false" autocomplete="off" />
    </label>

    <div class="grid">
      <label class="field">
        <span class="label">Language</span>
        <select bind:value={language}>
          {#each LANGUAGES as [code, name] (code)}<option value={code}>{languageLabel(code, name)}</option>{/each}
        </select>
      </label>
      <label class="check">
        <input type="checkbox" bind:checked={diarizeMic} />
        <span>This mic hears a room with several people <span class="help">(find speakers on the mic too)</span></span>
      </label>
    </div>

    <label class="field">
      <span class="label">Microphone</span>
      <select bind:value={mic}>
        <option value="">Default{defaultMic ? `: ${defaultMic.name}` : ""}</option>
        {#each mics as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
      </select>
    </label>

    <label class="field">
      <span class="label">System audio (what the meeting app plays)</span>
      <select bind:value={loopback}>
        <option value="">Default{defaultOutput ? `: ${defaultOutput.name}` : ""} (follows the app that plays)</option>
        {#each outputs as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
      </select>
      <span class="help">
        Recorded after the output's volume: keep it at a normal level. The default follows whichever output the
        meeting app plays to.
      </span>
    </label>

    {#if devicesError}
      <p class="error" role="alert">Could not list audio devices: {devicesError}</p>
    {/if}
    {#if error}
      <div class="error box" role="alert">
        <strong>Recording did not start.</strong>
        <span>{error}</span>
      </div>
    {/if}

    <div class="buttons">
      <span class="hint"><kbd>Enter</kbd> start · <kbd>Esc</kbd> cancel</span>
      <button type="button" class="btn" onclick={close} disabled={starting}>Cancel</button>
      <button type="submit" class="btn btn-rec" disabled={starting}>
        <span class="dot" aria-hidden="true"></span>{starting ? "Starting…" : "Start recording"}
      </button>
    </div>
  </form>
</dialog>

<style>
  .sheet {
    width: min(560px, calc(100vw - 48px));
    max-width: none;
    padding: 18px 20px 16px;
    border: 1px solid var(--border-strong);
    border-radius: 10px;
    background: var(--bg-raised);
    color: var(--fg);
    box-shadow: var(--shadow);
  }
  .sheet::backdrop {
    background: rgb(0 0 0 / 0.35);
  }
  h2 {
    display: flex;
    gap: 8px;
    align-items: center;
    margin: 0 0 14px;
    font-size: 16px;
  }
  .dot {
    display: inline-block;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--rec);
  }
  .btn-rec .dot {
    background: currentColor;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 0 0 12px;
  }
  .label {
    font-weight: 600;
  }
  .grid {
    display: grid;
    grid-template-columns: minmax(0, 200px) minmax(0, 1fr);
    gap: 12px;
    align-items: end;
  }
  .check {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    margin: 0 0 14px;
  }
  .check input {
    margin-top: 3px;
  }
  .help {
    color: var(--fg-muted);
    font-size: 12px;
  }
  .error {
    margin: 0 0 12px;
    color: var(--danger);
  }
  .error.box {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px 10px;
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    background: var(--danger-bg);
    user-select: text;
  }
  .buttons {
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: flex-end;
    margin-top: 4px;
  }
  .hint {
    flex: 1;
    color: var(--fg-muted);
    font-size: 12px;
  }
</style>
