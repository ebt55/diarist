// Which meeting a `live:state` update opens. Free of Svelte and Tauri so
// `bun test` can run it (tests/openOnce.test.ts).
import type { LivePhase } from "./generated/LivePhase";
import type { MeetingId } from "./generated/MeetingId";

/**
 * Opens each recording's meeting once, on its first `recording` update
 * (the updates repeat while it runs). The mark is cleared between
 * recordings instead of compared across them: meeting ids repeat (SQLite
 * gives a new meeting the id of a deleted newest one), so a new recording
 * can carry the previous recording's id and must still open.
 */
export class OpenOnce {
  #openedFor: MeetingId | null = null;

  /** The meeting to open for this update, or null. */
  update(phase: LivePhase, meetingId: MeetingId | null): MeetingId | null {
    if (phase === "idle" || phase === "starting") {
      this.#openedFor = null;
      return null;
    }
    if (phase === "recording" && meetingId !== null && this.#openedFor !== meetingId) {
      this.#openedFor = meetingId;
      return meetingId;
    }
    return null;
  }
}
