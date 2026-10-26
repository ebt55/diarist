// `bun test` (apps/desktop): the punctuation wording of the meeting details,
// the toasts, the bulk action and Settings (plan 16 sections 3.8 and 3.9).
import { describe, expect, test } from "bun:test";
import type { ModelFile } from "../src/lib/generated/ModelFile";
import type { PunctReport } from "../src/lib/generated/PunctReport";
import {
  ADD_LABEL,
  BACKFILL_JOB_NAME,
  PUNCT_MODES,
  PUNCT_ROLE_HELP,
  REMOVE_LABEL,
  backfillConfirm,
  backfillDoneText,
  backfillHelp,
  backfillLabel,
  modelCovers,
  punctErrorText,
  punctJobName,
  punctJobStage,
  punctLine,
  punctMode,
  punctModel,
  punctOutcomeText,
  punctReasonText,
  punctReport,
  type BackfillOutcome,
  type PunctContext,
  type PunctOutcome,
} from "../src/lib/punctuation";

const file = (id: number, patch: Partial<ModelFile> = {}, languages: string[] = ["*"]): ModelFile => ({
  id,
  path: `models/punct/${id}/model.onnx`,
  role: "punctuation",
  architecture: null,
  display_name: "Punctuation small: 28 languages",
  quant: null,
  size_bytes: 300_000_000,
  sha256: null,
  source: "manifest",
  capabilities: {
    streaming: false,
    diarize: false,
    languages,
    vram_mb_measured: null,
    vram_mb_estimate: null,
    file_right_context: null,
    file_preset: null,
  },
  manifest_name: "punct-small",
  first_seen_at: 0,
  last_seen_at: 0,
  missing: false,
  ...patch,
});

const report = (patch: Partial<PunctReport>): PunctReport => ({ applied: false, reason: "no_model", ...patch });
const info = (punctuation: unknown) => ({ runtime: "nemo-speech 0.2.0", punctuation });

const model = file(3);
const ctx = (patch: Partial<PunctContext> = {}): PunctContext => ({
  model,
  files: [model],
  off: false,
  language: "en",
  ...patch,
});

describe("reading engine_info", () => {
  test("a report, or null for older and unreadable runs", () => {
    expect(punctReport(info({ applied: true, reason: "ok", marks_added: 3 }))?.marks_added).toBe(3);
    expect(punctReport({ runtime: "x" })).toBeNull();
    expect(punctReport(null)).toBeNull();
    expect(punctReport(info("on"))).toBeNull();
    expect(punctReport(info({ reason: "ok" }))).toBeNull();
  });
});

describe("the model", () => {
  test("the selected file, else an installed one, manifest models first", () => {
    const drop = file(1, { manifest_name: null, display_name: "my-punct" });
    const gone = file(2, { missing: true });
    expect(punctModel([drop, gone, model], "1")?.id).toBe(1);
    expect(punctModel([drop, gone, model], "")?.id).toBe(3);
    expect(punctModel([drop, gone, model], "2")?.id).toBe(3);
    expect(punctModel([drop], undefined)?.id).toBe(1);
    expect(punctModel([gone, file(9, { role: "asr" })], "9")).toBeNull();
  });

  test("languages: * covers all, a list by primary subtag, none listed is not checked", () => {
    expect(modelCovers(model, "vi-VN")).toBe(true);
    expect(modelCovers(file(4, {}, ["en", "de-DE"]), "de")).toBe(true);
    expect(modelCovers(file(4, {}, ["en", "de-DE"]), "vi")).toBe(false);
    expect(modelCovers(file(4, {}, []), "vi")).toBe(true);
  });
});

