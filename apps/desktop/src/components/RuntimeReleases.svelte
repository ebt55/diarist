<script lang="ts">
  // "Check for newer runtime": the NeMo-Speech.cpp releases on GitHub
  // (asked only when the user presses the button), each Windows package
  // installable into engine\<name>\ and selected afterwards. Versions this
  // build was not tested with carry a warning, nothing more.
  import { api, errorText } from "../lib/api";
  import { downloads, failureHint, sizeMb } from "../lib/downloads.svelte";
  import type { RuntimePackage } from "../lib/generated/RuntimePackage";
  import type { RuntimeRelease } from "../lib/generated/RuntimeRelease";
  import { app } from "../lib/state.svelte";

  interface Props {
    /** Preselected package backend (from the GPU check). */
    backend?: string;
    /** Called after a runtime was installed. */
    oninstalled?: () => void;
  }
  let { backend = "cuda", oninstalled }: Props = $props();

  let releases = $state.raw<RuntimeRelease[] | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);

  async function check() {
    loading = true;
    try {
      releases = await api.runtimeReleases();
      error = null;
    } catch (e) {
      error = errorText(e);
    } finally {
      loading = false;
    }
  }

  async function install(r: RuntimeRelease, pkg: RuntimePackage) {
    if (!r.tested) {
      const ok = await app.confirm({
        title: `Install runtime ${r.version}?`,
        message:
          `meet-assist was tested with 0.2.0. ${r.version}${r.prerelease ? " is a pre-release and" : ""} may change ` +
          "the command line the app relies on. If transcription fails, select the previous runtime in Settings, Models.",
        confirmLabel: "Install",
        danger: false,
      });
      if (!ok) return;
    }
    const err = await downloads.run(pkg.name, pkg.size_bytes, () => api.installRuntime(pkg));
    if (err) {
      app.error(`Runtime ${pkg.name} was not installed: ${err}. ${failureHint("engine", err)}`);
    } else {
      app.toast("success", `${pkg.name} installed and selected`);
      await check();
      oninstalled?.();
    }
  }
</script>

<div class="releases">
  <button type="button" class="btn" onclick={check} disabled={loading}>
    {loading ? "Asking GitHub…" : releases ? "Check again" : "Check for newer runtime"}
  </button>
  <span class="hint">Asks api.github.com for the NeMo-Speech.cpp releases; nothing is sent about you.</span>
  {#if error}
    <p class="err">{error}</p>
  {/if}
  {#if releases}
    {#each releases as r (r.tag)}
      <div class="release">
        <div class="head">
          <strong>{r.version}</strong>
          {#if r.tested}<span class="chip ok">tested</span>{:else}<span class="chip warn">untested version</span>{/if}
          {#if r.prerelease}<span class="chip warn">pre-release</span>{/if}
          <span class="hint">{r.published_at.slice(0, 10)}</span>
        </div>
        <ul>
          {#each r.packages as pkg (pkg.name)}
            {@const p = downloads.of(pkg.name)}
            <li class:pick={pkg.backend === backend}>
              <span class="pkg">{pkg.backend}{pkg.backend === backend ? " (this PC)" : ""}</span>
              <span class="mono">{sizeMb(pkg.size_bytes)}</span>
              {#if pkg.installed || p?.state === "done"}
                <span class="ok-text">✓ Installed</span>
              {:else if p?.state === "running"}
                <span class="mono">{p.total ? Math.floor((p.done / p.total) * 100) : 0}%</span>
              {:else}
                <button type="button" class="link" onclick={() => install(r, pkg)} disabled={downloads.busy}>
                  {p?.state === "failed" ? "Retry" : "Install"}
                </button>
                <button
                  type="button"
                  class="link"
                  onclick={() =>
                    navigator.clipboard
                      .writeText(pkg.url)
                      .then(() => app.toast("info", "Link copied; put the downloaded zip into the engine folder and press Refresh"))}
                  >Copy link</button
                >
              {/if}
            </li>
          {/each}
        </ul>
      </div>
    {:else}
      <p class="hint">No Windows runtime was found in the releases.</p>
    {/each}
  {/if}
</div>

<style>
  .releases {
    margin-top: 8px;
  }
  .hint {
    color: var(--fg-muted);
    font-size: 12px;
  }
  .err {
    color: var(--danger);
  }
  .release {
    margin-top: 8px;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }
  ul {
    margin: 4px 0 0;
    padding: 0;
    list-style: none;
  }
  li {
    display: grid;
    grid-template-columns: 160px 80px auto auto;
    gap: 10px;
    align-items: center;
    padding: 2px 0;
    font-size: 13px;
  }
  li.pick .pkg {
    font-weight: 650;
  }
  .ok {
    background: var(--ok-bg);
    color: var(--ok-fg);
  }
  .ok-text {
    color: var(--ok-fg);
  }
  .warn {
    border-color: var(--warning-border);
    background: var(--warning-bg);
    color: var(--warning-fg);
  }
</style>
