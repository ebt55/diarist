// App-wide state: route, meeting list, open meeting, jobs, toasts, theme.
// Svelte 5 runes in a module; components import the `app` singleton.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, errorText, langApi, onEvent, onJobEvent, punctApi } from "./api";
import { CPU_NOTICE, isCpuPackage, selectedEngine } from "./cpuNotice";
import { forgetMeeting } from "./forget";
import { importSheet } from "./importSheet.svelte";
import { UNKNOWN_TOKENS_NOTICE, languageName } from "./languages";
import type { JobEvent } from "./generated/JobEvent";
import type { JobStage } from "./generated/JobStage";
import type { MeetingId } from "./generated/MeetingId";
import type { MeetingSummary } from "./generated/MeetingSummary";
import type { MissingPart } from "./generated/MissingPart";
import type { PostPassView } from "./generated/PostPassView";
import type { RerunAudio } from "./generated/RerunAudio";
import { againEndText } from "./processAgain";
import type { SegmentId } from "./generated/SegmentId";
import type { Settings } from "./generated/Settings";
import type { StartupReport } from "./generated/StartupReport";
import { SETTING_DEFAULTS, type SettingKey } from "./generated/settingDefaults";
import {
  BACKFILL_JOB_NAME,
  PUNCT_PROGRESS_EVENT,
  TRY_AGAIN,
  backfillDoneText,
  punctErrorText,
  punctJobName,
  punctOutcomeText,
  type BackfillOutcome,
  type PunctOutcome,
  type PunctProgress,
} from "./punctuation";

export type Route = "library" | "settings" | "diagnostics" | "setup";

/** Set by the backend before the page loads (`lib.rs`). */
export interface StartupFlags {
  /** Started by Windows at sign-in: the window stays in the tray. */
  autostart: boolean;
  /** A saved window size and position were restored. */
  windowStateSaved: boolean;
}

export function startupFlags(): StartupFlags {
  const f = (window as unknown as { __DIARIST_STARTUP__?: Partial<StartupFlags> }).__DIARIST_STARTUP__;
  return { autostart: f?.autostart ?? false, windowStateSaved: f?.windowStateSaved ?? false };
}
export type Theme = "system" | "light" | "dark";

export interface ToastAction {
  label: string;
  run: () => void;
  /** The meeting the action opens (the toast goes when that meeting is deleted). */
  meetingId?: MeetingId;
}

export interface Toast {
  id: number;
  kind: "info" | "success" | "error";
  message: string;
  action?: ToastAction;
}

export interface Job {
  id: string;
  /**
   * An import, the post-pass of a live recording, or Add or Remove
   * punctuation (for `punctuate`, `doneMs` and `totalMs` count meetings).
   */
  kind: "import" | "postpass" | "punctuate";
  meetingId: MeetingId | null;
  name: string;
  stage: JobStage;
  doneMs: number;
  totalMs: number | null;
  startedAt: number;
  cancelling: boolean;
}

/** A request to open a meeting scrolled to one segment (search hits). */
export interface FocusRequest {
  meetingId: MeetingId;
  segmentId: SegmentId;
  startMs: number;
  nonce: number;
}

export interface ConfirmRequest {
  title: string;
  message: string;
  confirmLabel: string;
  danger: boolean;
  resolve: (ok: boolean) => void;
}

export const AUDIO_EXTENSIONS = [
  "wav", "mp3", "m4a", "aac", "flac", "ogg", "opus", "wma",
  "mp4", "m4v", "mkv", "mov", "webm", "mka",
];

const TOAST_MS = { info: 4000, success: 5000, error: 9000 } as const;

function baseName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

class AppState {
  route = $state<Route>("library");
  meetings = $state.raw<MeetingSummary[]>([]);
  meetingsLoaded = $state(false);
  meetingsError = $state<string | null>(null);
  openId = $state<MeetingId | null>(null);
  focus = $state.raw<FocusRequest | null>(null);
  settings = $state<Settings>({});
  theme = $state<Theme>("system");
  toasts = $state<Toast[]>([]);
  jobs = $state<Job[]>([]);
  confirmRequest = $state.raw<ConfirmRequest | null>(null);
  helpOpen = $state(false);
  /** A Settings field to scroll to when Settings opens (`set-<name>`), from a checklist fix. */
  settingsAnchor = $state<string | null>(null);
  dragging = $state(false);
  /** Post-passes queued or running (from `live:state`). */
  postPasses = $state.raw<PostPassView[]>([]);
  /** Why a meeting's post-pass or import failed (this session only). */
  postPassErrors = $state.raw(new Map<MeetingId, string>());
  /** No engine, speech model or diarizer (`setup_status`); null until known. */
  setupNeeded = $state<boolean | null>(null);
  /** What launch recovered (`startup_report`, `app:startup`). */
  startup = $state.raw<StartupReport | null>(null);
  /** What Import or Record still needs (the inline panel), with the files waiting to import. */
  readiness = $state.raw<{ action: string; missing: MissingPart[]; pending: string[] } | null>(null);

