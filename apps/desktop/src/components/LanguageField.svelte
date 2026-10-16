<script lang="ts">
  // The meeting-language control of the start and import sheets: the
  // languages the installed models cover (lib/languages.ts), what the job
  // will run on, and, when nothing installed covers the language, the
  // Nemotron 3.5 download (the same row as Settings and Setup). `blocked`
  // is set while the job cannot start in this language.
  import { api, errorText } from "../lib/api";
  import { downloads, sizeMb } from "../lib/downloads.svelte";
  import type { DownloadItem } from "../lib/generated/DownloadItem";
  import type { ModelFile } from "../lib/generated/ModelFile";
  import {
    AUTO_NOTE,
    MULTILINGUAL_MODEL,
    OTHER,
    isAuto,
    languageOptions,
    planFor,
    type LanguagePlan,
  } from "../lib/languages";
  import { app } from "../lib/state.svelte";
  import DownloadRow from "./DownloadRow.svelte";
  import Icon from "./Icon.svelte";

  interface Props {
    id: string;
    language: string;
    /** The sheet is open: the model list is read again each time it opens. */
    active: boolean;
    /** Set while the job cannot start in this language (nothing installed covers it). */
    blocked?: boolean;
    plan?: LanguagePlan;
    /** The language changed, or a download made it runnable. */
    onchange?: (language: string) => void;
  }
  let {
    id,
    language = $bindable(),
    active,
    blocked = $bindable(false),
    plan = $bindable({ kind: "selected", file: null, note: null }),
    onchange,
  }: Props = $props();

  let files = $state.raw<ModelFile[]>([]);
  let selectedId = $state<number | null>(null);
  let item = $state.raw<DownloadItem | null>(null);
  let loadError = $state<string | null>(null);

  const options = $derived(languageOptions(files, selectedId, language, true));

  $effect(() => {
    plan = planFor(files, selectedId, language);
    blocked = plan.kind === "install";
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
</script>

<div class="lang">
  <label class="field">
    <span class="label">Language</span>
    <select
      {id}
      bind:value={language}
      onchange={() => {
        if (language !== OTHER) onchange?.(language);
      }}
    >
      {#each options as o (o.code)}<option value={o.code} disabled={o.disabled}>{o.label}</option>{/each}
    </select>
  </label>
  {#if isAuto(language)}<p class="help">{AUTO_NOTE}</p>{/if}
  {#if plan.note && plan.kind !== "install"}<p class="help">{plan.note}</p>{/if}
  {#if loadError}<p class="help">Could not read the installed models ({loadError}); the job checks again when it starts.</p>{/if}
  {#if plan.kind === "install"}
    <div class="needs" role="alert">
      <p>{plan.note} Install it, or choose another language.</p>
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