describe("meeting details line", () => {
  test("punctuated: the model, the counts and Remove", () => {
    const l = punctLine(report({ applied: true, reason: "ok", model: "punct-small", marks_added: 1234, case_upgrades: 1 }), ctx())!;
    expect(l.text).toBe("Punctuation: Punctuation small (1,234 marks, 1 capital).");
    expect([l.action, l.actionLabel]).toEqual(["remove", REMOVE_LABEL]);
    expect(REMOVE_LABEL).toBe("Remove punctuation");
  });

  test("a channel the speech model punctuates is named", () => {
    const r = report({
      applied: true,
      reason: "ok",
      model: "punct-small",
      marks_added: 2,
      case_upgrades: 0,
      channels: { mic: "ok", loopback: "asr_punctuates" },
    });
    expect(punctReasonText(r, [model])).toBe(
      "Punctuation: Punctuation small (2 marks, 0 capitals); the meeting audio: the speech model's own punctuation.",
    );
  });

  test("not available for a language, in the user's words", () => {
    const r = report({ reason: "language", model: "punct-small", language: "vi", language_source: "meeting" });
    const l = punctLine(r, ctx({ language: "vi-VN" }))!;
    expect(l.text).toBe("Punctuation: not available for Vietnamese with Punctuation small.");
    // The same model: Add would change nothing.
    expect(l.action).toBeNull();
    // A model gone since: "this model".
    expect(punctReasonText(r, [])).toBe("Punctuation: not available for Vietnamese with punct-small.");
    expect(punctReasonText(report({ reason: "language", language: "vi" }))).toBe(
      "Punctuation: not available for Vietnamese with this model.",
    );
    expect(punctReasonText(report({ reason: "language", language: "es", language_source: "meeting" }))).toBe(
      "Punctuation: not available for Spanish with this model.",
    );
    expect(punctReasonText(report({ reason: "language", language: "latin", language_source: "script" }))).toBe(
      "Punctuation: not available for this language with this model.",
    );
    expect(punctReasonText(report({ reason: "language", language: "th" }))).toBe(
      "No punctuation: Thai is written without sentence marks.",
    );
  });

  test("another model that covers the language offers Add", () => {
    const r = report({ reason: "language", model: "punct-en", language: "vi", language_source: "meeting" });
    expect(punctLine(r, ctx())!.action).toBe("add");
    const enOnly = file(5, { manifest_name: "punct-en2" }, ["en"]);
    expect(punctLine(r, ctx({ model: enOnly, files: [enOnly] }))!.action).toBeNull();
  });

  test("the speech model punctuates: said, no action", () => {
    const l = punctLine(report({ reason: "asr_punctuates", model: "punct-small" }), ctx())!;
    expect(l.text).toBe(
      "Punctuation: the speech model's own. It punctuates this language by itself, so only sentence starts were capitalised.",
    );
    expect([l.action, l.link]).toEqual([null, null]);
  });

  test("no model then, one now: Add; none now: the way to get one", () => {
    const r = report({ reason: "no_model" });
    const l = punctLine(r, ctx())!;
    expect(l.text).toBe("No punctuation: no punctuation model was installed when this transcript was made.");
    expect([l.action, l.actionLabel]).toEqual(["add", ADD_LABEL]);
    const none = punctLine(r, ctx({ model: null, files: [] }))!;
    expect([none.action, none.link, none.linkLabel]).toEqual([null, "settings", "Get a punctuation model"]);
  });

  test("older runs: Add when a model is there, nothing otherwise", () => {
    expect(punctLine(null, ctx())).toEqual({
      text: "No punctuation yet.",
      action: "add",
      actionLabel: ADD_LABEL,
      link: null,
      linkLabel: null,
    });
    expect(punctLine(null, ctx({ model: null, files: [] }))).toBeNull();
    expect(punctLine(null, ctx({ off: true }))).toBeNull();
  });

  test("removed by hand, and off in Settings", () => {
    const removed = report({ reason: "off", rebuilt_from: 4 });
    const l = punctLine(removed, ctx())!;
    expect(l.text).toBe("Punctuation removed: this transcript shows the speech model's own text.");
    expect(l.action).toBe("add");
    const off = punctLine(report({ reason: "off" }), ctx({ off: true }))!;
    expect(off.text).toBe("No punctuation: it was off in Settings when this transcript was made.");
    expect([off.action, off.link, off.linkLabel]).toEqual([null, "settings", "Punctuation settings"]);
  });

  test("failures: the message, Add to try again, Diagnostics for the runtime", () => {
    const rt = punctLine(report({ reason: "runtime", message: "VCRUNTIME140_1.dll is missing" }), ctx())!;
    expect(rt.text).toBe("Punctuation did not run: ONNX Runtime could not start (VCRUNTIME140_1.dll is missing).");
    expect([rt.action, rt.link, rt.linkLabel]).toEqual(["add", "diagnostics", "Diagnostics"]);
    const err = punctLine(report({ reason: "error", message: "bad model" }), ctx({ model: null, files: [] }))!;
    expect(err.text).toBe("Punctuation did not run (bad model); the speech model's text stays.");
    expect(err.action).toBeNull();
    expect(punctReasonText(report({ reason: "invariant" }))).toBe(
      "Punctuation was not applied: the model's labels did not fit the words, so the speech model's text stays.",
    );
  });
});

