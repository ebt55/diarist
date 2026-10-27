// "Process again" (plan 16 section 12.5 J): how a job that ran a finished
// meeting's post-pass again ended, in the user's words. From the stored
// FLAC the meeting stays done with its current transcript whatever happens
// (the new run becomes current only once nothing can fail), so that is said
// even with the meeting open; from the WAVs a failed job marks the meeting
// failed and its banner says it.
import type { RerunAudio } from "./generated/RerunAudio";

export type AgainEnd = { kind: "success" | "info" | "error"; text: string };

/** The toast for a Process-again job that ended (`null`: none; `open`: its meeting is on screen). */
export function againEndText(
  type: "done" | "cancelled" | "error",
  title: string,
  again: RerunAudio,
  message: string,
  open: boolean,
): AgainEnd | null {
  if (type === "done") return open ? null : { kind: "success", text: `"${title}" was processed again` };
  if (type === "cancelled") {
    return { kind: "info", text: `Process again of "${title}" cancelled; the current transcript stays` };
  }
  if (again === "flac") {
    return {
      kind: "error",
      text: `Process again of "${title}" did not finish (${message}); the current transcript stays`,
    };
  }
  return open ? null : { kind: "error", text: `Process again of "${title}" did not finish (${message})` };
}
