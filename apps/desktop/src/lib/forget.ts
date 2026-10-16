// What the window remembers by meeting id, and how a delete clears it.
// Ids are not unique forever: a database from beta.1 can give a deleted
// meeting's id to the next new meeting once (ids come from a counter after
// the 0002 migration), so nothing kept for the deleted meeting may outlive
// the delete. Free of Svelte and Tauri so `bun test` can run it
// (tests/forget.test.ts).
import type { MeetingId } from "./generated/MeetingId";

/** The parts of the app state that hold meeting ids. */
export interface Remembered<F extends { meetingId: MeetingId }, T extends { action?: { meetingId?: MeetingId } }> {
  /** The open meeting. */
  openId: MeetingId | null;
  /** A pending scroll to a search hit. */
  focus: F | null;
  /** Why a meeting's post-pass failed (this session). */
  postPassErrors: ReadonlyMap<MeetingId, string>;
  /** Toasts; an action that opens a meeting names it. */
  toasts: readonly T[];
}

/**
 * `r` without anything that points at meeting `id`. Fields with nothing to
 * clear are returned as they were (the same object), so assigning them back
 * to Svelte state changes nothing.
 */
export function forgetMeeting<F extends { meetingId: MeetingId }, T extends { action?: { meetingId?: MeetingId } }>(
  r: Remembered<F, T>,
  id: MeetingId,
): Remembered<F, T> {
  let postPassErrors = r.postPassErrors;
  if (postPassErrors.has(id)) {
    const next = new Map(postPassErrors);
    next.delete(id);
    postPassErrors = next;
  }
  const opens = (t: T) => t.action?.meetingId === id;
  return {
    openId: r.openId === id ? null : r.openId,
    focus: r.focus?.meetingId === id ? null : r.focus,
    postPassErrors,
    toasts: r.toasts.some(opens) ? r.toasts.filter((t) => !opens(t)) : r.toasts,
  };
}
