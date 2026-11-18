// `bun test` (apps/desktop): the meeting list's sort (issue 20).
import { describe, expect, test } from "bun:test";
import { parseSort, sortMeetings, sortsByAdded, SORT_OPTIONS } from "../src/lib/librarySort";

const DAY = 86_400_000;
// An old import: its file is from last year, it was added today.
const oldImport = { id: 3, started_at: 1_000 * DAY, created_at: 1_400 * DAY };
// A recording made yesterday.
const recording = { id: 2, started_at: 1_399 * DAY, created_at: 1_399 * DAY };
// Imported last week, its file from last month.
const lastWeek = { id: 1, started_at: 1_370 * DAY, created_at: 1_393 * DAY };
const list = [lastWeek, oldImport, recording];
const ids = (l: { id: number }[]) => l.map((m) => m.id);

describe("library sort", () => {
  test("recorded sorts by the recording's date", () => {
    expect(ids(sortMeetings(list, "recorded_desc"))).toEqual([2, 1, 3]);
    expect(ids(sortMeetings(list, "recorded_asc"))).toEqual([3, 1, 2]);
  });

  test("added sorts by the date added, so an old import comes first", () => {
    expect(ids(sortMeetings(list, "added_desc"))).toEqual([3, 2, 1]);
    expect(ids(sortMeetings(list, "added_asc"))).toEqual([1, 2, 3]);
  });

  test("equal dates fall back to the id in the same direction", () => {
    const a = { id: 5, started_at: 7, created_at: 7 };
    const b = { id: 9, started_at: 7, created_at: 7 };
    expect(ids(sortMeetings([a, b], "recorded_desc"))).toEqual([9, 5]);
    expect(ids(sortMeetings([b, a], "added_asc"))).toEqual([5, 9]);
  });

  test("the input is not changed", () => {
    const copy = [...list];
    sortMeetings(list, "added_asc");
    expect(list).toEqual(copy);
  });

  test("unknown stored values take the default", () => {
    expect(parseSort(undefined)).toBe("recorded_desc");
    expect(parseSort("")).toBe("recorded_desc");
    expect(parseSort("newest")).toBe("recorded_desc");
    expect(parseSort("added_asc")).toBe("added_asc");
  });

  test("every option parses to itself", () => {
    for (const o of SORT_OPTIONS) expect(parseSort(o.value)).toBe(o.value);
    expect(SORT_OPTIONS.filter((o) => sortsByAdded(o.value)).length).toBe(2);
  });
});
