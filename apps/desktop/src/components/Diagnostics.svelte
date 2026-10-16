<script lang="ts">
  import { onMount, tick } from "svelte";
  import { api, errorText } from "../lib/api";
  import { crashApi, CRASH_KIND_LABEL, DUMP_NOTE, type CrashReport } from "../lib/crash";
  import { formatBytes, formatClock, formatDate, formatDuration } from "../lib/format";
  import { downloads } from "../lib/downloads.svelte";
  import type { HealthCheck } from "../lib/generated/HealthCheck";
  import type { LiveStatusView } from "../lib/generated/LiveStatusView";
  import type { ServeStatus } from "../lib/generated/ServeStatus";
  import { app } from "../lib/state.svelte";
  import Icon from "./Icon.svelte";

  let path = $state("");
  let text = $state("");
  let error = $state<string | null>(null);
  let stderr = $state<string | null>(null);
  let auto = $state(true);
  let pre: HTMLPreElement | undefined = $state();
  let liveStatus = $state.raw<LiveStatusView | null>(null);
  let liveError = $state<string | null>(null);
  let checks = $state.raw<HealthCheck[] | null>(null);
  let checking = $state(false);
  let fixing = $state<string | null>(null);
  let crashes = $state.raw<CrashReport[] | null>(null);

  async function refreshCrashes() {
    try {
      crashes = await crashApi.list();
    } catch (e) {
      crashes = [];
      void api.log("warn", `crash reports not listed: ${errorText(e)}`);
    }
  }

  async function copyCrash(r: CrashReport) {
    try {
      await navigator.clipboard.writeText(await crashApi.text(r.name));
      app.toast("info", `Copied ${r.name}, with your user folder shown as ~ (nothing was sent)`);
    } catch (e) {
      app.error(`Could not copy the report (${errorText(e)}); Show in folder and attach the .txt file instead`);
    }
  }

  async function revealCrash(name: string) {
    try {
      await crashApi.reveal(name);
    } catch (e) {
      app.error(`${errorText(e)}; use Open log folder above`);
    }
  }

  async function runChecks() {
    checking = true;
    try {
      checks = await api.healthCheck();
    } catch (e) {
      app.error(`The checks could not run (${errorText(e)}); restart Diarist and try again`);
    } finally {
      checking = false;
    }
  }

  /** The fix button of a check row. */
  async function fix(c: HealthCheck) {
    const action = c.fix ?? "";
    fixing = c.id;
    try {
      if (action === "setup") app.route = "setup";
      else if (action === "settings") app.route = "settings";
      else if (action === "open_models") await api.openModelsFolder();
      else if (action === "open_engine") await api.openEngineFolder();
      else if (action === "sound_settings") await api.openSoundSettings();
      else if (action === "mic_privacy") await api.openMicrophonePrivacy();
      else if (action === "start_engine") await api.prepareRecording();
      else if (action === "rescan") await api.rescanModels();
      else if (action.startsWith("download:")) {
        const name = action.slice("download:".length);
        const item = (await api.recommendedDownloads()).find((d) => d.name === name);
        if (!item) throw new Error(`${name} is not in the download list`);
        const failed = await downloads.install([item]);
        if (failed > 0) app.error(`${name} did not download; the Setup screen has Retry and Copy link`);
      }
      if (app.route === "diagnostics") await runChecks();
    } catch (e) {
      app.error(`${c.fix_label ?? "The fix"} did not work: ${errorText(e)}. Copy diagnostics and report it`);
    } finally {
      fixing = null;
    }
  }

  const STATES: Record<ServeStatus["state"], string> = {
    stopped: "stopped",
    starting: "starting (loading models)",
    ready: "running",
    restarting: "restarting",
    failed: "failed",
  };

  async function refreshLive() {
    try {
      liveStatus = await api.recordingStatus();
      liveError = null;
    } catch (e) {
      liveError = errorText(e);
    }
  }

  onMount(() => {
    void refresh();
    void runChecks();
    void refreshLive();
    void refreshCrashes();
    const tl = setInterval(() => void refreshLive(), 2000);
    void api.lastEngineStderr().then((s) => (stderr = s)).catch(() => {});
    const t = setInterval(() => auto && void refresh(), 2000);
    const onRefresh = () => {
      void refresh();
      void refreshCrashes();
    };
    const onCopy = () => void copy();
    window.addEventListener("app:refresh", onRefresh);
    window.addEventListener("app:copy", onCopy);
    return () => {
      clearInterval(t);
      clearInterval(tl);
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

  async function copyDiagnostics() {
    try {
      const text = await api.diagnosticsReport();
      await navigator.clipboard.writeText(text);
      app.toast("info", "Diagnostics copied: versions, GPU, data folder, models and the last engine errors (no transcripts)");
    } catch (e) {
      app.error(`Could not copy diagnostics (${errorText(e)}); select the log below and copy it with Ctrl+C`);
    }
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(`${path}\n${text}`);
      app.toast("info", `Copied ${text.split("\n").length} log lines`);
    } catch (e) {
      app.error(`Copy failed (${errorText(e)}); select the log text and press Ctrl+C`);
    }
  }
</script>

<div class="page">
  <header class="page-head">
    <h1>Diagnostics</h1>
    <span class="hint">Esc closes · Ctrl+R refreshes · Ctrl+Shift+C copies</span>
  </header>

  <section class="actions">
    <button type="button" class="btn btn-primary" onclick={copyDiagnostics}><Icon name="copy" /> Copy diagnostics</button>
    <button type="button" class="btn" onclick={() => api.openLogFolder().catch((e) => app.error(`${errorText(e)}; the log file path is shown above the log`))}>
      <Icon name="folder" /> Open log folder
    </button>
    <span class="hint">For a bug report: versions, GPU and driver, data folder, selected models with hashes and the last engine errors. No transcripts.</span>
  </section>

  <section class="checklist">
    <div class="bar">
      <h2>Why isn't it working?</h2>
      <button type="button" class="btn" onclick={runChecks} disabled={checking}><Icon name="refresh" />{checking ? "Checking…" : "Check again"}</button>
    </div>
    {#if checks}
      <ul>
        {#each checks as c (c.id)}
          <li class="check st-{c.status}">
            <span class="mark" aria-hidden="true">{c.status === "pass" ? "✓" : c.status === "fail" ? "✕" : c.status === "warn" ? "!" : "–"}</span>
            <span class="what"><strong>{c.label}</strong> <span class="sr-only">{c.status}</span><span class="detail">{c.detail}</span></span>
            {#if c.fix && c.status !== "pass"}
              <button type="button" class="btn small" onclick={() => fix(c)} disabled={fixing !== null}>{fixing === c.id ? "…" : c.fix_label}</button>
            {/if}
          </li>
        {/each}
      </ul>
    {:else}
      <p class="placeholder">{checking ? "Checking the engine, GPU, models, disk and audio devices…" : "Press Check again."}</p>
    {/if}
  </section>

  {#if app.startup && (app.startup.recovered.length > 0 || app.startup.moved_from || app.startup.recovery_error)}
    {@const st = app.startup}
    <section class="recovered">
      <h2>At launch</h2>
      <ul>
        {#each st.recovered as r (r.meeting_id)}
          <li>
            <button type="button" class="link" onclick={() => app.open(r.meeting_id)}>{r.title || "Untitled meeting"}</button>
            (interrupted {r.was}): {r.action}
          </li>
        {/each}
        {#if st.recovery_error}<li class="error">Crash recovery could not run: {st.recovery_error}</li>{/if}
        {#if st.moved_from}
          <li>
            The data folder was moved; the old copy in <code>{st.moved_from}</code>
            {st.move_kept.length === 0 ? "was removed." : `was partly kept: ${st.move_kept.join("; ")}`}
          </li>
        {/if}
      </ul>
    </section>
  {/if}

  <section class="crashes">
    <h2>Crash reports</h2>
    {#if crashes && crashes.length > 0}
      <ul>
        {#each crashes as r (r.name)}
          <li class="crash">
            <span class="what">
              <strong>{CRASH_KIND_LABEL[r.kind]}</strong> · {formatDate(r.at_ms)}
              <span class="detail mono small">{r.name} ({formatBytes(r.bytes)}){r.dump ? ` + ${r.dump}` : ""}</span>
            </span>
            {#if r.name.endsWith(".txt")}
              <button type="button" class="btn small" onclick={() => copyCrash(r)}><Icon name="copy" /> Copy</button>
            {/if}
            <button type="button" class="btn small" onclick={() => revealCrash(r.dump ?? r.name)}><Icon name="folder" /> Show in folder</button>
          </li>
        {/each}
      </ul>
      {#if crashes.some((r) => r.dump)}<p class="hint">{DUMP_NOTE}</p>{/if}
    {:else}
      <p class="placeholder">
        {crashes ? "No crash reports." : "Reading…"} If Diarist or the engine crashes, a report is written to the log folder
        (the newest five of each kind are kept). Nothing is sent anywhere; Copy shows your user folder as ~.
      </p>
    {/if}
  </section>

  <section class="engine">
    <h2>Live engine</h2>
    {#if liveError}
      <p class="help error">{liveError}</p>
    {:else if liveStatus}
      {@const srv = liveStatus.server}
      {@const rec = liveStatus.recorder}
      <dl class="facts">
        <dt>Server</dt>
        <dd>
          {#if srv}
            <span class="chip st-{srv.status.state}">{STATES[srv.status.state]}</span>
            {#if "port" in srv.status}port <span class="mono">{srv.status.port}</span>{/if}
            {#if srv.pid !== null}· pid <span class="mono">{srv.pid}</span>{/if}
            {#if srv.uptime_ms !== null}· up {formatDuration(srv.uptime_ms)}{/if}
            · {srv.restarts} {srv.restarts === 1 ? "restart" : "restarts"}
            {#if srv.resident_vram_mb !== null}· holds {srv.resident_vram_mb} MB of VRAM{/if}
            {#if srv.status.state === "restarting"}<br /><span class="error">{srv.status.last_error}</span>{/if}
            {#if srv.status.state === "failed"}<br /><span class="error">{srv.status.message}</span>{/if}
          {:else}
            not running (it loads when the record sheet opens or Ctrl+Alt+R is pressed, and unloads after the idle time in Settings)
          {/if}
        </dd>
        {#if srv}
          <dt>Models</dt>
          <dd class="mono small">{srv.asr_model} · {srv.diar_model}</dd>
          {#if srv.log_path}<dt>Server log</dt><dd class="mono small">{srv.log_path}</dd>{/if}
        {/if}
        <dt>Recording</dt>
        <dd>
          {#if rec}
            meeting {rec.meeting_id} · {formatClock(rec.elapsed_ms)} ·
            {Object.entries(rec.revisions)
              .map(([ch, n]) => `${ch} ${n} revisions`)
              .join(", ") || "no words yet"}
          {:else}
            {liveStatus.state.phase === "idle" ? "not recording" : liveStatus.state.phase}
          {/if}
        </dd>
        {#if rec}
          {#each Object.entries(rec.captures) as [ch, c] (ch)}
            <dt>{ch === "mic" ? "Mic capture" : "Loopback capture"}</dt>
            <dd>
              {c.device_name ?? "?"} · {c.reopens} reopens · {c.overflow_packets} dropped packets
              {#if c.last_error}<br /><span class="error">{c.last_error}</span>{/if}
            </dd>
          {/each}
          <dt>Issues</dt>
          <dd class="mono small">
            degraded {rec.issues.degraded} · engine down {rec.issues.engine_down} · restarts {rec.issues.restarts} ·
            gaps {rec.issues.gaps} ({rec.issues.gap_ms} ms) · device changes {rec.issues.device_changes}
          </dd>
        {/if}
        <dt>GPU queue</dt>
        <dd>
          {liveStatus.queue_running ? "1 job running" : "idle"}{liveStatus.queue_waiting.length > 0
            ? ` · ${liveStatus.queue_waiting.length} waiting${liveStatus.state.queue_paused ? " (paused while recording)" : ""}`
            : ""}
        </dd>
        <dt>Shortcut</dt>
        <dd>
          <kbd>{liveStatus.shortcut}</kbd>
          {#if liveStatus.shortcut_error}<span class="error">not registered: {liveStatus.shortcut_error}</span>{:else}toggles recording from any app{/if}
        </dd>
      </dl>
    {:else}
      <p class="placeholder">Reading…</p>
    {/if}
  </section>

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
    overflow-y: auto;
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
    min-height: 280px;
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
  .help.error,
  .error {
    color: var(--danger);
  }
  .engine h2,
  .recovered h2,
  .crashes h2 {
    margin-bottom: 6px;
  }
  .crashes ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .crash {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    gap: 8px;
    align-items: center;
    padding: 4px 0;
    border-top: 1px solid var(--border);
    font-size: 13px;
  }
  .crash .detail {
    display: block;
    color: var(--fg-muted);
    overflow-wrap: anywhere;
  }
  .checklist ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .check {
    display: grid;
    grid-template-columns: 20px minmax(0, 1fr) auto;
    gap: 8px;
    align-items: start;
    padding: 4px 0;
    border-top: 1px solid var(--border);
    font-size: 13px;
  }
  .check .mark {
    font-weight: 700;
    text-align: center;
  }
  .st-pass .mark {
    color: var(--ok-fg);
  }
  .st-fail .mark {
    color: var(--danger);
  }
  .st-warn .mark {
    color: var(--warning-fg);
  }
  .st-skip .mark {
    color: var(--fg-faint);
  }
  .check .detail {
    display: block;
    color: var(--fg-muted);
    overflow-wrap: anywhere;
  }
  .btn.small {
    padding: 2px 10px;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }
  .recovered ul {
    margin: 0;
    padding-left: 18px;
    font-size: 13px;
  }
  .facts {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 3px 14px;
    margin: 0;
    font-size: 13px;
  }
  .facts dt {
    color: var(--fg-muted);
  }
  .facts dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .small {
    font-size: 12px;
  }
  .st-ready {
    border-color: var(--ok-fg);
    background: var(--ok-bg);
    color: var(--ok-fg);
  }
  .st-failed,
  .st-restarting {
    border-color: var(--danger);
    color: var(--danger);
  }
  .st-starting {
    border-color: var(--warning-border);
    background: var(--warning-bg);
    color: var(--warning-fg);
  }
</style>