describe("after Add or Remove", () => {
  const outcome = (patch: Partial<PunctOutcome>): PunctOutcome => ({
    meeting_id: 1,
    run: 9,
    punctuation: report({ applied: true, reason: "ok", marks_added: 40, case_upgrades: 12 }),
    error: null,
    cancelled: false,
    ...patch,
  });

  test("added, removed, nothing to do, cancelled", () => {
    expect(punctOutcomeText(outcome({}), false)).toEqual({
      kind: "success",
      text: "Punctuation added (40 marks, 12 capitals). The earlier transcript stays; Remove punctuation switches back.",
    });
    expect(punctOutcomeText(outcome({ punctuation: report({ reason: "off" }) }), true).text).toBe(
      "Punctuation removed. The punctuated transcript stays; Add punctuation puts it back.",
    );
    expect(punctOutcomeText(outcome({ run: null }), true).text).toBe("There was no punctuation to remove.");
    expect(punctOutcomeText(outcome({ run: null }), false).text).toBe(
      "The transcript already has this punctuation; nothing changed.",
    );
    expect(
      punctOutcomeText(outcome({ run: null, punctuation: report({ reason: "language", language: "vi" }) }), false).text,
    ).toBe("Punctuation: not available for Vietnamese with this model. Nothing changed.");
    expect(punctOutcomeText(outcome({ run: null, cancelled: true }), false)).toEqual({
      kind: "info",
      text: "Add punctuation was cancelled; the transcript is as it was.",
    });
  });

  test("an error says what did not work (the toast offers Try again)", () => {
    expect(punctErrorText(false, "This meeting is still recording.")).toBe(
      "Add punctuation did not work: This meeting is still recording.",
    );
    expect(punctErrorText(true, "The meeting changed meanwhile; try again")).toBe(
      "Remove punctuation did not work: The meeting changed meanwhile; try again.",
    );
  });
});

