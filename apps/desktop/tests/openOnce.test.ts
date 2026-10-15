// `bun test` (apps/desktop): which meeting a `live:state` update opens.
import { describe, expect, test } from "bun:test";
import type { LivePhase } from "../src/lib/generated/LivePhase";
import { OpenOnce } from "../src/lib/openOnce";

/** The `live:state` sequence of one recording, as `live.rs` emits it, with a repeat while it runs. */
function recording(o: OpenOnce, id: number): (number | null)[] {
  const updates: [LivePhase, number | null][] = [
    ["starting", null],
    ["recording", id],
    ["recording", id],
    ["stopping", id],
    ["idle", null],
  ];
  return updates.map(([phase, m]) => o.update(phase, m));
}

describe("OpenOnce", () => {
  test("opens each recording's meeting once", () => {
    const o = new OpenOnce();
    expect(recording(o, 1)).toEqual([null, 1, null, null, null]);
    expect(recording(o, 2)).toEqual([null, 2, null, null, null]);
    expect(recording(o, 3)).toEqual([null, 3, null, null, null]);
  });

  test("opens a new recording that reuses the previous recording's meeting id", () => {
    // Record meeting 4, delete it, record again: SQLite hands out 4 again.
    const o = new OpenOnce();
    expect(recording(o, 4)).toEqual([null, 4, null, null, null]);
    expect(recording(o, 4)).toEqual([null, 4, null, null, null]);
  });

  test("a reload during a recording opens it from the first status", () => {
    const o = new OpenOnce();
    expect(o.update("recording", 7)).toBe(7);
    expect(o.update("recording", 7)).toBe(null);
  });
});
