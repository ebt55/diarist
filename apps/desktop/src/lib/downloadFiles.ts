// The one line a multi-file download's row (a punctuation model: its
// tokenizer, licences, ONNX Runtime library and model) gives about its files,
// and the label of the disclosure that holds each file's address and hash
// (DownloadRow.svelte). Plain functions, so `bun test` covers the wording
// (tests/downloadFiles.test.ts).
import { formatBytes } from "./format";
import type { DownloadItem } from "./generated/DownloadItem";

/** Missing files are named up to this many; more are only counted. */
export const NAMED_FILES = 3;

/**
 * What the row of a multi-file download says about its files while it is
 * not installed, beside its action: "2 of 8 files missing: LICENSE, NOTICE
 * (13 kB)", sizes in decimal units. Null for a single-file download, and
 * while none of the files is there: the row then says Not installed (or
 * shows its action), as a single-file row does.
 */
export function filesSummary(item: Pick<DownloadItem, "files">): string | null {
  const files = item.files ?? [];
  const missing = files.filter((f) => !f.present);
  if (files.length === 0 || missing.length === files.length) return null;
  if (missing.length === 0) return `All ${files.length} files there; Ctrl+R rescans`;
  const bytes = missing.reduce((n, f) => n + f.size_bytes, 0);
  const names = missing.length <= NAMED_FILES ? `: ${missing.map((f) => f.path).join(", ")}` : "";
  return `${missing.length} of ${files.length} files missing${names} (${formatBytes(bytes)})`;
}

/** The summary of the collapsed list of every file: "Files and addresses (8)". */
export function filesDisclosure(count: number): string {
  return `Files and addresses (${count})`;
}
