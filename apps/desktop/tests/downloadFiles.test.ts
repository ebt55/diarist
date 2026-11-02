// `bun test` (apps/desktop): the one line a multi-file download's row gives
// about its files (DownloadRow.svelte), and its disclosure's label.
import { describe, expect, test } from "bun:test";
import { filesDisclosure, filesSummary } from "../src/lib/downloadFiles";
import type { DownloadFile } from "../src/lib/generated/DownloadFile";

const part = (path: string, size_bytes: number, present: boolean): DownloadFile => ({
  path,
  url: `https://example.invalid/p/${path}`,
  size_bytes,
  sha256: "ab".repeat(32),
  dest: `C:\\d\\${path}`,
  present,
});

/** The shipped punctuation entry's eight files (manifests/models.json), all there unless named. */
const punctuation = (missing: string[]): DownloadFile[] =>
  (
    [
      ["tokenizer.json", 34_363_286],
      ["LICENSE", 11_358],
      ["NOTICE", 1_567],
      ["NOTICE-punctuation.txt", 2_179],
      ["onnxruntime-LICENSE.txt", 1_094],
      ["onnxruntime-ThirdPartyNotices.txt", 344_457],
      ["diarist-onnxruntime.dll", 17_253_176],
      ["onnx/model_quantized.onnx", 268_414_381],
    ] as const
  ).map(([p, n]) => part(p, n, !missing.includes(p)));

describe("a multi-file download's summary", () => {
  test("a 0.1.0-beta.5 model folder: the two licence files of v1.1.1, named, in decimal units", () => {
    expect(filesSummary({ files: punctuation(["LICENSE", "NOTICE"]) })).toBe(
      "2 of 8 files missing: LICENSE, NOTICE (13 kB)",
    );
  });

  test("up to three missing files are named, more are only counted", () => {
    expect(filesSummary({ files: punctuation(["LICENSE", "NOTICE", "diarist-onnxruntime.dll"]) })).toBe(
      "3 of 8 files missing: LICENSE, NOTICE, diarist-onnxruntime.dll (17 MB)",
    );
    expect(filesSummary({ files: punctuation(["tokenizer.json", "LICENSE", "NOTICE", "onnx/model_quantized.onnx"]) })).toBe(
      "4 of 8 files missing (303 MB)",
    );
    expect(filesSummary({ files: punctuation(["onnx/model_quantized.onnx"]) })).toBe(
      "1 of 8 files missing: onnx/model_quantized.onnx (268 MB)",
    );
  });

  test("nothing there: no summary, the row says Not installed or shows its action", () => {
    const all = punctuation([]).map((f) => f.path);
    expect(filesSummary({ files: punctuation(all) })).toBeNull();
  });

  test("every file there but not yet scanned", () => {
    expect(filesSummary({ files: punctuation([]) })).toBe("All 8 files there; Ctrl+R rescans");
  });

  test("a single-file download has no summary", () => {
    expect(filesSummary({ files: [] })).toBeNull();
    expect(filesSummary({})).toBeNull();
  });

  test("the disclosure counts every file", () => {
    expect(filesDisclosure(8)).toBe("Files and addresses (8)");
  });
});
