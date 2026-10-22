import { describe, expect, test } from "bun:test";
import { RevisionsSeen } from "../src/lib/liveLag";

describe("the live latency statistic", () => {
  test("counts each revision once and skips re-sends", () => {
    const seen = new RevisionsSeen();
    expect(seen.fresh("mic", 0, 0)).toBe(true);
    expect(seen.fresh("mic", 0, 1)).toBe(true);
    // The echo gate sends revision 1 again with other words hidden.
    expect(seen.fresh("mic", 0, 1)).toBe(false);
    expect(seen.fresh("mic", 0, 0)).toBe(false);
    expect(seen.fresh("mic", 0, 2)).toBe(true);
  });

  test("keeps channels and utterances apart, and forgets on clear", () => {
    const seen = new RevisionsSeen();
    expect(seen.fresh("mic", 3, 0)).toBe(true);
    expect(seen.fresh("loopback", 3, 0)).toBe(true);
    expect(seen.fresh("mic", 4, 0)).toBe(true);
    seen.clear();
    expect(seen.fresh("mic", 3, 0)).toBe(true);
  });
});
