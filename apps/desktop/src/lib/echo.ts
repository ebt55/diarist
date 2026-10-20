// Recording with speakers (plan 15): the words shown for the echo handling
// in the live banner, the meeting details and Settings. Plain functions, so
// `bun test` covers the wording.
import { ECHO_GATE } from "./generated/settingDefaults";

/** The live warning (`LiveWarning::Echo`), before its action. */
export function echoWarningText(aec: boolean): string {
  return aec
    ? "Sound from the meeting reaches your microphone. Echo cancellation is on."
    : `Sound from the meeting reaches your microphone. Echoed words are removed from "me"; headphones would help.`;
}

/** The banner after "Turn off for this recording". */
export const ECHO_OFF_NOTE = `Echo handling is off for this recording: everything the microphone picks up stays in "me".`;

/** The banner's action and its key. */
export const ECHO_OFF_ACTION = "Turn off for this recording";
export const ECHO_OFF_KEY = "N";

/** `engine_info.echo` of a run, as far as the UI reads it. */
export interface EchoInfo {
  coupled: boolean;
  coupling: number | null;
  lagMs: number | null;
  dropped: number;
  aec: boolean;
  userOff: boolean;
  restored: boolean;
  restoredWords: number;
  error: string | null;
}

const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

/** Reads `engine_info.echo` (null when the run has none: echo handling off, or an older run). */
export function echoInfo(engineInfo: unknown): EchoInfo | null {
  if (typeof engineInfo !== "object" || engineInfo === null) return null;
  const e = (engineInfo as Record<string, unknown>).echo;
  if (typeof e !== "object" || e === null || Array.isArray(e)) return null;
  const o = e as Record<string, unknown>;
  return {
    coupled: o.coupled === true,
    coupling: num(o.coupling),
    lagMs: num(o.lag_ms),
    dropped: num(o.dropped) ?? 0,
    aec: o.aec === true,
    userOff: o.user_off === true,
    restored: o.restored === true,
    restoredWords: num(o.restored_words) ?? 0,
    error: typeof o.error === "string" ? o.error : null,
  };
}

const words = (n: number) => `${n.toLocaleString("en-US")} far-end word${n === 1 ? "" : "s"}`;

/** "Echo detected (coupling 0.72, 3 ms)". */
function detected(e: EchoInfo): string {
  const parts = [];
  if (e.coupling !== null) parts.push(`coupling ${e.coupling.toFixed(2)}`);
  if (e.lagMs !== null) parts.push(`${Math.round(e.lagMs)} ms`);
  return parts.length > 0 ? `Echo detected (${parts.join(", ")})` : "Echo detected";
}

/** The meeting details' echo line and its action, or null when there is nothing to say. */
export interface EchoLine {
  text: string;
  /** `restore`: put the removed words back; `remove`: switch back to the run without them. */
  action: "restore" | "remove" | null;
  actionLabel: string | null;
  /** "Echo cancellation on". */
  aec: boolean;
}

export function echoLine(e: EchoInfo | null): EchoLine | null {
  if (!e) return null;
  if (e.error !== null) {
    return {
      text: `The echo check did not run for this recording (${e.error}); nothing was removed from "me".`,
      action: null,
      actionLabel: null,
      aec: false,
    };
  }
  if (e.restored) {
    return {
      text: `${detected(e)}: the ${words(e.restoredWords)} removed from "me" are back in this transcript.`,
      action: "remove",
      actionLabel: "Remove echo words again",
      aec: e.aec,
    };
  }
  if (!e.coupled && e.dropped === 0) return null;
  if (e.userOff) {
    return {
      text: `${detected(e)}; echo handling was turned off for this recording, so nothing was removed from "me".`,
      action: null,
      actionLabel: null,
      aec: e.aec,
    };
  }
  if (e.dropped === 0) {
    return { text: `${detected(e)}; no words needed removing from "me".`, action: null, actionLabel: null, aec: e.aec };
  }
  return {
    text: `${detected(e)}: ${words(e.dropped)} removed from "me".`,
    action: "restore",
    actionLabel: "Restore",
    aec: e.aec,
  };
}

/** `echo.mode` as `diarist_core::audio::echo::EchoMode::parse` reads it. */
export type EchoModeValue = "auto" | "off" | "always";

export function echoMode(v: string): EchoModeValue {
  const s = v.trim().toLowerCase();
  return s === "off" || s === "always" ? s : "auto";
}

export const ECHO_MODES: [EchoModeValue, string][] = [
  ["auto", "Detect and handle"],
  ["off", "Off"],
  ["always", "Always on"],
];

export const ECHO_MODE_HELP =
  `Without headphones the microphone also hears the meeting: Detect and handle removes the echoed words ` +
  `from "me" when it hears that, Always on does so in every recording, and Off keeps every word.`;

export const ECHO_THRESHOLD_HELP =
  `A mic word that correlates with the system audio at ${ECHO_GATE.dropAt.toFixed(2)} or more is always ` +
  `removed, under ${ECHO_GATE.keepUnder.toFixed(2)} always kept; in between it is removed when the median ` +
  `of its neighbours reaches this value. Lower removes more echo, and more of your own words spoken over ` +
  `the meeting. Applies from the next recording.`;

/** The threshold as stored: clamped to the accepted range, two decimals; null when not a number. */
export function echoThreshold(input: string): string | null {
  const v = Number(input.trim());
  if (input.trim() === "" || !Number.isFinite(v)) return null;
  const c = Math.min(ECHO_GATE.max, Math.max(ECHO_GATE.min, v));
  return String(Math.round(c * 100) / 100);
}
