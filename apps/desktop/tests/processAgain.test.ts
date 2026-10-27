// `bun test` (apps/desktop): how a "Process again" job that ended is told
// (plan 16 section 12.5 J): once, in its own words, never "the live
// transcript".
import { describe, expect, test } from "bun:test";
import { againEndText } from "../src/lib/processAgain";

describe("againEndText", () => {
  test("from the FLAC, cancel and failure say the current transcript stays, open meeting or not", () => {
    for (const open of [true, false]) {
      expect(againEndText("cancelled", "Standup", "flac", "", open)).toEqual({
        kind: "info",
        text: 'Process again of "Standup" cancelled; the current transcript stays',
      });
      expect(againEndText("error", "Standup", "flac", "fake crash", open)).toEqual({
        kind: "error",
        text: 'Process again of "Standup" did not finish (fake crash); the current transcript stays',
      });
    }
  });

  test("from the WAVs a failure claims nothing and the open meeting's banner tells it", () => {
    expect(againEndText("error", "Standup", "wavs", "fake crash", false)).toEqual({
      kind: "error",
      text: 'Process again of "Standup" did not finish (fake crash)',
    });
    expect(againEndText("error", "Standup", "wavs", "fake crash", true)).toBeNull();
  });

  test("done is told only for a meeting that is not open", () => {
    expect(againEndText("done", "Standup", "flac", "", false)).toEqual({
      kind: "success",
      text: '"Standup" was processed again',
    });
    expect(againEndText("done", "Standup", "flac", "", true)).toBeNull();
  });

  test("never the live transcript", () => {
    for (const type of ["done", "cancelled", "error"] as const) {
      for (const again of ["flac", "wavs"] as const) {
        for (const open of [true, false]) {
          expect(againEndText(type, "T", again, "m", open)?.text ?? "").not.toContain("live transcript");
        }
      }
    }
  });
});
