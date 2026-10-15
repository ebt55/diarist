<script lang="ts">
  import { onMount, tick } from "svelte";
  import { api, errorText } from "../lib/api";
  import { app } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  let path = $state("");
  let text = $state("");
  let error = $state<string | null>(null);
  let stderr = $state<string | null>(null);
  let auto = $state(true);
  let pre: HTMLPreElement | undefined = $state();

  onMount(() => {
    void refresh();
    void api.lastEngineStderr().then((s) => (stderr = s)).catch(() => {});
    const t = setInterval(() => auto && void refresh(), 2000);
    const onRefresh = () => void refresh();
    const onCopy = () => void copy();
    window.addEventListener("app:refresh", onRefresh);
    window.addEventListener("app:copy", onCopy);
    return () => {
      clearInterval(t);
      window.removeEventListener("app:refresh", onRefresh);
      window.removeEventListener("app:copy", onCopy);
    };
  });

  async function refresh() {
    try {
      const raw = await api.readLogTail(500);
      const nl = raw.indexOf("\n");
      const next = nl >= 0 ? raw.slice(nl + 1) : "";
      path = nl >= 0 ? raw.slice(0, nl) : raw;
      error = null;
      if (next === text) return;
      const atBottom = !pre || pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 24;
      text = next;
      await tick();
      if (atBottom && pre) pre.scrollTop = pre.scrollHeight;
    } catch (e) {
      error = errorText(e);
    }
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(`${path}\n${text}`);
      app.toast("info", `Copied ${text.split("\n").length} log lines`);
    } catch (e) {
      app.error(`Copy failed: ${errorText(e)}`);
    }
  }
</script>

<div class="page">
  <header class="page-head">
    <h1>Diagnostics</h1>
    <span class="hint">Esc closes · Ctrl+R refreshes · Ctrl+Shift+C copies</span>
  </header>

  <section class="log">
    <div class="bar">
      <h2>App log</h2>
      <code class="path" title={path}>{path}</code>
      <label class="radio"><input type="checkbox" bind:checked={auto} /> Follow</label>
      <button type="button" class="btn" onclick={refresh}><Icon name="refresh" /> Refresh</button>
      <button type="button" class="btn" onclick={copy} id="copy-log"><Icon name="copy" /> Copy</button>
    </div>
    {#if error}
      <p class="help error">{error}</p>
    {/if}
    <!-- Scrollable regions must be reachable by keyboard. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <pre bind:this={pre} tabindex="0" aria-label="App log, last 500 lines">{text}</pre>
  </section>

  <section class="stderr">
    <h2>Last engine stderr</h2>
    {#if stderr}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <pre tabindex="0">{stderr}</pre>
    {:else}
      <p class="placeholder">
        No engine job has run on this data root yet. The stderr of the last transcription job appears here
        verbatim, including the runtime's own JSON error object.
      </p>
    {/if}
  </section>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 12px;
    height: 100%;
    padding: 16px 24px 16px;
  }
  .page-head {
    display: flex;
    gap: 12px;
    align-items: baseline;
  }
  h1 {
    margin: 0;
    font-size: 20px;
  }
  h2 {
    margin: 0;
    font-size: 15px;
    white-space: nowrap;
  }
  .hint {
    color: var(--fg-muted);
    font-size: 12px;
  }
  .log {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }
  .bar {
    display: flex;
    gap: 10px;
    align-items: center;
    margin-bottom: 8px;
  }
  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--fg-muted);
    font-size: 12px;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .radio {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    white-space: nowrap;
  }
  pre {
    flex: 1;
    min-height: 0;
    margin: 0;
    padding: 10px 12px;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-sidebar);
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  pre:focus-visible {
    outline: 2px solid var(--focus);
  }
  .stderr pre {
    max-height: 160px;
  }
  .stderr h2 {
    margin-bottom: 6px;
  }
  .placeholder {
    margin: 0;
    padding: 12px;
    border: 1px dashed var(--border-strong);
    border-radius: var(--radius);
    color: var(--fg-muted);
  }
  .help.error {
    color: var(--danger);
  }
</style>
