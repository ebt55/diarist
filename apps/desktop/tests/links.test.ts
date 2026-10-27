// `bun test` (apps/desktop): addresses in a Diagnostics row and every
// address of a multi-file download (plan 16 section 3.6).
import { describe, expect, test } from "bun:test";
import type { DownloadItem } from "../src/lib/generated/DownloadItem";
import { downloadLinks, linkParts, missingFiles, urlsIn } from "../src/lib/links";

describe("addresses in a sentence", () => {
  test("split around the address, without the sentence's full stop", () => {
    const s =
      "VCRUNTIME140_1.dll is missing: install it from https://aka.ms/vs/17/release/vc_redist.x64.exe, then restart Diarist.";
    expect(linkParts(s)).toEqual([
      { text: "VCRUNTIME140_1.dll is missing: install it from ", url: false },
      { text: "https://aka.ms/vs/17/release/vc_redist.x64.exe", url: true },
      { text: ", then restart Diarist.", url: false },
    ]);
    expect(urlsIn("See https://example.org/a.")).toEqual(["https://example.org/a"]);
    expect(linkParts("No address here.")).toEqual([{ text: "No address here.", url: false }]);
    expect(urlsIn("(http://x.test/y) and https://z.test")).toEqual(["http://x.test/y", "https://z.test"]);
  });
});

describe("download addresses", () => {
  const item = (name: string, files: DownloadItem["files"]): DownloadItem => ({
    kind: "model",
    name,
    label: name,
    size_bytes: 10,
    installed: false,
    required: true,
    experimental: false,
    url: `https://example.invalid/${name}/model`,
    sha256: "",
    dest: "C:\\d",
    licence: "",
    backend: null,
    tested: true,
    files,
  });
  const part = (path: string, present: boolean) => ({
    path,
    url: `https://example.invalid/p/${path}`,
    size_bytes: 1,
    sha256: "",
    dest: `C:\\d\\${path}`,
    present,
  });

  test("each file of a multi-file download, else the one address", () => {
    const multi = item("punct", [part("tokenizer.json", true), part("diarist-onnxruntime.dll", false), part("onnx/model.onnx", false)]);
    expect(downloadLinks([item("asr", []), multi, item("old", undefined)])).toEqual([
      "https://example.invalid/asr/model",
      "https://example.invalid/p/tokenizer.json",
      "https://example.invalid/p/diarist-onnxruntime.dll",
      "https://example.invalid/p/onnx/model.onnx",
      "https://example.invalid/old/model",
    ]);
    expect(missingFiles(multi)).toBe(2);
    expect(missingFiles(item("asr", undefined))).toBe(0);
  });
});
