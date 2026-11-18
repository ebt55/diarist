// Jump to a speaker's next or previous line (issue 21). Segments are in
// time order; `speaker_id` is the resolved (root) speaker.

export type JumpDir = 1 | -1;

interface Line {
  start_ms: number;
  end_ms: number;
  speaker_id: number;
}

/**
 * Index of the first line by `speaker` after (`dir` 1) or before (`dir` -1)
 * the line at index `from`, or -1 when there is none. `from` may be -1
 * (before the first line) or `lines.length` (after the last).
 */
export function speakerLine(lines: readonly Line[], from: number, speaker: number, dir: JumpDir): number {
  for (let i = from + dir; i >= 0 && i < lines.length; i += dir) {
    if (lines[i]?.speaker_id === speaker) return i;
  }
  return -1;
}

/**
 * Where a jump from the playhead at `ms` starts, as a `from` for
 * `speakerLine`. Next: the last line that started at or before `ms`, so
 * the line playing now is skipped. Previous: the line playing now, else the
 * first line after `ms`, so a line that just ended is found.
 */
export function playheadFrom(lines: readonly Line[], ms: number, dir: JumpDir): number {
  let last = -1;
  let lo = 0;
  let hi = lines.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if ((lines[mid]?.start_ms ?? 0) <= ms) {
      last = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  if (dir === 1) return last;
  const playing = lines[last];
  return playing && ms < playing.end_ms ? last : last + 1;
}
