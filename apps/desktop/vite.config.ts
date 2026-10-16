import process from "node:process";
import { fileURLToPath } from "node:url";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig, searchForWorkspaceRoot } from "vite";

// Set by the Tauri CLI for mobile/remote dev; unset on desktop.
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/ and https://tauri.app/start/frontend/vite/
export default defineConfig({
  plugins: [svelte()],
  // Keep Rust compiler errors visible in `tauri dev`.
  clearScreen: false,
  server: {
    // tauri.conf.json `build.devUrl` expects exactly this port.
    port: 1420,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
    // The sidebar's app icon comes from the repository's branding folder.
    fs: { allow: [searchForWorkspaceRoot(process.cwd()), fileURLToPath(new URL("../../branding", import.meta.url))] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    // `tauri build --debug` sets TAURI_ENV_DEBUG: keep debug builds readable.
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
