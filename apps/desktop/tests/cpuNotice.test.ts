import { describe, expect, test } from "bun:test";
import { CPU_NOTICE, isCpuPackage, selectedEngine } from "../src/lib/cpuNotice";
import type { ModelFile } from "../src/lib/generated/ModelFile";

function engine(id: number, folder: string, quant: string | null, extra: Partial<ModelFile> = {}): ModelFile {
  return {
    id,
    path: `engine/${folder}`,
    role: "engine",
    architecture: "0.2.0",
    display_name: folder,
    quant,
    size_bytes: 1,
    sha256: null,
    source: "manifest",
    capabilities: { streaming: false, diarize: false, languages: [], vram_mb_measured: null, vram_mb_estimate: null },
    manifest_name: null,
    first_seen_at: 0,
    last_seen_at: 0,
    missing: false,
    ...extra,
  } as ModelFile;
}

const CUDA = engine(1, "nemo-speech-0.2.0-windows-x86_64-cuda", "cuda");
const VULKAN = engine(2, "nemo-speech-0.2.0-windows-x86_64-vulkan", "vulkan");
const CPU = engine(3, "nemo-speech-0.2.0-windows-x86_64-cpu", "cpu");

describe("the CPU package notice", () => {
  test("only the CPU package gets it", () => {
    expect(isCpuPackage(CPU)).toBe(true);
    expect(isCpuPackage(CUDA)).toBe(false);
    expect(isCpuPackage(VULKAN)).toBe(false);
    expect(isCpuPackage(null)).toBe(false);
    // A row scanned before the backend was stored: the folder name.
    expect(isCpuPackage(engine(4, "nemo-speech-0.3.0-windows-x86_64-CPU", null))).toBe(true);
    expect(isCpuPackage(engine(5, "my-build", null, { path: "D:/rt/my-build-cpu/" }))).toBe(true);
    expect(isCpuPackage(engine(6, "my-build", null))).toBe(false);
    // The stored backend wins over the name.
    expect(isCpuPackage(engine(7, "renamed-cpu", "vulkan"))).toBe(false);
  });

  test("it follows the selected runtime", () => {
    const files = [CUDA, VULKAN, CPU, engine(9, "gone-cpu", "cpu", { missing: true })];
    expect(selectedEngine(files, { selected_engine: "3" })?.id).toBe(3);
    expect(isCpuPackage(selectedEngine(files, { selected_engine: "1" }))).toBe(false);
    expect(selectedEngine(files, { selected_engine: "9" })).toBeNull();
    expect(selectedEngine(files, {})).toBeNull();
    expect(CPU_NOTICE).toContain("3 to 6 times real time");
    expect(CPU_NOTICE).toContain("Live recording is not recommended");
  });
});
