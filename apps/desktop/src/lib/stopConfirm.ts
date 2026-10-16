// The one "Stop recording?" question, wherever Stop comes from: the Stop
// buttons, Ctrl+., and Ctrl+Alt+R or the tray while the window is on screen
// (`live:toggled` `confirm_stop`). With the window hidden the backend asks
// the same question in a notification and a second press within 3 s stops
// (`stop_armed`); a second press while the dialog is open confirms it the
// same way. Free of Svelte and Tauri so `bun test` can run it
// (tests/stopConfirm.test.ts).

export const STOP_QUESTION = "Stop recording?";

/** The dialog's title, text and button for a recording `elapsedMs` long. */
export function stopDialog(elapsedMs: number): { title: string; message: string; confirmLabel: string } {
  const secs = Math.max(0, Math.round(elapsedMs / 1000));
  const length = secs < 120 ? `${secs} s` : `${Math.round(secs / 60)} min`;
  return {
    title: STOP_QUESTION,
    message: `${length} recorded. The transcript is finalized from the recording after you stop.`,
    confirmLabel: "Stop",
  };
}

/**
 * What a Ctrl+Alt+R or tray Stop does with the window on screen: open the
 * question, or, when it is already open, answer it (the second press).
 */
export function stopPress(questionOpen: boolean): "ask" | "confirm" {
  return questionOpen ? "confirm" : "ask";
}

/** The reminder after a first press with the window hidden (the backend's notification says the same). */
export function armedNote(via: "shortcut" | "tray"): string {
  return via === "shortcut"
    ? `${STOP_QUESTION} Press Ctrl+Alt+R again within 3 s to stop.`
    : `${STOP_QUESTION} Choose Stop recording in the tray again within 3 s to stop.`;
}
