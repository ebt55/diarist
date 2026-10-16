// Which live line R renames while recording. The selected line, else the
// newest final line of a known speaker: the newest line is usually still
// provisional ("Unknown speaker", a label the engine replaces when the
// utterance ends), and a name given to that row has no words after Stop.
// Free of Svelte and Tauri so `bun test` can run it (tests/renameTarget.test.ts).

/** The engine label of words the live diarizer has not tagged yet. */
export const UNKNOWN_LABEL = "spk_unknown";

/** Said when R finds no line to rename. */
export const NO_LINE_NOTE = "Select a line first (j/k) or rename from the speaker legend";

export interface RenameRow {
  label: string;
  speakerId: number | null;
}

/**
 * The line R renames: the selected one (`cursor`), else the newest line
 * that is final and belongs to a known speaker, else null.
 */
export function renameTarget<R extends RenameRow>(rows: readonly R[], cursor: number, isFinal: (row: R) => boolean): R | null {
  const selected = cursor >= 0 ? rows[cursor] : undefined;
  if (selected) return selected;
  for (let i = rows.length - 1; i >= 0; i--) {
    const row = rows[i]!;
    if (row.speakerId !== null && row.label !== UNKNOWN_LABEL && isFinal(row)) return row;
  }
  return null;
}
