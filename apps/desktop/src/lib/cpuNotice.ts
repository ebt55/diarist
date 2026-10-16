// The once-per-session note that the selected runtime package is the CPU
// one (`app.cpuNotice()`, shown with the import and record toasts). The
// backend picks the device from the same package (`models::device`).
import type { ModelFile } from "./generated/ModelFile";
import type { Settings } from "./generated/Settings";

/** Measured on a Ryzen 9 5900X, 16 Oct 2026 (docs/04 section 4). */
export const CPU_NOTICE =
  "The CPU package is 3 to 6 times real time: an hour of audio takes 10 to 20 minutes. Live recording is not recommended.";

/** The selected runtime (`selected_engine`), if it is present. */
export function selectedEngine(files: ModelFile[], settings: Settings): ModelFile | null {
  const id = Number(settings["selected_engine"] ?? "");
  return files.find((f) => f.id === id && f.role === "engine" && !f.missing) ?? null;
}

const BACKENDS = ["cuda", "vulkan", "cpu"];

/** A release-style folder name's backend (`...-windows-x86_64-vulkan`), else null. */
function backendOfName(name: string): string | null {
  const last = name.trim().split(/[-_]/).pop()?.toLowerCase() ?? "";
  return BACKENDS.includes(last) ? last : null;
}

/**
 * The CPU package, judged as the backend's `models::engine_backend` does: its
 * backend as the scan stores it (`quant`), else its display name, else its
 * folder name.
 */
export function isCpuPackage(engine: ModelFile | null): boolean {
  if (!engine) return false;
  const quant = engine.quant?.trim().toLowerCase() ?? "";
  const folder = engine.path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? "";
  const backend = (BACKENDS.includes(quant) ? quant : null) ?? backendOfName(engine.display_name) ?? backendOfName(folder);
  return backend === "cpu";
}
