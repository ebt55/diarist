// Punctuation (plan 16 sections 3.8, 3.9 and 4.3): the meeting details line
// with Add and Remove punctuation, the toasts after them, the bulk action's
// confirmation and report, the Settings texts, the job list's words, and the
// shapes of the punctuation commands in src-tauri/src/commands.rs (mirrored
// by hand; they are not ts-rs types). Plain functions, so `bun test` covers
// the wording (tests/punctuation.test.ts).
import type { DownloadItem } from "./generated/DownloadItem";
import type { HealthCheck } from "./generated/HealthCheck";
import type { MeetingId } from "./generated/MeetingId";
import type { ModelFile } from "./generated/ModelFile";
import type { PunctReason } from "./generated/PunctReason";
import type { PunctReport } from "./generated/PunctReport";
import type { RunId } from "./generated/RunId";
import { languageName, primarySubtag } from "./languages";

// ---- Command results (commands.rs) ------------------------------------------------

/** What Add or Remove punctuation did to one meeting (`PunctOutcome`). */
export interface PunctOutcome {
  meeting_id: MeetingId;
  /** The new current run; null when nothing would have changed, after an error, when cancelled. */
  run: RunId | null;
  /** `engine_info.punctuation` of the new run, or what it would have been. */
  punctuation: PunctReport | null;
  /**
   * Why the meeting was left as it was: it changed meanwhile (`changed`), or,
   * in the bulk action only, any other error (a single meeting's command
   * rejects with it).
   */
  error: string | null;
  /** The transcript changed while the job ran (an edit finished first); nothing was switched. Worth another try. */
  changed: boolean;
  cancelled: boolean;
}

/** What the bulk action did (`BackfillOutcome`). */
export interface BackfillOutcome {
  total: number;
  outcomes: PunctOutcome[];
  /** Nothing could punctuate (no model, off, ONNX Runtime missing): nothing was done. */
  unavailable: PunctReport | null;
  /** The model failed beyond use part way: the meetings after the last outcome were left as they were. */
  stopped: PunctReport | null;
  cancelled: boolean;
}

/** What Settings shows (`PunctStatus`). */
export interface PunctStatus {
  /** The Diagnostics row "Punctuation runtime" (restart needed, a missing library). */
  check: HealthCheck;
  /** Finished meetings the bulk action would punctuate. */
  candidates: number;
  /** Finished meetings it leaves alone on purpose. */
  settled: number;
}

/** `punctuation:progress`: meetings done of a punctuation job. */
export const PUNCT_PROGRESS_EVENT = "punctuation:progress";
export interface PunctProgress {
  job_id: string;
  done: number;
  total: number;
}

/** One reason the "recommended downloads" notice gives (`NoticePart`). */
export interface NoticePart {
  id: string;
  title: string;
  text: string;
  items: DownloadItem[];
}

/** The "recommended downloads available" notice (`DownloadsNotice`); `key` is set to "true" once answered. */
export interface DownloadsNotice {
  key: string;
  parts: NoticePart[];
}

// ---- Reading a run ------------------------------------------------------------------

/** `engine_info.punctuation` of a run; null for a run from before punctuation (or anything unreadable). */
export function punctReport(engineInfo: unknown): PunctReport | null {
  if (typeof engineInfo !== "object" || engineInfo === null) return null;
  const p = (engineInfo as Record<string, unknown>).punctuation;
  if (typeof p !== "object" || p === null || Array.isArray(p)) return null;
  const o = p as Record<string, unknown>;
  if (typeof o.applied !== "boolean" || typeof o.reason !== "string") return null;
  return o as unknown as PunctReport;
}

// ---- The model --------------------------------------------------------------------

/**
 * The punctuation model a build would use (`punct::current`): the selected
 * file (`selected_model.punctuation`), else an installed one, manifest models
 * first; null when none is installed.
 */
