// `bun test` (apps/desktop): what a meeting delete clears in the window's state.
import { describe, expect, test } from "bun:test";
import { forgetMeeting, type Remembered } from "../src/lib/forget";

type Focus = { meetingId: number; segmentId: number };
type Toast = { id: number; action?: { label: string; meetingId?: number } };

function state(): Remembered<Focus, Toast> {
  return {
    openId: 7,
    focus: { meetingId: 7, segmentId: 70 },
    postPassErrors: new Map([
      [7, "engine exited"],
      [8, "out of memory"],
    ]),
    toasts: [
      { id: 1, action: { label: "Open", meetingId: 7 } },
      { id: 2, action: { label: "Show log" } },
      { id: 3 },
      { id: 4, action: { label: "Open", meetingId: 8 } },
    ],
  };
}

describe("forgetMeeting", () => {
  test("clears everything kept for the deleted meeting", () => {
    const after = forgetMeeting(state(), 7);
    expect(after.openId).toBeNull();
    expect(after.focus).toBeNull();
    expect([...after.postPassErrors.keys()]).toEqual([8]);
    expect(after.toasts.map((t) => t.id)).toEqual([2, 3, 4]);
  });

  test("a recreated id finds nothing of the deleted meeting", () => {
    // Delete 7, then a new meeting gets 7 again (a beta.1 database, once).
    const after = forgetMeeting(state(), 7);
    expect(after.postPassErrors.get(7)).toBeUndefined();
    expect(after.focus?.meetingId === 7).toBe(false);
  });

  test("leaves other meetings alone and keeps unchanged fields as they were", () => {
    const before = state();
    before.openId = 8;
    before.focus = { meetingId: 8, segmentId: 80 };
    const after = forgetMeeting(before, 9);
    expect(after.openId).toBe(8);
    expect(after.focus).toBe(before.focus);
    expect(after.postPassErrors).toBe(before.postPassErrors);
    expect(after.toasts).toBe(before.toasts);
  });

  test("the input is not changed", () => {
    const before = state();
    forgetMeeting(before, 7);
    expect(before.openId).toBe(7);
    expect(before.postPassErrors.has(7)).toBe(true);
    expect(before.toasts).toHaveLength(4);
  });
});
