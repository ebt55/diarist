<script lang="ts">
  import { onMount, tick, type Component } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { PhysicalSize } from "@tauri-apps/api/dpi";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, errorText } from "./lib/api";
  import { AUDIO_EXTENSIONS, app, isTyping } from "./lib/state.svelte";
  import Library from "./components/Library.svelte";
  import MeetingView from "./components/MeetingView.svelte";
  import Overlays from "./components/Overlays.svelte";

  // Settings and Diagnostics are split into their own chunks.
  let settingsPage = $state.raw<Component | null>(null);
  let diagnosticsPage = $state.raw<Component | null>(null);

  $effect(() => {
    if (app.route === "settings" && !settingsPage) {
      void import("./components/Settings.svelte").then((m) => (settingsPage = m.default));
    } else if (app.route === "diagnostics" && !diagnosticsPage) {
      void import("./components/Diagnostics.svelte").then((m) => (diagnosticsPage = m.default));
    }
  });

  onMount(() => {
    let unlistenDrop: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type === "enter" || p.type === "over") app.dragging = true;
        else if (p.type === "leave") app.dragging = false;
        else if (p.type === "drop") {
          app.dragging = false;
          if (p.paths.length > 0) void app.importPaths(p.paths);
        }
      })
      .then((u) => (unlistenDrop = u))
      .catch(() => {});
    void reveal();
    return () => unlistenDrop?.();
  });

  /**
   * Shows the window once the library has rendered (the window starts hidden
   * so the user never sees an empty frame), then logs startup timing. Waits
   * at most 600 ms for data so a slow store cannot keep the window hidden.
   */
  async function reveal() {
    await Promise.race([app.ready, new Promise((r) => setTimeout(r, 600))]);
    await fitWindow();
    await tick();
    const tShow = performance.now();
    try {
      await getCurrentWindow().show();
    } catch (e) {
      console.error("window.show failed", e);
    }
    document.getElementById("meeting-list")?.focus();
    // First frame after show; rAF never fires while the window is hidden.
    await new Promise<void>((resolve) => {
      requestAnimationFrame(() => setTimeout(resolve, 0));
      setTimeout(resolve, 250);
    });
    const tInteractive = performance.now();
    // WebView2 can reset focus while the window activates; keep the list focused.
    if (document.activeElement === document.body) document.getElementById("meeting-list")?.focus();
    window.addEventListener(
      "focus",
      () => {
        if (document.activeElement === document.body) document.getElementById("meeting-list")?.focus();
      },
      { once: true },
    );
    const boot = (window as unknown as { __boot?: number }).__boot ?? 0;
    void api.log(
      "info",
      `startup: show()->interactive ${(tInteractive - tShow).toFixed(1)} ms; ` +
        `script ${boot.toFixed(0)} ms, show() at ${tShow.toFixed(0)} ms, interactive at ${tInteractive.toFixed(0)} ms ` +
        `after navigation start; ${app.meetings.length} meetings`,
    );
  }

  /**
   * The configured size is in OS logical pixels, but WebView2 also applies the
   * Windows text-size setting, so the page can end up tiny (750 x 430 CSS px
   * on a 4K screen at 200 % with 150 % text). Grow the hidden window to a
   * comfortable share of the screen in CSS pixels; never shrink it.
   */
  async function fitWindow() {
    const wantW = Math.min(1280, Math.round(screen.availWidth * 0.86));
    const wantH = Math.min(820, Math.round(screen.availHeight * 0.86));
    if (innerWidth >= wantW - 16 && innerHeight >= wantH - 16) return;
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(innerWidth, wantW);
    const h = Math.max(innerHeight, wantH);
    try {
      const win = getCurrentWindow();
      await win.setSize(new PhysicalSize(Math.round(w * dpr), Math.round(h * dpr)));
      await win.center();
    } catch (e) {
      void api.log("warn", `window resize failed: ${errorText(e)}`);
    }
  }

  async function importDialog() {
    try {
      const picked = await open({
        title: "Import recordings",
        multiple: true,
        directory: false,
        filters: [
          { name: "Audio and video", extensions: AUDIO_EXTENSIONS },
          { name: "All files", extensions: ["*"] },
        ],
      });
      if (picked && picked.length > 0) await app.importPaths(picked);
    } catch (e) {
      app.error(`Could not open the file dialog: ${errorText(e)}`);
    }
  }

  function focusSearch() {
    app.route = "library";
    void tick().then(() => {
      const el = document.getElementById("search-input") as HTMLInputElement | null;
      el?.focus();
      el?.select();
    });
  }

  function onkeydown(e: KeyboardEvent) {
    if (app.confirmRequest) return;
    const k = e.key;
    const ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && !e.altKey) {
      const lower = k.toLowerCase();
      if (!e.shiftKey && (lower === "k" || lower === "f")) focusSearch();
      else if (!e.shiftKey && lower === "o") void importDialog();
      else if (!e.shiftKey && k === ",") app.route = app.route === "settings" ? "library" : "settings";
      else if (e.shiftKey && lower === "d") app.route = app.route === "diagnostics" ? "library" : "diagnostics";
      else if (e.shiftKey && lower === "c" && app.route === "diagnostics") window.dispatchEvent(new Event("app:copy"));
      else if (!e.shiftKey && k === "1") {
        app.route = "library";
        void tick().then(() =>
          (document.getElementById("meeting-list") ?? document.getElementById("search-results"))?.focus(),
        );
      } else if (lower === "r") refreshView();
      else if (lower === "p" || lower === "u" || lower === "s" || lower === "g") {
        // Keep WebView2's print, view-source, save and find bars out of the app.
      } else return;
      e.preventDefault();
      return;
    }
    if (k === "F5") {
      refreshView();
      e.preventDefault();
      return;
    }
    if (isTyping(e) || e.altKey) return;
    if (k === "F1" || k === "?") {
      app.helpOpen = !app.helpOpen;
      e.preventDefault();
    } else if (k === "Escape" && !e.defaultPrevented && app.route !== "library") {
      app.route = "library";
      e.preventDefault();
    }
  }

  function refreshView() {
    if (app.route === "library") {
      void app.refreshMeetings().then(() => app.toast("info", "Library refreshed"));
    } else {
      window.dispatchEvent(new Event("app:refresh"));
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="shell">
  <Library onimport={importDialog} />
  <main class="main">
    {#if app.openId !== null}
      {#key app.openId}
        <div class="pane" class:hidden={app.route !== "library"}>
          <MeetingView meetingId={app.openId} active={app.route === "library"} />
        </div>
      {/key}
    {:else if app.route === "library"}
      <div class="welcome">
        <h1>Your meetings, searchable</h1>
        <p>
          {app.meetings.length.toLocaleString()} meetings on this computer. Everything runs locally; nothing leaves
          the machine.
        </p>
        <ul class="keys">
          <li><kbd>↑</kbd> <kbd>↓</kbd> <kbd>Enter</kbd> open a meeting</li>
          <li><kbd>Ctrl K</kbd> search every transcript</li>
          <li><kbd>Ctrl O</kbd> import a recording, or drop files anywhere</li>
          <li><kbd>?</kbd> all keyboard shortcuts</li>
        </ul>
      </div>
    {/if}
    {#if app.route === "settings"}
      <div class="pane">
        {#if settingsPage}{@const Page = settingsPage}<Page />{/if}
      </div>
    {:else if app.route === "diagnostics"}
      <div class="pane">
        {#if diagnosticsPage}{@const Page = diagnosticsPage}<Page />{/if}
      </div>
    {/if}
  </main>
</div>

<Overlays />

<style>
  .shell {
    display: grid;
    grid-template-columns: var(--sidebar-width) minmax(0, 1fr);
    height: 100%;
  }
  .main {
    position: relative;
    min-width: 0;
    min-height: 0;
  }
  .pane {
    position: absolute;
    inset: 0;
    background: var(--bg);
  }
  .pane.hidden {
    visibility: hidden;
    pointer-events: none;
  }
  .welcome {
    max-width: 520px;
    margin: 12vh auto 0;
    padding: 0 24px;
  }
  .welcome h1 {
    margin: 0 0 8px;
    font-size: 22px;
    letter-spacing: -0.01em;
  }
  .welcome p {
    margin: 0 0 16px;
    color: var(--fg-muted);
  }
  .keys {
    display: grid;
    gap: 8px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
</style>