export function punctModel(files: readonly ModelFile[], selected: string | undefined): ModelFile | null {
  const present = files.filter((f) => f.role === "punctuation" && !f.missing);
  const id = Number((selected ?? "").trim() || NaN);
  return present.find((f) => f.id === id) ?? present.find((f) => f.manifest_name !== null) ?? present[0] ?? null;
}

/** A model's short name: a manifest label's part before the colon ("Punctuation small: 28 languages"). */
export function punctModelName(f: ModelFile): string {
  return f.display_name.split(":")[0]?.trim() || f.display_name;
}

/** The name `engine_info.punctuation.model` (a manifest name or a file's name) stands for. */
export function reportModelName(model: string | undefined, files: readonly ModelFile[]): string | null {
  if (!model) return null;
  const f = files.find((x) => x.role === "punctuation" && (x.manifest_name === model || x.display_name === model));
  return f ? punctModelName(f) : model;
}

/** Whether `model` is the one a report names. */
function isReportModel(model: ModelFile, report: PunctReport): boolean {
  return report.model !== undefined && (report.model === model.manifest_name || report.model === model.display_name);
}

/** Whether the model covers `language` (`*` covers all; a model that lists none is not checked). */
export function modelCovers(f: ModelFile, language: string): boolean {
  const langs = f.capabilities.languages.map((l) => (l.trim() === "*" ? "*" : primarySubtag(l)));
  if (langs.length === 0 || langs.includes("*")) return true;
  return langs.includes(primarySubtag(language));
}

// ---- Words ------------------------------------------------------------------------

const count = (n: number, one: string, many: string) => `${n.toLocaleString("en-US")} ${n === 1 ? one : many}`;

/** "Spanish" for "Spanish, Americas". */
const baseName = (name: string) => name.split(", ")[0] ?? name;

const CHANNEL: Record<string, string> = {
  mic: "your microphone",
  loopback: "the meeting audio",
  source: "the recording",
};

function channelNote(why: PunctReason): string {
  switch (why) {
    case "asr_punctuates":
      return "the speech model's own punctuation";
    case "language":
      return "its language is not covered";
    default:
      return "left as it was";
  }
}

const message = (r: PunctReport) => (r.message ? ` (${r.message})` : "");

/** The language a report was punctuated as, by name; null when only the script decided. */
function reportLanguage(r: PunctReport, name: (tag: string) => string): string | null {
  if (!r.language || r.language_source === "script") return null;
  return baseName(name(r.language));
}

/**
 * What `engine_info.punctuation` says, in the user's words: "Punctuation:
 * <model> (N marks, M capitals)" or why it did not run.
 */
export function punctReasonText(
  r: PunctReport,
  files: readonly ModelFile[] = [],
  name: (tag: string) => string = languageName,
): string {
  const model = reportModelName(r.model, files);
  if (r.applied) {
    const marks = count(r.marks_added ?? 0, "mark", "marks");
    const caps = count(r.case_upgrades ?? 0, "capital", "capitals");
    const kept = Object.entries(r.channels ?? {})
      .filter(([, why]) => why !== "ok")
      .map(([c, why]) => `${CHANNEL[c] ?? c}: ${channelNote(why)}`);
    return `Punctuation: ${model ?? "added"} (${marks}, ${caps})${kept.length ? `; ${kept.join("; ")}` : ""}.`;
  }
  switch (r.reason) {
    case "off":
      return r.rebuilt_from !== undefined
        ? "Punctuation removed: this transcript shows the speech model's own text."
        : "No punctuation: it was off in Settings when this transcript was made.";
    case "no_model":
      return "No punctuation: no punctuation model was installed when this transcript was made.";
    case "language": {
      const l = r.language?.toLowerCase();
      if (l === "th" || l === "thai") return "No punctuation: Thai is written without sentence marks.";
      return `Punctuation: not available for ${reportLanguage(r, name) ?? "this language"} with ${model ?? "this model"}.`;
    }
    case "asr_punctuates":
      return "Punctuation: the speech model's own. It punctuates this language by itself, so only sentence starts were capitalised.";
    case "runtime":
      return `Punctuation did not run: ONNX Runtime could not start${message(r)}.`;
    case "error":
      return `Punctuation did not run${message(r)}; the speech model's text stays.`;
    case "invariant":
      return "Punctuation was not applied: the model's labels did not fit the words, so the speech model's text stays.";
    default:
      return "No punctuation was added.";
  }
}

