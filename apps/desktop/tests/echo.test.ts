// `bun test` (apps/desktop): the echo wording of the meeting details, the
// banner and Settings (plan 15).
import { describe, expect, test } from "bun:test";
import {
  ECHO_MODE_HELP,
  ECHO_MODES,
  ECHO_OFF_ACTION,
  ECHO_OFF_KEY,
  ECHO_OFF_NOTE,
  ECHO_THRESHOLD_HELP,
  echoInfo,
  echoLine,
  echoMode,
  echoThreshold,
  echoWarningText,
} from "../src/lib/echo";

const run = (echo: unknown) => ({ runtime: "nemo-speech 0.2.0", echo });

describe("meeting details echo line", () => {
  test("removed words: the counts and Restore", () => {
    const l = echoLine(echoInfo(run({ coupled: true, coupling: 0.7213, lag_ms: 3.25, dropped: 1234, aec: false })));
    expect(l).toEqual({
      text: `Echo detected (coupling 0.72, 3 ms): 1,234 far-end words removed from "me".`,
      action: "restore",
      actionLabel: "Restore",
      aec: false,
    });
  });

  test("one word, echo cancellation on", () => {
    const l = echoLine(echoInfo(run({ coupled: true, coupling: 0.5, lag_ms: 12.6, dropped: 1, aec: true })))!;
    expect(l.text).toBe(`Echo detected (coupling 0.50, 13 ms): 1 far-end word removed from "me".`);
    expect(l.aec).toBe(true);
  });

  test("restored: the way back", () => {
    const l = echoLine(
      echoInfo(run({ coupled: true, coupling: 0.72, lag_ms: 3, dropped: 0, restored: true, restored_words: 312 })),
    )!;
    expect(l.text).toBe(`Echo detected (coupling 0.72, 3 ms): the 312 far-end words removed from "me" are back in this transcript.`);
    expect([l.action, l.actionLabel]).toEqual(["remove", "Remove echo words again"]);
  });

  test("turned off for the recording: no action", () => {
    const l = echoLine(echoInfo(run({ coupled: true, coupling: 0.8, lag_ms: null, dropped: 0, user_off: true })))!;
    expect(l.text).toBe(`Echo detected (coupling 0.80); echo handling was turned off for this recording, so nothing was removed from "me".`);
    expect(l.action).toBeNull();
  });

  test("nothing to say without echo, without the key, or for an older run", () => {
    expect(echoLine(echoInfo(run({ coupled: false, coupling: 0.04, dropped: 0 })))).toBeNull();
    expect(echoLine(echoInfo({ runtime: "x" }))).toBeNull();
    expect(echoLine(echoInfo(null))).toBeNull();
    expect(echoLine(echoInfo(run("not an object")))).toBeNull();
  });

  test("coupled but nothing removed, and a failed echo step", () => {
    expect(echoLine(echoInfo(run({ coupled: true, coupling: 0.4, lag_ms: 4, dropped: 0 })))!.text).toBe(
      `Echo detected (coupling 0.40, 4 ms); no words needed removing from "me".`,
    );
    const failed = echoLine(echoInfo(run({ error: "open mic.wav: not found" })))!;
    expect(failed.text).toBe(`The echo check did not run for this recording (open mic.wav: not found); nothing was removed from "me".`);
    expect(failed.action).toBeNull();
  });
});

describe("live banner", () => {
  test("the warning says what happens, the action says what it does", () => {
    expect(echoWarningText(false)).toBe(
      `Sound from the meeting reaches your microphone. Echoed words are removed from "me"; headphones would help.`,
    );
    expect(echoWarningText(true)).toBe("Sound from the meeting reaches your microphone. Echo cancellation is on.");
    expect(ECHO_OFF_ACTION).toBe("Turn off for this recording");
    expect(ECHO_OFF_KEY).toBe("N");
    expect(ECHO_OFF_NOTE).toBe(`Echo handling is off for this recording: everything the microphone picks up stays in "me".`);
  });
});

describe("settings", () => {
  test("echo.mode reads like the core", () => {
    expect(echoMode("Off")).toBe("off");
    expect(echoMode(" always ")).toBe("always");
    expect(echoMode("auto")).toBe("auto");
    expect(echoMode("anything")).toBe("auto");
    expect(ECHO_MODES.map(([, label]) => label)).toEqual(["Detect and handle", "Off", "Always on"]);
    expect(ECHO_MODE_HELP.split(". ").length).toBe(1);
  });

  test("the threshold is clamped to its range, two decimals", () => {
    expect(echoThreshold("0.45")).toBe("0.45");
    expect(echoThreshold("0.9")).toBe("0.6");
    expect(echoThreshold("0.1")).toBe("0.2");
    expect(echoThreshold("0.333")).toBe("0.33");
    expect(echoThreshold("high")).toBeNull();
    expect(echoThreshold(" ")).toBeNull();
    expect(ECHO_THRESHOLD_HELP).toContain("0.60 or more is always removed, under 0.20 always kept");
  });
});
