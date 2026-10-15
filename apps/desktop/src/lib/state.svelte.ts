// App-wide state: route, meeting list, open meeting, jobs, toasts, theme.
// Svelte 5 runes in a module; components import the `app` singleton.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, errorText, onJobEvent } from "./api";
import type { JobEvent } from "./generated/JobEvent";
import type { JobStage } from "./generated/JobStage";
import type { MeetingId } from "./generated/MeetingId";
import type { MeetingSummary } from "./generated/MeetingSummary";
import type { SegmentId } from "./generated/SegmentId";
import type { Settings } from "./generated/Settings";

export type Route = "library" | "settings" | "diagnostics";
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

  #toastId = 0;
  #nonce = 0;
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
      this.jobs.push({
        id,
        name: "Importing…",
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
    const job = this.jobs.find((j) => j.id === e.job_id);
    const name = job?.name ?? "file";
    if (e.type === "progress") {
      const first = !job || job.stage === "queued";
      this.#upsertJob(e.job_id, { stage: e.stage, doneMs: e.done_ms, totalMs: e.total_ms });
      // The meeting row exists (status processing) from the first event on.
      if (first) void this.refreshMeetings();
      return;
    }
    this.jobs = this.jobs.filter((j) => j.id !== e.job_id);
    void this.refreshMeetings();
    if (e.type === "done") {
      const meetingId = e.meeting_id;
      this.toast("success", `Imported ${name}`, { label: "Open", run: () => this.open(meetingId) });
    } else if (e.message === "Import cancelled") {
      this.toast("info", `Import of ${name} cancelled`);
    } else {
      this.error(`Import of ${name} failed: ${e.message}`);
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