// ---- The meeting details line -------------------------------------------------------

export interface PunctContext {
  /** The model Add would use (`punctModel`); null: none installed. */
  model: ModelFile | null;
  /** Every model file, to name a report's model. */
  files: readonly ModelFile[];
  /** `transcript.punctuation` is `off`. */
  off: boolean;
  /** The meeting's language (`meetings.language`). */
  language: string;
  name?: (tag: string) => string;
}

export interface PunctLine {
  text: string;
  /** `add`: Add punctuation; `remove`: Remove punctuation. */
  action: "add" | "remove" | null;
  actionLabel: string | null;
  /** The next step elsewhere: Settings (a model, the setting) or Diagnostics (the runtime). */
  link: "settings" | "diagnostics" | null;
  linkLabel: string | null;
}

export const ADD_LABEL = "Add punctuation";
export const REMOVE_LABEL = "Remove punctuation";

/**
 * The details line for the current run's `engine_info.punctuation` (null for
 * a run from before punctuation) and its actions: Remove when the run is
 * punctuated; Add when it is not, a model is installed, the setting is on and
 * a rebuild could change something (not when the speech model punctuates by
 * itself, nor for a language the same model was found not to cover).
 */
export function punctLine(r: PunctReport | null, ctx: PunctContext): PunctLine | null {
  const name = ctx.name ?? languageName;
  const line = (text: string, action: PunctLine["action"] = null): PunctLine => ({
    text,
    action,
    actionLabel: action === "add" ? ADD_LABEL : action === "remove" ? REMOVE_LABEL : null,
    link: null,
    linkLabel: null,
  });
  if (r?.applied) return line(punctReasonText(r, ctx.files, name), "remove");
  const model = ctx.model;
  const canAdd = model !== null && !ctx.off;
  // Nothing ran on this run and nothing can: nothing to say.
  if (r === null) return canAdd ? line("No punctuation yet.", "add") : null;
  const out = line(punctReasonText(r, ctx.files, name));
  switch (r.reason) {
    case "asr_punctuates":
      return out;
    case "language": {
      const lang = r.language && r.language_source !== "script" ? r.language : ctx.language;
      if (model !== null && !ctx.off && !isReportModel(model, r) && modelCovers(model, lang)) out.action = "add";
      break;
    }
    case "runtime":
      if (canAdd) out.action = "add";
      out.link = "diagnostics";
      out.linkLabel = "Diagnostics";
      break;
    default:
      if (canAdd) out.action = "add";
  }
  if (out.action === "add") out.actionLabel = ADD_LABEL;
  else if (out.link === null && model === null && (r.reason === "no_model" || r.reason === "off")) {
    out.link = "settings";
    out.linkLabel = "Get a punctuation model";
  } else if (out.link === null && ctx.off && r.reason !== "language") {
    out.link = "settings";
    out.linkLabel = "Punctuation settings";
  }
  return out;
}

// ---- After Add or Remove -----------------------------------------------------------

export interface Notice {
  kind: "info" | "success" | "error";
  text: string;
  /** Offer Try again. */
  retry?: boolean;
}

/** What a rebuild that lost a race says (commands.rs `PUNCT_CHANGED_*`). */
export function changedText(remove: boolean): string {
  return `This meeting changed while punctuation was being ${remove ? "removed" : "added"}. Try again.`;
}