describe("the bulk action", () => {
  test("the confirmation names the count and says the earlier versions stay", () => {
    expect(backfillLabel(1)).toBe("Add punctuation to 1 older meeting");
    const c = backfillConfirm(1234);
    expect(c.title).toBe("Add punctuation to 1,234 older meetings?");
    expect(c.message).toContain("changes on 1,234 meetings");
    expect(c.message).toContain("The earlier versions stay");
    expect(c.confirmLabel).toBe("Add punctuation");
  });

  const done = (patch: Partial<BackfillOutcome>): BackfillOutcome => ({
    total: 0,
    outcomes: [],
    unavailable: null,
    cancelled: false,
    ...patch,
  });
  const o = (meeting_id: number, patch: Partial<PunctOutcome>): PunctOutcome => ({
    meeting_id,
    run: meeting_id + 100,
    punctuation: report({ applied: true, reason: "ok" }),
    error: null,
    cancelled: false,
    ...patch,
  });

  test("the report when done", () => {
    expect(backfillDoneText(done({ total: 2, outcomes: [o(1, {}), o(2, {})] }))).toEqual({
      kind: "success",
      text: "Punctuation added to 2 of 2 meetings; the earlier versions stay.",
    });
    const mixed = done({
      total: 4,
      outcomes: [
        o(1, {}),
        o(2, { run: null, punctuation: report({ reason: "language" }) }),
        o(3, { run: null, punctuation: null, error: "The meeting changed meanwhile; try again." }),
        o(4, { run: null, punctuation: report({ reason: "asr_punctuates" }) }),
      ],
    });
    expect(backfillDoneText(mixed)).toEqual({
      kind: "info",
      text:
        "Punctuation added to 1 of 4 meetings; the earlier versions stay. 2 meetings stayed as they were (the model " +
        "does not cover the language, or the speech model punctuates it). 1 meeting could not be changed now (The " +
        "meeting changed meanwhile; try again); try again later.",
    });
    expect(backfillDoneText(done({ total: 5, outcomes: [o(1, {})], cancelled: true })).text).toBe(
      "Stopped: punctuation added to 1 of 5 meetings; the earlier versions stay.",
    );
    expect(backfillDoneText(done({ cancelled: true })).text).toBe(
      "Adding punctuation to older meetings was cancelled; nothing changed.",
    );
    expect(backfillDoneText(done({ total: 4, cancelled: true })).text).toBe(
      "Adding punctuation to older meetings was cancelled; nothing changed.",
    );
  });

  test("nothing to do, or nothing to do it with", () => {
    expect(backfillDoneText(done({})).text).toBe("Every finished meeting has punctuation already, or needs none.");
    expect(backfillDoneText(done({ total: 3, unavailable: report({ reason: "no_model" }) })).text).toBe(
      "No punctuation was added: no punctuation model is installed.",
    );
    expect(
      backfillDoneText(done({ total: 3, unavailable: report({ reason: "runtime", message: "wrong module" }) })).text,
    ).toBe("No punctuation was added: ONNX Runtime could not start (wrong module); Diagnostics says what to do.");
  });

  test("the line under it", () => {
    const check = { id: "punctuation", label: "Punctuation runtime", status: "pass", detail: "", fix: null, fix_label: null };
    expect(backfillHelp({ check, candidates: 0, settled: 0 })).toBe("Every finished meeting has punctuation.");
    expect(backfillHelp({ check, candidates: 3, settled: 1 })).toContain(
      "1 meeting needs none: the speech model punctuates it",
    );
  });
});

describe("settings and the job list", () => {
  test("transcript.punctuation reads like the core", () => {
    expect(punctMode("auto")).toBe("auto");
    expect(punctMode(" OFF ")).toBe("off");
    expect(punctMode("none")).toBe("off");
    expect(punctMode("anything")).toBe("auto");
    expect(PUNCT_MODES.map(([, l]) => l)).toEqual(["Add automatically", "Off"]);
    expect(PUNCT_ROLE_HELP).toBe("Punctuation is added after Stop and to imports; it never changes words or times.");
  });

  test("job names and stages", () => {
    expect(punctJobName(false, "Standup")).toBe("Adding punctuation: Standup");
    expect(punctJobName(true, "")).toBe("Removing punctuation: Untitled meeting");
    expect(BACKFILL_JOB_NAME).toBe("Adding punctuation to older meetings");
    expect(punctJobStage(false, 0, null)).toBe("Queued");
    expect(punctJobStage(true, 0, 1)).toBe("Working");
    expect(punctJobStage(true, 3, 12)).toBe("3 of 12 meetings");
  });
});
