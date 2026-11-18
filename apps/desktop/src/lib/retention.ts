// Audio retention warnings (diarist_core::retention, issue 22): the
// commands, and the pure helpers behind the library notices
// (components/RetentionNotice.svelte) and Settings.
import { invoke } from "@tauri-apps/api/core";
import { formatBytes, plural } from "./format";
import type { MeetingId } from "./generated/MeetingId";
import type { RetentionAction } from "./generated/RetentionAction";
import type { RetentionMeeting } from "./generated/RetentionMeeting";
import type { RetentionNotice } from "./generated/RetentionNotice";
import type { RetentionReport } from "./generated/RetentionReport";
import type { RetentionStatus } from "./generated/RetentionStatus";

export type { RetentionMeeting, RetentionNotice, RetentionStatus };

export const retentionApi = {
  /** What retention would delete and is waiting for. */
  status: () => invoke<RetentionStatus>("retention_preview"),
  /** A choice: delete (an approval), delete as set, or a notice was on screen. */
  act: (action: RetentionAction) => invoke<RetentionReport>("apply_retention", { action }),
};

/** Fired by the backend after each automatic check. */
export const RETENTION_EVENT = "retention:changed";
/** Fired on `window` by the UI after a choice, so Settings and the notices refetch. */
export const RETENTION_UI_EVENT = "app:retention";

export function retentionChanged(): void {
  window.dispatchEvent(new Event(RETENTION_UI_EVENT));
}

const DAY_MS = 86_400_000;

/** "today", "in 1 day", "in 3 days": when audio due at `dueAt` goes. */
export function whenText(dueAt: number, now: number): string {
  const days = Math.ceil((dueAt - now) / DAY_MS);
  return days <= 0 ? "today" : `in ${plural(days, "day")}`;
}

/** Bytes of a planned meeting (all its files). */
export function meetingBytes(m: RetentionMeeting): number {
  return m.files.reduce((sum, f) => sum + f.bytes, 0);
}

/** The entries whose meeting is not pinned (pins made since the status was read). */
export function withoutPinned<T extends { meeting_id: MeetingId }>(list: T[], pinned: ReadonlySet<MeetingId>): T[] {
  return list.filter((n) => !pinned.has(n.meeting_id));
}

/** Heads-up entries not yet reported as seen. */
export function toReport(list: RetentionNotice[], reported: ReadonlySet<MeetingId>): MeetingId[] {
  return list.map((n) => n.meeting_id).filter((id) => !reported.has(id));
}

/** The first-deletion hold's text. */
export function holdText(days: number, meetings: number, bytes: number): string {
  return (
    `Keep audio for is set to ${plural(days, "day")}. The audio of ${plural(meetings, "meeting")} ` +
    `(${formatBytes(bytes)}) is older than that. Nothing has been deleted yet. Transcripts always stay.`
  );
}

/** The heads-up's heading. */
export function headsUpTitle(meetings: number): string {
  return `Audio of ${plural(meetings, "meeting")} will be deleted soon`;
}

/** The held notice's heading. */
export function heldTitle(meetings: number): string {
  return `Audio of ${plural(meetings, "meeting")} is past Keep audio for`;
}

/** The storage prompt's text. */
export function storageText(bytes: number, days: number): string {
  return days === 0
    ? `Recordings now use ${formatBytes(bytes)}. Diarist keeps them until you set a limit.`
    : `Recordings now use ${formatBytes(bytes)}. Audio is kept for ${plural(days, "day")}.`;
}

/** The toast after "Delete as set". */
export function deletedAsSetText(meetings: number, bytes: number, headsUpDays: number): string {
  const warn = `From now on Diarist warns ${plural(headsUpDays, "day")} before it deletes audio.`;
  return meetings > 0 ? `Deleted the audio of ${plural(meetings, "meeting")} (${formatBytes(bytes)}). ${warn}` : warn;
}
