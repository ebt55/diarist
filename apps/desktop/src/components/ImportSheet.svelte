<script lang="ts">
  // The import sheet: which files, and the meeting language (default: the
  // setting, then the last choice). Enter imports, Esc cancels. The job
  // runs on the model that covers the language; when none is installed the
  // sheet offers Nemotron 3.5 and Import waits for it.
  import { tick } from "svelte";
  import { importSheet } from "../lib/importSheet.svelte";
  import { canonical } from "../lib/languages";
  import { app } from "../lib/state.svelte";
  import LanguageField from "./LanguageField.svelte";

  let dialog: HTMLDialogElement | undefined = $state();
  let language = $state("en");
  let blocked = $state(false);
  let notice = $state<string | null>(null);
  let starting = $state(false);

  const names = $derived(importSheet.paths.map((p) => p.split(/[\\/]/).pop() || p));

  $effect(() => {
    if (!dialog) return;
    if (importSheet.isOpen && !dialog.open) {
      language = canonical(importSheet.last ?? app.setting("default_language"));
      starting = false;
      dialog.showModal();
      void tick().then(() => dialog?.querySelector<HTMLButtonElement>("button[type=submit]")?.focus());
    } else if (!importSheet.isOpen && dialog.open) {
      dialog.close();
    }
  });

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (starting || blocked) return;
    starting = true;
    const paths = importSheet.paths;
    const switched = notice;
    importSheet.last = language;
    importSheet.close();
    await app.importPaths(paths, language);
    starting = false;
    if (switched && !app.readiness) app.toast("info", switched);
  }
</script>

<dialog
  bind:this={dialog}
  class="modal sheet"
  aria-labelledby="import-title"
  onkeydown={(e) => e.stopPropagation()}
  oncancel={(e) => {
    e.preventDefault();
    importSheet.close();
  }}
  onclose={() => {
    if (importSheet.isOpen) importSheet.close();
  }}
>
  <form onsubmit={submit}>
    <h2 id="import-title">Import {names.length === 1 ? "a recording" : `${names.length} recordings`}</h2>
    <ul class="files">
      {#each names.slice(0, 5) as n, i (i)}<li>{n}</li>{/each}
      {#if names.length > 5}<li class="more">and {names.length - 5} more</li>{/if}
    </ul>

    <LanguageField id="import-language" bind:language bind:blocked bind:notice active={importSheet.isOpen} />

    <div class="buttons">
      <span class="hint"><kbd>Enter</kbd> import · <kbd>Esc</kbd> cancel</span>
      <button type="button" class="btn" onclick={() => importSheet.close()}>Cancel</button>
      <button type="submit" class="btn btn-primary" disabled={starting || blocked}>
        {starting ? "Importing…" : "Import"}
      </button>
    </div>
  </form>
</dialog>

<style>
  .sheet {
    width: min(520px, calc(100vw - 48px));
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
    margin: 0 0 10px;
    font-size: 16px;
  }
  .files {
    margin: 0 0 14px;
    padding: 0;
    list-style: none;
    color: var(--fg-muted);
    overflow-wrap: anywhere;
  }
  .more {
    font-style: italic;
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
