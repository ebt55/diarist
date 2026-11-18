// The one line a multi-file download's row (a punctuation model: its
// tokenizer, licences, ONNX Runtime library and model) gives about its files,
// and the label of the disclosure that holds each file's address and hash
// (DownloadRow.svelte); what a download still fetches, and the first-run
// screen's "Install recommended models (…)" button (Setup.svelte). Plain
// functions, so `bun test` covers the wording (tests/downloadFiles.test.ts).
import { formatBytes } from "./format";
import type { DownloadItem } from "./generated/DownloadItem";
import { MULTILINGUAL_MODEL } from "./languages";

/** Manifest names of the default English model and the diarizer (`manifests/models.json`). */
export const EN_MODEL = "nemotron-en";
export const DIARIZER_MODEL = "nemotron-3-diarization";

/**
 * Bytes a download of `item` still fetches: none once installed; for a
 * multi-file download only its files that are not there (the download skips
 * a file already there with the right hash); else all of it. Counts as the
 * backend's `disk_needed_bytes` does (crates/core/src/models/setup.rs).
 */
export function missingBytes(item: Pick<DownloadItem, "installed" | "size_bytes" | "files">): number {
  if (item.installed) return 0;
  const files = item.files ?? [];
  if (files.length === 0) return item.size_bytes;
  return files.filter((f) => !f.present).reduce((n, f) => n + f.size_bytes, 0);
}

/**
 * The first-run button for the recommended models: the bytes still to fetch
 * and the models still missing, the diarizer first, then the speech model,
 * then the rest by the start of their label. An upgrader whose punctuation
 * folder lacks two licence files reads "Install recommended models (13 kB:
 * Horizon-Labs punctuation small)", not the size of every model.
 */
export function installModelsText(
  models: Pick<DownloadItem, "name" | "label" | "installed" | "size_bytes" | "files">[],
): string {
  const missing = models.filter((m) => !m.installed);
  const bytes = missing.reduce((n, m) => n + missingBytes(m), 0);
  const rank = (name: string) => (name === DIARIZER_MODEL ? 0 : name === EN_MODEL || name === MULTILINGUAL_MODEL ? 1 : 2);
  const names = [...missing]
    .sort((a, b) => rank(a.name) - rank(b.name))
    .map((m) =>
      m.name === DIARIZER_MODEL
        ? "diarizer"
        : m.name === EN_MODEL
          ? "the EN model"
          : m.name === MULTILINGUAL_MODEL
            ? "Nemotron 3.5"
            : m.label.split(":")[0]?.trim() || m.name,
    );
  const list = names.length <= 1 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
  return `Install recommended models (${formatBytes(bytes)}${list ? `: ${list}` : ""})`;
}

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
