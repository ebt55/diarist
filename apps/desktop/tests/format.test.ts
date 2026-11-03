// `bun test` (apps/desktop): download sizes in decimal units, as the README,
// docs/INSTALL.md and docs/04 quote them (the setup screen and Settings use
// formatBytes through sizeMb; up to 0.1.0-beta.5 they showed MiB).
import { describe, expect, test } from "bun:test";
import { formatBytes } from "../src/lib/format";

describe("download sizes", () => {
  test("each manifest download, as the docs give it", () => {
    expect(formatBytes(102_183_465)).toBe("102 MB"); // CUDA runtime 0.2.0
    expect(formatBytes(19_302_464)).toBe("19 MB"); // Vulkan runtime
    expect(formatBytes(5_528_418)).toBe("6 MB"); // CPU runtime
    expect(formatBytes(107_012_128)).toBe("107 MB"); // Nemotron 3 Diarization
    expect(formatBytes(699_872_960)).toBe("700 MB"); // Nemotron EN
    expect(formatBytes(741_548_352)).toBe("742 MB"); // Nemotron 3.5
    expect(formatBytes(713_975_456)).toBe("714 MB"); // Parakeet
    expect(formatBytes(320_391_498)).toBe("320 MB"); // punctuation, all its files
  });

  test("the totals, with two decimals in GB", () => {
    // Diarizer, EN model and punctuation: "Install recommended models".
    expect(formatBytes(107_012_128 + 699_872_960 + 320_391_498)).toBe("1.13 GB");
    // With the CUDA runtime: the README's default download.
    expect(formatBytes(102_183_465 + 107_012_128 + 699_872_960 + 320_391_498)).toBe("1.23 GB");
  });

  test("small files", () => {
    expect(formatBytes(11_358)).toBe("11 kB");
    expect(formatBytes(512)).toBe("512 B");
  });
});
