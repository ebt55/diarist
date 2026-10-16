// `bun test` (apps/desktop): the Stop question is the same on every path.
import { describe, expect, test } from "bun:test";
import { armedNote, STOP_QUESTION, stopDialog, stopPress } from "../src/lib/stopConfirm";

describe("stop confirmation", () => {
  test("the dialog asks the one question with the recorded length", () => {
    expect(stopDialog(45_400)).toEqual({
      title: "Stop recording?",
      message: "45 s recorded. The transcript is finalized from the recording after you stop.",
      confirmLabel: "Stop",
    });
    expect(stopDialog(119_000).message).toStartWith("119 s recorded.");
    expect(stopDialog(150_000).message).toStartWith("3 min recorded.");
    expect(stopDialog(-5).message).toStartWith("0 s recorded.");
  });

  test("a second Ctrl+Alt+R or tray Stop while the question is open answers it", () => {
    expect(stopPress(false)).toBe("ask");
    expect(stopPress(true)).toBe("confirm");
  });

  test("the hidden-window reminder asks the same question", () => {
    expect(armedNote("shortcut")).toStartWith(STOP_QUESTION);
    expect(armedNote("shortcut")).toContain("Ctrl+Alt+R again within 3 s");
    expect(armedNote("tray")).toStartWith(STOP_QUESTION);
    expect(armedNote("tray")).toContain("again within 3 s");
  });
});
