// App-wide state: route, meeting list, open meeting, jobs, toasts, theme.
// Svelte 5 runes in a module; components import the `app` singleton.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, errorText, onEvent, onJobEvent } from "./api";
import type { JobEvent } from "./generated/JobEvent";
import type { JobStage } from "./generated/JobStage";
import type { MeetingId } from "./generated/MeetingId";
import type { MeetingSummary } from "./generated/MeetingSummary";
import type { PostPassView } from "./generated/PostPassView";
import type { SegmentId } from "./generated/SegmentId";
import type { Settings } from "./generated/Settings";
import type { StartupReport } from "./generated/StartupReport";
import { SETTING_DEFAULTS, type SettingKey } from "./generated/settingDefaults";

export type Route = "library" | "settings" | "diagnostics" | "setup";

/** Set by the backend before the page loads (`lib.rs`). */
export interface StartupFlags {
  /** Started by Windows at sign-in: the window stays in the tray. */
  autostart: boolean;
  /** A saved window size and position were restored. */
  windowStateSaved: boolean;
}

export function startupFlags(): StartupFlags {
  const f = (window as unknown as { __MEET_ASSIST_STARTUP__?: Partial<StartupFlags> }).__MEET_ASSIST_STARTUP__;
  return { autostart: f?.autostart ?? false, windowStateSaved: f?.windowStateSaved ?? false };
}
export type Theme = "system" | "light" | "dark";

export interface ToastAction {
  label: string;
  run: () => void;
}

export interface Toast {
  id: number;
  kind: "info" | "success" | "error";
  message: string;
  action?: ToastAction;
}

export interface Job {
  id: string;
  /** An import, or the post-pass of a live recording. */
  kind: "import" | "postpass";
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
  dragging = $state(false);
  /** Post-passes queued or running (from `live:state`). */
  postPasses = $state.raw<PostPassView[]>([]);
  /** Why a meeting's post-pass failed (this session only). */
  postPassErrors = $state.raw(new Map<MeetingId, string>());
  /** No engine, speech model or diarizer (`setup_status`); null until known. */
  setupNeeded = $state<boolean | null>(null);
  /** What launch recovered (`startup_report`, `app:startup`). */
  startup = $state.raw<StartupReport | null>(null);

  #toastId = 0;
  #nonce = 0;
  #jobListeners = new Set<(e: JobEvent) => void>();
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
      if (s.needs_setup && this.route === "library" && this.openId === null) this.route = "setup";
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

  // ---- Toasts and dialogs --------------------------------------------------

  toast(kind: Toast["kind"], message: string, action?: ToastAction): void {
    const id = ++this.#toastId;
    this.toasts = [...this.toasts.slice(-3), { id, kind, message, action }];
    setTimeout(() => this.dismiss(id), TOAST_MS[kind]);
  }

  dismiss(id: number): void {
    this.toasts = this.toasts.filter((t) => t.id !== id);
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

  /** A setting's value: stored, else its default from `meet_core::settings::DEFAULTS`. */
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
      this.error(`Could not save ${key}: ${errorText(e)}`);
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

  /** Registers a post-pass the app just queued (Stop, Retry). */
  trackPostPass(jobId: string, meetingId: MeetingId, title: string): void {
    this.#upsertJob(jobId, { kind: "postpass", meetingId, name: title || "Recording" });
    if (this.postPassErrors.has(meetingId)) {
      const next = new Map(this.postPassErrors);
      next.delete(meetingId);
      this.postPassErrors = next;
    }
  }

  // ---- Import jobs ---------------------------------------------------------

  async importPaths(paths: string[]): Promise<void> {
    for (const path of paths) {
      try {
        const id = await api.importFile(path);
        this.#upsertJob(id, { name: baseName(path) });
      } catch (e) {
        this.error(`Could not import ${baseName(path)}: ${errorText(e)}`);
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
    if (job?.kind === "postpass" || pp) {
      this.#postPassEnded(e, job?.meetingId ?? pp?.meeting_id ?? null, pp?.title || job?.name || "Recording");
      return;
    }
    if (e.type === "done") {
      const meetingId = e.meeting_id;
      void this.refreshMeetings().then(() => {
        const title = this.meetings.find((m) => m.id === meetingId)?.title || name;
        this.toast("success", `Imported "${title}"`, { label: "Open", run: () => this.open(meetingId) });
      });
      return;
    }
    void this.refreshMeetings();
    if (e.type === "cancelled") {
      this.toast("info", `Import of ${name} cancelled`);
    } else {
      this.error(`Import of ${name} failed: ${e.message}`);
    }
  }

  #postPassEnded(e: JobEvent, meetingId: MeetingId | null, title: string): void {
    void this.refreshMeetings();
    const id = e.type === "done" ? e.meeting_id : (meetingId ?? ("meeting_id" in e ? e.meeting_id : null));
    if (e.type === "error" && id !== null) {
      const next = new Map(this.postPassErrors);
      next.set(id, e.message);
      this.postPassErrors = next;
    }
    const open = id !== null && this.openId === id && this.route === "library";
    if (e.type === "done") {
      if (!open && id !== null) {
        this.toast("success", `"${title}" is final`, { label: "Open", run: () => this.open(id) });
      }
    } else if (e.type === "cancelled") {
      this.toast("info", `Post-pass of "${title}" cancelled; the live transcript stays`);
    } else if (e.type === "error") {
      void api.log("error", `post-pass of ${title} failed: ${e.message}`);
      if (!open) {
        this.toast("error", `Finalizing "${title}" failed; the live transcript stays`, {
          label: id !== null ? "Open" : "Show log",
          run: () => (id !== null ? this.open(id) : (this.route = "diagnostics")),
        });
      }
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
