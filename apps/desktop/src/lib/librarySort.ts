// The meeting list's order (issue 20): by the recording's date (`started_at`,
// an import's file date) or by the date the meeting was added
// (`created_at`), newest or oldest first. Stored in `library.sort`.

/** The values of the `library.sort` setting. */
export type LibrarySort = "recorded_desc" | "recorded_asc" | "added_desc" | "added_asc";

export const DEFAULT_SORT: LibrarySort = "recorded_desc";

/** The sort control's options, in menu order. */
export const SORT_OPTIONS: readonly { value: LibrarySort; label: string }[] = [
  { value: "recorded_desc", label: "Recorded, newest first" },
  { value: "recorded_asc", label: "Recorded, oldest first" },
  { value: "added_desc", label: "Added, newest first" },
  { value: "added_asc", label: "Added, oldest first" },
];

/** A stored value as a sort; anything unknown is the default. */
export function parseSort(value: string | null | undefined): LibrarySort {
  return SORT_OPTIONS.find((o) => o.value === value)?.value ?? DEFAULT_SORT;
}

/** The date a sort orders by. */
export function sortsByAdded(sort: LibrarySort): boolean {
  return sort === "added_desc" || sort === "added_asc";
}

interface Sortable {
  id: number;
  started_at: number;
  created_at: number;
}

/** A sorted copy of `list`; equal dates fall back to the id, in the same direction. */
export function sortMeetings<T extends Sortable>(list: readonly T[], sort: LibrarySort): T[] {
  const key: (m: T) => number = sortsByAdded(sort) ? (m) => m.created_at : (m) => m.started_at;
  const dir = sort.endsWith("_asc") ? 1 : -1;
  return [...list].sort((a, b) => dir * (key(a) - key(b) || a.id - b.id));
}
