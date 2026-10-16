<script lang="ts">
  // The meeting-language control of the start sheet, the import sheet and
  // Settings: the table's supported languages that the installed models
  // declare (lib/languages.ts, with the backend's language table), a typed
  // tag ("Other tag…", with a note on what the tag will give), what the job
  // will run on, and, when nothing installed covers a known language, the
  // Nemotron 3.5 download (the same row as Settings and Setup).
  // `blocked` is set while the job cannot start in this language.
  import { api, errorText, langApi } from "../lib/api";
  import { downloads, sizeMb } from "../lib/downloads.svelte";
  import type { DownloadItem } from "../lib/generated/DownloadItem";
  import type { ModelFile } from "../lib/generated/ModelFile";
  import {
    AUTO_NOTE,
    LANGUAGE_TIERS,
    MULTILINGUAL_MODEL,
    OTHER,
    TYPED,
    canonical,
    isAuto,
    isTag,
    languageOptions,
    planFor,
    type LanguageTable,
  } from "../lib/languages";
  import { app } from "../lib/state.svelte";
  import DownloadRow from "./DownloadRow.svelte";
  import Icon from "./Icon.svelte";

  interface Props {
    id: string;
    language: string;
    /** The field is shown: models and the table are read again each time. */
    active: boolean;
    label?: string;
    /** Set while the job cannot start in this language (nothing installed covers it). */
    blocked?: boolean;
    /** "Transcribing with Nemotron 3.5: ..." when the job will switch models, else null. */
    notice?: string | null;
    /** The language changed (also a typed tag), or a download made it runnable. */
    onchange?: (language: string) => void;
  }
  let {
    id,
    language = $bindable(),
    active,
    label = "Language",
    blocked = $bindable(false),
    notice = $bindable(null),
    onchange,
  }: Props = $props();

  let files = $state.raw<ModelFile[]>([]);
  let selectedId = $state<number | null>(null);
  let table = $state.raw<LanguageTable>(LANGUAGE_TIERS);
  let item = $state.raw<DownloadItem | null>(null);
  let loadError = $state<string | null>(null);
  let typed = $state("");

  const options = $derived(languageOptions(files, selectedId, language, true, table));
  const current = $derived(planFor(files, selectedId, language, table));
  /** What the tag being typed would give ("Not in this model's supported list…", "Not checked…"). */
  const typedNote = $derived(isTag(typed) ? planFor(files, selectedId, canonical(typed, table), table).note : null);

  // Pushed to the parent's bindings as plain values. An object written to
  // a binding is a new value on every run, so an effect that wrote the plan
  // object and read it back looped (effect_update_depth_exceeded at every
  // launch); this one reads only `current` and `language`, and writes
  // values that compare equal when nothing changed.
  $effect(() => {
    const kind = current.kind;
    const note = kind === "switch" ? current.note : null;
    const stop = kind === "install" || language === TYPED;
    blocked = stop;
    notice = note;
  });

  $effect(() => {
    if (active) void load();
  });

  async function load(): Promise<void> {
    try {
      const [f, settings, items] = await Promise.all([
        api.listModelFiles(),
        api.getSettings(),
        api.recommendedDownloads(),
      ]);
      // The shipped table stays when the command fails (an older backend).
      table = await langApi.languageTable().catch(() => LANGUAGE_TIERS);
      files = f;
      const sel = Number(settings["selected_model.asr"] ?? "");
      selectedId = Number.isFinite(sel) && sel > 0 ? sel : null;
      item = items.find((i) => i.name === MULTILINGUAL_MODEL) ?? null;
      loadError = null;
    } catch (e) {
      loadError = errorText(e);
    }
  }

  async function install(): Promise<void> {
    if (!item) return;
    const failed = await downloads.install([item]);
    await load();
    if (failed === 0) {
      app.toast("success", "Nemotron 3.5 is installed; meetings in other languages run on it");
      onchange?.(language);
    }
  }

  function useTyped(): void {
    if (!isTag(typed)) return;
    language = canonical(typed, table);
    typed = "";
    onchange?.(language);
  }
</script>

<div class="lang">
  <label class="field">
    <span class="label">{label}</span>
    <select
      {id}
      bind:value={language}
      onchange={(e) => {
        // The element's value: the binding may update after this handler.
        const v = e.currentTarget.value;
        if (v !== OTHER && v !== TYPED) onchange?.(v);
      }}
    >
      {#each options as o (o.code)}<option value={o.code} disabled={o.disabled}>{o.label}</option>{/each}
    </select>
  </label>
  {#if language === TYPED}
    <div class="typed">
      <input
        type="text"
        bind:value={typed}
        placeholder="A language tag, for example sw-KE"
        spellcheck="false"
        autocomplete="off"
        aria-label="Language tag"
        onkeydown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            useTyped();
          }
        }}
      />
      <button type="button" class="btn" onclick={useTyped} disabled={!isTag(typed)}>Use</button>
    </div>
    <p class="help">
      Any BCP-47 tag. A model may list a tag it was not trained on, which can give unusable output; a tag no installed
      model lists is passed to the selected model as it is, not checked.
    </p>
    {#if typedNote}<p class="help">{typedNote}</p>{/if}
  {/if}
  {#if isAuto(language)}<p class="help">{AUTO_NOTE}</p>{/if}
  {#if current.note && current.kind !== "install"}<p class="help">{current.note}</p>{/if}
  {#if loadError}<p class="help">Could not read the installed models ({loadError}); the job checks again when it starts.</p>{/if}
  {#if current.kind === "install"}
    <div class="needs" role="alert">
      <p>{current.note} Install it, or choose another language.</p>
      {#if item}
        {@const it = item}
        <ul class="rows">
          <DownloadRow item={it} onretry={install}>
            {#snippet action()}
              <button type="button" class="btn btn-primary" onclick={install} disabled={downloads.busy}>
                <Icon name="download" /> Install Nemotron 3.5 ({sizeMb(it.size_bytes)})
              </button>
            {/snippet}
          </DownloadRow>
        </ul>
      {:else}
        <p class="help">Settings, Models lists the download.</p>
      {/if}
    </div>
  {/if}
</div>

<style>
  .lang {
    margin: 0 0 12px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .label {
    font-weight: 600;
  }
  .typed {
    display: flex;
    gap: 8px;
    margin-top: 6px;
  }
  .typed input {
    flex: 1;
  }
  .help {
    margin: 4px 0 0;
    color: var(--fg-muted);
    font-size: 12px;
  }
  .needs {
    margin-top: 8px;
    padding: 8px 10px;
    border: 1px solid var(--warning-border);
    border-radius: var(--radius);
    background: var(--warning-bg);
  }
  .needs p {
    margin: 0 0 6px;
  }
  .rows {
    margin: 0;
    padding: 0;
    list-style: none;
  }
</style>