/** The toast after Add (or Remove) punctuation on one meeting. */
export function punctOutcomeText(o: PunctOutcome, remove: boolean, files: readonly ModelFile[] = []): Notice {
  const what = remove ? REMOVE_LABEL : ADD_LABEL;
  if (o.cancelled) return { kind: "info", text: `${what} was cancelled; the transcript is as it was.` };
  if (o.changed) return { kind: "error", text: o.error ?? changedText(remove), retry: true };
  const r = o.punctuation;
  if (remove) {
    return o.run !== null
      ? { kind: "success", text: "Punctuation removed. The punctuated transcript stays; Add punctuation puts it back." }
      : { kind: "info", text: "There was no punctuation to remove." };
  }
  if (o.run !== null && r?.applied) {
    const marks = count(r.marks_added ?? 0, "mark", "marks");
    const caps = count(r.case_upgrades ?? 0, "capital", "capitals");
    return {
      kind: "success",
      text: `Punctuation added (${marks}, ${caps}). The earlier transcript stays; Remove punctuation switches back.`,
    };
  }
  if (r?.applied) return { kind: "info", text: "The transcript already has this punctuation; nothing changed." };
  return { kind: "info", text: `${r ? punctReasonText(r, files) : "No punctuation was added."} Nothing changed.` };
}

/** The error toast when Add or Remove could not run (the meeting records, it changed meanwhile, …); it offers Try again. */
export function punctErrorText(remove: boolean, error: string): string {
  return `${remove ? REMOVE_LABEL : ADD_LABEL} did not work: ${error.replace(/\.$/, "")}.`;
}

export const TRY_AGAIN = "Try again";

// ---- The bulk action ---------------------------------------------------------------

export function backfillLabel(n: number): string {
  return `Add punctuation to ${count(n, "older meeting", "older meetings")}`;
}

/** The confirmation: the count, what changes, and that the earlier runs stay. */
export function backfillConfirm(n: number): { title: string; message: string; confirmLabel: string } {
  return {
    title: `${backfillLabel(n)}?`,
    message:
      `The transcript you see changes on ${count(n, "meeting", "meetings")}: each gets a new version with ` +
      `punctuation, made from the speech model's words. The earlier versions stay, and Remove punctuation on a ` +
      `meeting switches back. It runs on the CPU, a few seconds per meeting, after any transcription in the queue.`,
    confirmLabel: ADD_LABEL,
  };
}

/** Reasons a meeting got no new run that are failures, not a settled answer. */
const FAILED: PunctReason[] = ["error", "runtime", "invariant", "no_model", "off"];

/** Why nothing could punctuate, for the bulk report. */
function unavailableText(r: PunctReport): string {
  switch (r.reason) {
    case "no_model":
      return "no punctuation model is installed";
    case "off":
      return "punctuation is off in Settings";
    case "runtime":
      return `ONNX Runtime could not start${message(r)}; Diagnostics says what to do`;
    default:
      return `the punctuation model did not load${message(r)}`;
  }
}

/** Why one meeting of the bulk action got no punctuation, short. */
function failureText(r: PunctReport): string {
  if (r.message) return r.message;
  return r.reason === "invariant" ? "the model's labels did not fit the words" : unavailableText(r);
}

