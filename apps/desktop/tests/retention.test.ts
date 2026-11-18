// `bun test` (apps/desktop): the retention notices' pure helpers
// (src/lib/retention.ts, components/RetentionNotice.svelte, issue 22).
import { describe, expect, test } from "bun:test";
import {
  deletedAsSetText,
  headsUpTitle,
  heldTitle,
  holdText,
  meetingBytes,
  storageText,
  toReport,
  whenText,
  withoutPinned,
  type RetentionMeeting,
  type RetentionNotice,
} from "../src/lib/retention";

const DAY = 86_400_000;
const NOW = 1_800_000_000_000;

const notice = (id: number, dueAt = NOW + DAY): RetentionNotice => ({
  meeting_id: id,
  title: `m${id}`,
  ended_at: dueAt - 30 * DAY,
  due_at: dueAt,
  bytes: 1000,
});

describe("retention helpers", () => {
  test("when the audio goes", () => {
    expect(whenText(NOW - 1, NOW)).toBe("today");
    expect(whenText(NOW, NOW)).toBe("today");
    expect(whenText(NOW + 5 * 3_600_000, NOW)).toBe("in 1 day");
    expect(whenText(NOW + 2 * DAY + 1, NOW)).toBe("in 3 days");
  });

  test("pins made since the status was read drop out", () => {
    const list = [notice(1), notice(2), notice(3)];
    expect(withoutPinned(list, new Set([2])).map((n) => n.meeting_id)).toEqual([1, 3]);
    expect(withoutPinned(list, new Set())).toHaveLength(3);
  });

  test("each heads-up meeting is reported once", () => {
    const list = [notice(1), notice(2)];
    expect(toReport(list, new Set())).toEqual([1, 2]);
    expect(toReport(list, new Set([1]))).toEqual([2]);
    expect(toReport(list, new Set([1, 2]))).toEqual([]);
  });

  test("a planned meeting's bytes add up its files", () => {
    const m: RetentionMeeting = {
      meeting_id: 1,
      title: "a",
      ended_at: NOW,
      files: [
        { path: "a.flac", bytes: 900, track_id: 1 },
        { path: "a.gain", bytes: 16, track_id: null },
      ],
    };
    expect(meetingBytes(m)).toBe(916);
  });

  test("texts are plain, with no semicolons", () => {
    const texts = [
      holdText(30, 2, 180_000_000),
      headsUpTitle(1),
      heldTitle(3),
      storageText(10_400_000_000, 0),
      storageText(10_400_000_000, 30),
      deletedAsSetText(2, 180_000_000, 3),
      deletedAsSetText(0, 0, 3),
    ];
    for (const t of texts) expect(t).not.toContain(";");
    expect(holdText(30, 2, 180_000_000)).toBe(
      "Keep audio for is set to 30 days. The audio of 2 meetings (180 MB) is older than that. " +
        "Nothing has been deleted yet. Transcripts always stay.",
    );
    expect(headsUpTitle(1)).toBe("Audio of 1 meeting will be deleted soon");
    expect(storageText(10_400_000_000, 0)).toBe(
      "Recordings now use 10.40 GB. Diarist keeps them until you set a limit.",
    );
    expect(deletedAsSetText(0, 0, 3)).toBe("From now on Diarist warns 3 days before it deletes audio.");
  });
});
