// `bun test` (apps/desktop): jump to a speaker's next or previous line (issue 21).
import { describe, expect, test } from "bun:test";
import { playheadFrom, speakerLine } from "../src/lib/speakerJump";

const A = 1;
const B = 2;
// 0: A 1-3 s, 1: B 3-5 s, 2: A 5-7 s, gap, 3: B 10-12 s, 4: A 12-14 s
const lines = [
  { start_ms: 1_000, end_ms: 3_000, speaker_id: A },
  { start_ms: 3_000, end_ms: 5_000, speaker_id: B },
  { start_ms: 5_000, end_ms: 7_000, speaker_id: A },
  { start_ms: 10_000, end_ms: 12_000, speaker_id: B },
  { start_ms: 12_000, end_ms: 14_000, speaker_id: A },
];

describe("speakerLine", () => {
  test("next and previous skip other speakers", () => {
    expect(speakerLine(lines, 0, A, 1)).toBe(2);
    expect(speakerLine(lines, 2, A, 1)).toBe(4);
    expect(speakerLine(lines, 4, A, -1)).toBe(2);
    expect(speakerLine(lines, 0, B, 1)).toBe(1);
  });

  test("repeated jumps walk every line of the speaker", () => {
    const seen: number[] = [];
    for (let at = -1; (at = speakerLine(lines, at, A, 1)) >= 0; ) seen.push(at);
    expect(seen).toEqual([0, 2, 4]);
  });

  test("none past either end", () => {
    expect(speakerLine(lines, 4, A, 1)).toBe(-1);
    expect(speakerLine(lines, 3, B, 1)).toBe(-1);
    expect(speakerLine(lines, 0, A, -1)).toBe(-1);
    expect(speakerLine(lines, 1, B, -1)).toBe(-1);
    expect(speakerLine(lines, 2, 99, 1)).toBe(-1);
    expect(speakerLine([], -1, A, 1)).toBe(-1);
  });

  test("the ends as starting points", () => {
    expect(speakerLine(lines, -1, B, 1)).toBe(1);
    expect(speakerLine(lines, lines.length, B, -1)).toBe(3);
  });
});

describe("playheadFrom", () => {
  const next = (ms: number, sp: number) => speakerLine(lines, playheadFrom(lines, ms, 1), sp, 1);
  const prev = (ms: number, sp: number) => speakerLine(lines, playheadFrom(lines, ms, -1), sp, -1);

  test("before the first line", () => {
    expect(next(0, A)).toBe(0);
    expect(prev(0, A)).toBe(-1);
  });

  test("inside a line: next skips it, previous skips it too", () => {
    expect(next(5_500, A)).toBe(4);
    expect(prev(5_500, A)).toBe(0);
    expect(next(1_000, A)).toBe(2);
  });

  test("in a gap: next is after it, previous finds the line that just ended", () => {
    expect(next(8_000, A)).toBe(4);
    expect(prev(8_000, A)).toBe(2);
    expect(next(8_000, B)).toBe(3);
  });

  test("after the last line", () => {
    expect(next(20_000, A)).toBe(-1);
    expect(prev(20_000, A)).toBe(4);
  });
});