/** The report when the bulk action is done. */
export function backfillDoneText(o: BackfillOutcome): Notice {
  if (o.unavailable) return { kind: "info", text: `No punctuation was added: ${unavailableText(o.unavailable)}.` };
  if (o.cancelled && o.outcomes.length === 0) {
    return { kind: "info", text: "Adding punctuation to older meetings was cancelled; nothing changed." };
  }
  if (o.total === 0) {
    return { kind: "info", text: "Every finished meeting has punctuation already, or needs none." };
  }
  const added = o.outcomes.filter((x) => x.run !== null && x.punctuation?.applied).length;
  // No new run: settled (a language the model does not cover, the speech
  // model punctuates) or failed (the model, the runtime, the meeting).
  const why = (x: PunctOutcome): string | null =>
    (x.changed ? "it changed while this ran" : x.error) ??
    (x.run === null && x.punctuation && !x.punctuation.applied && FAILED.includes(x.punctuation.reason)
      ? failureText(x.punctuation)
      : null);
  const failed = o.outcomes.filter((x) => !x.cancelled && why(x) !== null);
  const same = o.outcomes.filter((x) => x.run === null && !x.cancelled && why(x) === null).length;
  const parts = [
    o.cancelled
      ? `Stopped: punctuation added to ${added} of ${count(o.total, "meeting", "meetings")}`
      : `Punctuation added to ${added} of ${count(o.total, "meeting", "meetings")}`,
  ];
  if (added > 0) parts[0] += "; the earlier versions stay";
  if (same > 0) {
    parts.push(
      `${count(same, "meeting stayed as it was", "meetings stayed as they were")} (the model does not cover the ` +
        `language, or the speech model punctuates it)`,
    );
  }
  if (failed.length > 0) {
    parts.push(
      `${count(failed.length, "meeting", "meetings")} could not be changed now (${why(failed[0]!)?.replace(/\.$/, "")}); ` +
        `try again later`,
    );
  }
  if (o.stopped) {
    parts.push(
      `Stopped there: the punctuation model failed${message(o.stopped)}; the remaining meetings were left as they were`,
    );
  }
  const quiet = failed.length > 0 || o.cancelled || o.stopped !== null;
  return { kind: quiet ? "info" : "success", text: `${parts.join(". ")}.` };
}

// ---- Settings -----------------------------------------------------------------------

export const PUNCT_ROLE_HELP = "Punctuation is added after Stop and to imports; it never changes words or times.";

export const PUNCT_NONE_HELP =
  "No punctuation model yet. Download it in the list above, or put a model folder with its .meet.json sidecar " +
  "into the models folder and press Refresh.";

export const PUNCT_RESTART_NOTE =
  "ONNX Runtime loads once per run of Diarist: a model whose folder brings another version of it takes effect " +
  "after a restart.";

/** `transcript.punctuation` as `diarist_core::punct::setting` reads it. */
export type PunctMode = "auto" | "off";

export function punctMode(v: string): PunctMode {
  return ["off", "false", "0", "no", "none"].includes(v.trim().toLowerCase()) ? "off" : "auto";
}

export const PUNCT_MODES: [PunctMode, string][] = [
  ["auto", "Add automatically"],
  ["off", "Off"],
];

export const PUNCT_MODE_HELP =
  "Add automatically punctuates new transcripts when a punctuation model is installed and covers the meeting's " +
  "language; Off keeps the speech model's own text. Meetings you already have change only through Add or Remove " +
  "punctuation.";

/** The line under the bulk action. */
export function backfillHelp(s: PunctStatus): string {
  const settled =
    s.settled > 0
      ? ` ${count(s.settled, "meeting needs", "meetings need")} none: the speech model punctuates it, the model ` +
        `does not cover the language, or the punctuation was removed by hand.`
      : "";
  return s.candidates === 0
    ? `Every finished meeting has punctuation.${settled}`
    : `Older meetings keep their transcript until you add punctuation; each keeps its earlier version too.${settled}`;
}

// ---- The job list -------------------------------------------------------------------

export function punctJobName(remove: boolean, title: string): string {
  return `${remove ? "Removing" : "Adding"} punctuation: ${title || "Untitled meeting"}`;
}

export const BACKFILL_JOB_NAME = "Adding punctuation to older meetings";

/** The job list's stage text for a punctuation job (`doneMs`/`totalMs` count meetings). */
export function punctJobStage(started: boolean, done: number, total: number | null): string {
  if (!started) return "Queued";
  return total !== null && total > 1 ? `${done} of ${total} meetings` : "Working";
}