  #toastId = 0;
  #nonce = 0;
  #jobListeners = new Set<(e: JobEvent) => void>();
  #deleteListeners = new Set<(id: MeetingId) => void>();
  /** Resolves once the first meeting list and settings are in. */
  ready: Promise<void> = Promise.resolve();

  /** Starts the first fetches; called from main.ts before mounting. */
  init(): void {
    const meetings = api
      .listMeetings()
      .then((list) => {
        this.meetings = list;
      })
      .catch((e: unknown) => {
        this.meetingsError = errorText(e);
      })
      .finally(() => {
        this.meetingsLoaded = true;
      });
    const settings = api
      .getSettings()
      .then((s) => {
        this.settings = s;
        this.applyTheme((s.theme as Theme | undefined) ?? "system");
      })
      .catch(() => {});
    this.ready = Promise.all([meetings, settings]).then(() => {});
    void onJobEvent((e) => this.#onJob(e));
    void onEvent<PunctProgress>(PUNCT_PROGRESS_EVENT, (p) => this.#onPunctProgress(p));
    // After the first paint data: the check scans the models folder.
    void this.ready.then(() => this.#checkSetup());
    void onEvent<StartupReport>("app:startup", (r) => this.#onStartup(r));
    void api
      .startupReport()
      .then((r) => {
        if (r.recovery_done) this.#onStartup(r);
        else this.startup = r;
      })
      .catch(() => {});
  }

  /** Opens the first-run screen when there is nothing to transcribe with. */
  async #checkSetup(): Promise<void> {
    try {
      const s = await api.setupStatus();
      this.setupNeeded = s.needs_setup;
      // The data move from before the rename is being offered (an empty store).
      if (this.startup?.legacy_root) return;
      // The first run opens on the setup screen once; after "Set up later" the
      // library opens and Import and Record say what they miss.
      if (s.needs_setup && !this.settingBool("setup.later") && this.route === "library" && this.openId === null) {
        this.route = "setup";
      }
    } catch (e) {
      void api.log("warn", `setup check failed: ${errorText(e)}`);
    }
  }

  #startupShown = false;

  /** Tells the user once what launch recovered or finished. */
  #onStartup(r: StartupReport): void {
    this.startup = r;
    if (this.#startupShown) return;
    this.#startupShown = true;
    const details = { label: "Details", run: () => (this.route = "diagnostics") };
    // A meeting recovery left failed as it was: its banner says why.
    for (const x of r.recovered) if (x.error) this.#noteFailure(x.meeting_id, x.error);
    if (r.recovered.length > 0) {
      const what = r.recovered.map((x) => `"${x.title || "Untitled"}"`).join(", ");
      this.toast("info", `Recovered after an unexpected exit: ${what}`, details);
      void this.refreshMeetings();
    }
    if (r.recovery_error) this.toast("error", `Crash recovery could not run: ${r.recovery_error}`, details);
    if (r.moved_from) {
      this.toast(
        "success",
        r.move_kept.length === 0
          ? `Data folder moved. The old copy in ${r.moved_from} was removed.`
          : `Data folder moved. Some items stayed in ${r.moved_from}.`,
        details,
      );
    }
  }

  async refreshMeetings(): Promise<void> {
    try {
      this.meetings = await api.listMeetings();
      this.meetingsError = null;
    } catch (e) {
      this.meetingsError = errorText(e);
    }
  }

  /** Patches one row locally (title, pin) without refetching 500 rows. */
  patchMeeting(id: MeetingId, patch: Partial<MeetingSummary>): void {
    this.meetings = this.meetings.map((m) => (m.id === id ? { ...m, ...patch } : m));
  }

  open(id: MeetingId, at?: { segmentId: SegmentId; startMs: number }): void {
    this.route = "library";
    this.openId = id;
    this.focus = at ? { meetingId: id, ...at, nonce: ++this.#nonce } : null;
  }

  /** Asks, deletes the meeting, and forgets what the window kept for it. */
  async deleteMeeting(m: { id: MeetingId; title: string }): Promise<void> {
    const name = m.title || "Untitled meeting";
    const ok = await this.confirm({
      title: "Delete meeting?",
      message: `"${name}" and its transcript will be deleted. Audio files are removed too.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!ok) return;
    try {
      await api.deleteMeeting(m.id);
    } catch (e) {
      // The row can be gone with its audio folder left behind.
      await this.refreshMeetings();
      const gone = !this.meetings.some((x) => x.id === m.id);
      if (gone) this.forgetMeeting(m.id);
      this.error(
        gone
          ? `Deleted "${name}", but not all of its audio (${errorText(e)}); close any program using it`
          : `Could not delete "${name}": ${errorText(e)}`,
      );
      return;
    }
    this.forgetMeeting(m.id);
    await this.refreshMeetings();
    this.toast("info", `Deleted "${name}"`);
  }

  /**
   * Drops every meeting id the window kept for a deleted meeting (open
   * meeting, search focus, post-pass errors, toasts that open it, the live
   * view's last recording), so a later meeting with the same id starts clean.
   */
  forgetMeeting(id: MeetingId): void {
    const next = forgetMeeting(
      { openId: this.openId, focus: this.focus, postPassErrors: this.postPassErrors, toasts: this.toasts },
      id,
    );
    if (next.openId !== this.openId) this.openId = next.openId;
    if (next.focus !== this.focus) this.focus = next.focus;
    if (next.postPassErrors !== this.postPassErrors) this.postPassErrors = new Map(next.postPassErrors);
    if (next.toasts !== this.toasts) this.toasts = [...next.toasts];
    for (const cb of this.#deleteListeners) cb(id);
  }

  /** Called with each deleted meeting's id, after the app's own state is cleared. */
  onMeetingDeleted(cb: (id: MeetingId) => void): () => void {
    this.#deleteListeners.add(cb);
    return () => this.#deleteListeners.delete(cb);
  }

  // ---- Toasts and dialogs --------------------------------------------------

  toast(kind: Toast["kind"], message: string, action?: ToastAction): void {
    const id = ++this.#toastId;
    this.toasts = [...this.toasts.slice(-3), { id, kind, message, action }];
    setTimeout(() => this.dismiss(id), TOAST_MS[kind]);
  }

  dismiss(id: number): void {
    this.toasts = this.toasts.filter((t) => t.id !== id);
  }

  #cpuNoticeShown = false;

  /**
   * `CPU_NOTICE` the first time per session an import or a recording
   * starts on the CPU runtime package; null afterwards and on a GPU package.
   */
  async cpuNotice(): Promise<string | null> {
    if (this.#cpuNoticeShown) return null;
    try {
      const [files, settings] = await Promise.all([api.listModelFiles(), api.getSettings()]);
      if (!isCpuPackage(selectedEngine(files, settings))) return null;
    } catch {
      return null;
    }
    this.#cpuNoticeShown = true;
    return CPU_NOTICE;
  }

  /** Error toast with a "Show log" action that opens Diagnostics. */
  error(message: string): void {
    void api.log("error", message);
    this.toast("error", message, { label: "Show log", run: () => (this.route = "diagnostics") });
  }

  confirm(opts: Omit<ConfirmRequest, "resolve">): Promise<boolean> {
    return new Promise((resolve) => {
      this.confirmRequest = {
        ...opts,
        resolve: (ok) => {
          this.confirmRequest = null;
          resolve(ok);
        },
      };
    });
  }

  // ---- Settings and theme --------------------------------------------------

  /** A setting's value: stored, else its default from `diarist_core::settings::DEFAULTS`. */
  setting(key: SettingKey): string {
    return this.settings[key] ?? SETTING_DEFAULTS[key];
  }

  /** A boolean setting, read like `settings::get_bool` (unparseable values take the default). */
  settingBool(key: SettingKey): boolean {
    const parse = (v: string): boolean | null => {
      const s = v.trim().toLowerCase();
      if (["1", "true", "yes", "on"].includes(s)) return true;
      if (["0", "false", "no", "off"].includes(s)) return false;
      return null;
    };
    return parse(this.setting(key)) ?? parse(SETTING_DEFAULTS[key]) ?? false;
  }

  async setSetting(key: string, value: string): Promise<boolean> {
    try {
      await api.setSetting(key, value);
      this.settings[key] = value;
      if (key === "theme") this.applyTheme(value as Theme);
      return true;
    } catch (e) {
      this.error(`Could not save ${key} (${errorText(e)}); the change was not saved, try again`);
      return false;
    }
  }

  applyTheme(theme: Theme): void {
    this.theme = theme;
    const root = document.documentElement;
    if (theme === "system") delete root.dataset.theme;
    else root.dataset.theme = theme;
    getCurrentWindow()
      .setTheme(theme === "system" ? null : theme)
      .catch(() => {});
  }

  /** Every job event after the app's own bookkeeping (for open views). */
  onJob(cb: (e: JobEvent) => void): () => void {
    this.#jobListeners.add(cb);
    return () => this.#jobListeners.delete(cb);
  }

  /** The post-pass of a meeting, if one is queued or running. */
  postPassOf(id: MeetingId): PostPassView | null {
    return this.postPasses.find((p) => p.meeting_id === id) ?? null;
  }

  setPostPasses(list: PostPassView[]): void {
    this.postPasses = list;
    for (const p of list) {
      const job = this.jobs.find((j) => j.id === p.job_id);
      const name = p.title || "Recording";
      if (job) Object.assign(job, { kind: "postpass", meetingId: p.meeting_id, name });
    }
  }

  /** Process-again jobs (plan 16 section 12.5 J) and the audio each reads, until they end. */
  #againJobs = new Map<string, RerunAudio>();

  /**
   * Registers a post-pass the app just queued (Stop, Retry; `again`: "Process again" from that
   * audio, whose end is told in its own words, once, open meeting or not).
   */
  trackPostPass(jobId: string, meetingId: MeetingId, title: string, again?: RerunAudio): void {
    if (again) this.#againJobs.set(jobId, again);
    this.#upsertJob(jobId, { kind: "postpass", meetingId, name: title || "Recording" });
    if (this.postPassErrors.has(meetingId)) {
      const next = new Map(this.postPassErrors);
      next.delete(meetingId);
      this.postPassErrors = next;
    }
  }

  // ---- Import jobs ---------------------------------------------------------

  /** Imports `paths` in `language`; without one, the import sheet asks for it first. */
  async importPaths(paths: string[], language?: string): Promise<void> {
    if (language === undefined) {
      importSheet.show(paths);
      return;
    }
    // Checked at click time: what is missing shows inline with its fix.
    try {
      const r = await langApi.checkReady("import", language);
      if (!r.ok) {
        this.readiness = { action: "import", missing: r.missing, pending: paths };
        return;
      }
    } catch {
      // The import itself reports what went wrong.
    }
    for (const path of paths) {
      try {
        const id = await langApi.importFile(path, language);
        this.#upsertJob(id, { name: `${baseName(path)} · ${languageName(language)}` });
      } catch (e) {
        this.error(`Could not import ${baseName(path)} (${errorText(e)}); check that it plays in a media player, then drop it again`);
      }
    }
  }

  async cancelJob(id: string): Promise<void> {
    this.#upsertJob(id, { cancelling: true });
    try {
      await api.cancelJob(id);
    } catch (e) {
      this.jobs = this.jobs.filter((j) => j.id !== id);
      this.toast("info", errorText(e));
    }
  }

  #upsertJob(id: string, patch: Partial<Job>): void {
    const existing = this.jobs.find((j) => j.id === id);
    if (existing) {
      Object.assign(existing, patch);
    } else {
      const pp = this.postPasses.find((p) => p.job_id === id);
      this.jobs.push({
        id,
        kind: pp ? "postpass" : "import",
        meetingId: pp?.meeting_id ?? null,
        name: pp ? pp.title || "Recording" : "Importing…",
        stage: "queued",
        doneMs: 0,
        totalMs: null,
        startedAt: Date.now(),
        cancelling: false,
        ...patch,
      });
    }
  }

  #onJob(e: JobEvent): void {
    this.#handleJob(e);
    for (const cb of this.#jobListeners) cb(e);
  }

  #handleJob(e: JobEvent): void {
    const job = this.jobs.find((j) => j.id === e.job_id);
    // Punctuation jobs report through their command (`punctuate`, `backfill`).
    if (job?.kind === "punctuate") return;
    const name = job && job.name !== "Importing…" ? job.name : "file";
    if (e.type === "progress") {
      const first = !job || job.stage === "queued";
      this.#upsertJob(e.job_id, { stage: e.stage, doneMs: e.done_ms, totalMs: e.total_ms });
      // The meeting row exists (status processing) from the first event on.
      if (first) void this.refreshMeetings();
      return;
    }
    this.jobs = this.jobs.filter((j) => j.id !== e.job_id);
    const pp = this.postPasses.find((p) => p.job_id === e.job_id);
    const again = this.#againJobs.get(e.job_id) ?? null;
    this.#againJobs.delete(e.job_id);
    if (job?.kind === "postpass" || pp || again) {
      this.#postPassEnded(e, job?.meetingId ?? pp?.meeting_id ?? null, pp?.title || job?.name || "Recording", again);
      return;
    }
    if (e.type === "done") {
      const meetingId = e.meeting_id;
      void this.refreshMeetings().then(() => {
        const title = this.meetings.find((m) => m.id === meetingId)?.title || name;
        this.toast("success", `Imported "${title}"`, { label: "Open", run: () => this.open(meetingId), meetingId });
      });
      return;
    }
    void this.refreshMeetings();
    if (e.type === "cancelled") {
      this.toast("info", `Import of ${name} cancelled`);
    } else if (e.message === UNKNOWN_TOKENS_NOTICE && e.meeting_id !== null) {
      // The meeting stays (failed, with its audio); its page says why.
      const id = e.meeting_id;
      this.#noteFailure(id, e.message);
      void api.log("warn", `import of ${name}: ${e.message}`);
      this.toast("error", `${name}: ${e.message}`, { label: "Open", run: () => this.open(id), meetingId: id });
    } else {
      if (e.meeting_id !== null) this.#noteFailure(e.meeting_id, e.message);
      this.error(`Import of ${name} failed: ${e.message}`);
    }
  }

  /** Keeps why a job for meeting `id` failed, for the meeting's banner (this session). */
  #noteFailure(id: MeetingId, message: string): void {
    const next = new Map(this.postPassErrors);
    next.set(id, message);
    this.postPassErrors = next;
  }

  /**
   * How a post-pass ended, told once. `again`: a "Process again" job and the audio it read; from
   * the FLAC the meeting stays done with its current transcript whatever happens (the new run
   * becomes current only once nothing can fail), so it is said even with the meeting open.
   */
  #postPassEnded(e: JobEvent, meetingId: MeetingId | null, title: string, again: RerunAudio | null = null): void {
    void this.refreshMeetings();
    const id = e.type === "done" ? e.meeting_id : (meetingId ?? ("meeting_id" in e ? e.meeting_id : null));
    if (e.type === "error" && id !== null) this.#noteFailure(id, e.message);
    const open = id !== null && this.openId === id && this.route === "library";
    const openAction = !open && id !== null ? { label: "Open", run: () => this.open(id), meetingId: id } : undefined;
    if (again && e.type !== "progress") {
      if (e.type === "error") void api.log("error", `process again of ${title} failed: ${e.message}`);
      const t = againEndText(e.type, title, again, e.type === "error" ? e.message : "", open);
      if (t) this.toast(t.kind, t.text, openAction);
      return;
    }
    if (e.type === "done") {
      if (!open && id !== null) {
        this.toast("success", `"${title}" is final`, { label: "Open", run: () => this.open(id), meetingId: id });
      }
    } else if (e.type === "cancelled") {
      this.toast("info", `Post-pass of "${title}" cancelled; the live transcript stays`);
    } else if (e.type === "error" && e.message === UNKNOWN_TOKENS_NOTICE) {
      // The meeting's banner says it too, with Retry (the WAVs stay).
      void api.log("warn", `post-pass of ${title}: ${e.message}`);
      if (!open) {
        this.toast("error", `"${title}": ${e.message} The live transcript stays.`, {
          label: id !== null ? "Open" : "Show log",
          run: () => (id !== null ? this.open(id) : (this.route = "diagnostics")),
          meetingId: id ?? undefined,
        });
      }
    } else if (e.type === "error") {
      void api.log("error", `post-pass of ${title} failed: ${e.message}`);
      if (!open) {
        this.toast("error", `Finalizing "${title}" failed; the live transcript stays`, {
          label: id !== null ? "Open" : "Show log",
          run: () => (id !== null ? this.open(id) : (this.route = "diagnostics")),
          meetingId: id ?? undefined,
        });
      }
    }
  }

  // ---- Punctuation (plan 16 section 3.9) ----------------------------------------

  /** Meetings with Add or Remove punctuation queued or running. */
  punctuating = $state.raw(new Set<MeetingId>());
  /** The bulk action is queued or running. */
  backfillRunning = $state(false);
  #punctListeners = new Set<(id: MeetingId) => void>();

  /** Called with a meeting's id when a punctuation job on it ended (its current run may be new). */
  onPunctuated(cb: (id: MeetingId) => void): () => void {
    this.#punctListeners.add(cb);
    return () => this.#punctListeners.delete(cb);
  }

  #setPunctuating(id: MeetingId, on: boolean): void {
    const next = new Set(this.punctuating);
    if (on) next.add(id);
    else next.delete(id);
    this.punctuating = next;
  }

  #onPunctProgress(p: PunctProgress): void {
    const job = this.jobs.find((j) => j.id === p.job_id);
    if (job) Object.assign(job, { stage: "saving", doneMs: p.done, totalMs: p.total });
  }

  /** A job id the backend accepts (any unique string). */
  #newJobId(): string {
    const c = globalThis.crypto as Crypto | undefined;
    return `punct-${c?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(36).slice(2)}`}`;
  }

  /**
   * "Add punctuation" (`remove`: "Remove punctuation") on one meeting, as a
   * job in the job list; says what it did when it is done, and offers Try
   * again when it could not run (the meeting records, it changed meanwhile).
   */
  async punctuate(meetingId: MeetingId, title: string, remove: boolean): Promise<PunctOutcome | null> {
    if (this.punctuating.has(meetingId)) return null;
    const id = this.#newJobId();
    this.#upsertJob(id, { kind: "punctuate", meetingId, name: punctJobName(remove, title) });
    this.#setPunctuating(meetingId, true);
    try {
      const o = await punctApi.punctuateMeeting(meetingId, remove, id);
      const files = await api.listModelFiles().catch(() => []);
      const t = punctOutcomeText(o, remove, files);
      const elsewhere = this.openId !== meetingId || this.route !== "library";
      const action = t.retry
        ? { label: TRY_AGAIN, run: () => void this.punctuate(meetingId, title, remove), meetingId }
        : elsewhere
          ? { label: "Open", run: () => this.open(meetingId), meetingId }
          : undefined;
      this.toast(t.kind, elsewhere ? `"${title || "Untitled meeting"}": ${t.text}` : t.text, action);
      return o;
    } catch (e) {
      const msg = errorText(e);
      void api.log("warn", `${remove ? "remove" : "add"} punctuation on meeting ${meetingId}: ${msg}`);
      this.toast("error", punctErrorText(remove, msg), {
        label: TRY_AGAIN,
        run: () => void this.punctuate(meetingId, title, remove),
        meetingId,
      });
      return null;
    } finally {
      this.jobs = this.jobs.filter((j) => j.id !== id);
      this.#setPunctuating(meetingId, false);
      for (const cb of this.#punctListeners) cb(meetingId);
    }
  }

  /** The bulk action, "Add punctuation to N older meetings" (confirmed by the caller); reports when done. */
  async backfill(): Promise<BackfillOutcome | null> {
    if (this.backfillRunning) return null;
    const id = this.#newJobId();
    this.backfillRunning = true;
    this.#upsertJob(id, { kind: "punctuate", meetingId: null, name: BACKFILL_JOB_NAME });
    try {
      const o = await punctApi.backfill(id);
      const t = backfillDoneText(o);
      this.toast(t.kind, t.text);
      for (const x of o.outcomes) if (x.run !== null) for (const cb of this.#punctListeners) cb(x.meeting_id);
      return o;
    } catch (e) {
      const msg = errorText(e);
      void api.log("warn", `punctuation for older meetings: ${msg}`);
      this.toast("error", `Adding punctuation to older meetings did not work: ${msg.replace(/\.$/, "")}.`, {
        label: TRY_AGAIN,
        run: () => void this.backfill(),
      });
      return null;
    } finally {
      this.jobs = this.jobs.filter((j) => j.id !== id);
      this.backfillRunning = false;
    }
  }
}

export const app = new AppState();

/** True when a key event should be left to a text field. */
export function isTyping(e: KeyboardEvent): boolean {
  const t = e.target as HTMLElement | null;
  if (!t) return false;
  if (t.isContentEditable) return true;
  if (t instanceof HTMLTextAreaElement || t instanceof HTMLSelectElement) return true;
  if (t instanceof HTMLInputElement) {
    return !["checkbox", "radio", "range", "button", "submit"].includes(t.type);
  }
  return false;
}
