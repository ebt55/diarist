// Live recording state: the `live:*` events folded into what the record
// controls and the live meeting view render. One recording at a time.
import { api, errorText, onEvent } from "./api";
import type { Channel } from "./generated/Channel";
import type { DegradedUpdate } from "./generated/DegradedUpdate";
import type { DeviceChange } from "./generated/DeviceChange";
import type { LevelsUpdate } from "./generated/LevelsUpdate";
import type { LiveLevels } from "./generated/LiveLevels";
import type { LivePhase } from "./generated/LivePhase";
import type { LiveStateView } from "./generated/LiveStateView";
import type { LiveWarning } from "./generated/LiveWarning";
import type { MeetingId } from "./generated/MeetingId";
import type { SpeakerId } from "./generated/SpeakerId";
import type { StartRequest } from "./generated/StartRequest";
import type { TextFinal } from "./generated/TextFinal";
import type { Toggled } from "./generated/Toggled";
import type { WarningNote } from "./generated/WarningNote";
import type { WordsUpdate } from "./generated/WordsUpdate";
import { ME_COLOR, SPEAKER_PALETTE } from "./generated/palette";
import { OpenOnce } from "./openOnce";
import { armedNote, stopDialog, stopPress } from "./stopConfirm";
import { app, type ConfirmRequest } from "./state.svelte";

/** Text for a warning banner. */
export function warningText(w: LiveWarning): string {
  switch (w.kind) {
    case "loopback_silent":
      return `No system audio for ${w.seconds} s while the microphone hears speech. Is the right output selected?`;
    case "loopback_quiet":
      return `System audio from ${w.device || "the output"} is very quiet (peaks ${Math.round(w.peak_dbfs)} dBFS); raise its Windows volume or pick another output.`;
    case "capture":
      return `${w.channel === "mic" ? "Microphone" : "System audio"}: ${w.message}`;
    case "echo":
      return w.aec
        ? "Sound from the meeting reaches your microphone. Echo cancellation is on."
        : `Sound from the meeting reaches your microphone. Echoed words are removed from "me"; headphones would help.`;
    default:
      return JSON.stringify(w);
  }
}

/** `diarist_core::views::default_speaker_name` for labels without a speaker row. */
export function defaultSpeakerName(label: string): string {
  if (label === "me") return "Me";
  if (label === "spk_unknown") return "Unknown speaker";
  const spk = /^spk_(\d+)$/.exec(label);
  if (spk) return `Speaker ${Number(spk[1]) + 1}`;
  const user = /^user_(\d+)$/.exec(label);
  if (user) return `Speaker ${Number(user[1])}`;
  return label;
}

/** `diarist_core::views::default_speaker_color`. */
export function defaultSpeakerColor(label: string): string {
  if (label === "me") return ME_COLOR;
  const spk = /^spk_(\d+)$/.exec(label);
  let index: number;
  if (spk) index = Number(spk[1]);
  else {
    let h = 0x811c9dc5;
    for (const b of new TextEncoder().encode(label)) h = Math.imul(h ^ b, 0x01000193) >>> 0;
    index = h;
  }
  return SPEAKER_PALETTE[index % SPEAKER_PALETTE.length]!;
}

/** One transcript row: a run of same-label words of one utterance. */
export interface LiveRow {
  key: string;
  channel: Channel;
  utt: number;
  start_ms: number;
  end_ms: number;
  label: string;
  speakerId: SpeakerId | null;
  text: string;
}

export interface Meter {
  rms: number;
  peak: number;
  /** Automatic gain applied (0 without AGC). */
  gain: number;
  /** Peak before the automatic gain. */
  rawPeak: number;
  /** Date.now() of the reading. */
  at: number;
}

function meter(l: LevelsUpdate, at: number): Meter {
  return { rms: l.rms_dbfs, peak: l.peak_dbfs, gain: l.gain_db, rawPeak: l.raw_peak_dbfs, at };
}

/** Text older than this counts as final without a `text_final` mark. */
export const FINAL_AFTER_MS = 10_000;

function splitRows(u: WordsUpdate): LiveRow[] {
  const rows: LiveRow[] = [];
  let i = 0;
  while (i < u.words.length) {
    const first = u.words[i]!;
    let j = i + 1;
    while (j < u.words.length && u.words[j]!.label === first.label) j++;
    const run = u.words.slice(i, j);
    rows.push({
      key: `${u.channel}:${u.utt}:${i}`,
      channel: u.channel,
      utt: u.utt,
      start_ms: first.start_ms,
      end_ms: run[run.length - 1]!.end_ms,
      label: first.label,
      speakerId: first.speaker_id,
      text: run.map((w) => w.text).join(" "),
    });
    i = j;
  }
  return rows;
}

interface Lag {
  n: number;
  sum: number;
  max: number;
  first: number | null;
}

class LiveStore {
  phase = $state<LivePhase>("idle");
  /** The current recording, or the last one after Stop. */
  meetingId = $state<MeetingId | null>(null);
  title = $state<string | null>(null);
  /** Unix ms of meeting time zero. */
  startedAt = $state<number | null>(null);
  devices = $state.raw<Record<string, DeviceChange>>({});
  /** The latest endpoint switch after the first open (shown as a chip). */
  deviceNotice = $state.raw<DeviceChange | null>(null);
  degraded = $state.raw<Record<string, DegradedUpdate>>({});
  warnings = $state.raw<WarningNote[]>([]);
  /** Warnings before this index are dismissed. */
  warningsSeen = $state(0);
  error = $state<string | null>(null);
  /** The GPU queue holds new jobs while a recording runs (`JobQueue::is_paused`). */
  queuePaused = $state(false);
  mic = $state.raw<Meter | null>(null);
  loopback = $state.raw<Meter | null>(null);
  rows = $state.raw<LiveRow[]>([]);
  finalUntil = $state.raw<Record<string, number>>({ mic: -1, loopback: -1 });
  now = $state(Date.now());
  sheetOpen = $state(false);

  #utts = new Map<string, LiveRow[]>();
  #rebuild: ReturnType<typeof setTimeout> | undefined;
  #ticker: ReturnType<typeof setInterval> | undefined;
  #noticeTimer: ReturnType<typeof setTimeout> | undefined;
  #opener = new OpenOnce();
  /** The open "Stop recording?" question, if any. */
  #asking: ConfirmRequest | null = null;
  #lag: Record<string, Lag> = {};

  get active(): boolean {
    return this.phase === "recording" || this.phase === "stopping";
  }

  get elapsedMs(): number {
    return this.startedAt === null ? 0 : Math.max(0, this.now - this.startedAt);
  }

  /** Whether a row's text and label are past the final mark (or old enough). */
  isFinal(row: LiveRow): boolean {
    return row.end_ms <= (this.finalUntil[row.channel] ?? -1) || row.end_ms <= this.elapsedMs - FINAL_AFTER_MS;
  }

  init(): void {
    app.onMeetingDeleted((id) => this.#forget(id));
    void onEvent<LiveStateView>("live:state", (v) => this.#apply(v));
    void onEvent<WordsUpdate>("live:words", (u) => this.#onWords(u));
    void onEvent<TextFinal>("live:text_final", (t) => {
      const cur = this.finalUntil[t.channel] ?? -1;
      if (t.until_ms > cur) this.finalUntil = { ...this.finalUntil, [t.channel]: t.until_ms };
    });
    void onEvent<LiveLevels>("live:levels", (l) => {
      const at = Date.now();
      if (l.mic) this.mic = meter(l.mic, at);
      if (l.loopback) this.loopback = meter(l.loopback, at);
    });
    void onEvent<DegradedUpdate>("live:degraded", (d) => {
      const next = { ...this.degraded };
      if (d.degraded) next[d.channel] = d;
      else delete next[d.channel];
      this.degraded = next;
    });
    void onEvent<WarningNote>("live:warning", (w) => {
      this.warnings = [...this.warnings, w];
    });
    void onEvent<DeviceChange>("live:device_changed", (d) => {
      this.devices = { ...this.devices, [d.channel]: d };
      if (d.reason !== "initial") {
        this.deviceNotice = d;
        clearTimeout(this.#noticeTimer);
        this.#noticeTimer = setTimeout(() => (this.deviceNotice = null), 20_000);
      }
    });
    void onEvent<Toggled>("live:toggled", (t) => this.#onToggled(t));
    void api
      .recordingStatus()
      .then(async (s) => {
        this.#apply(s.state);
        if (s.state.meeting_id !== null && this.active) {
          const snap = await api.liveSnapshot(s.state.meeting_id);
          for (const u of snap) this.#onWords(u);
        }
      })
      .catch(() => {});
  }

  // ---- actions ---------------------------------------------------------------

  /** The Record / Stop button: open the start sheet, or stop (after confirming). */
  async toggle(): Promise<void> {
    if (this.phase === "idle") await this.#openSheet();
    else if (this.phase === "recording") await this.stop();
  }

  /** Opens the start sheet, or the panel naming what recording still needs. */
  async #openSheet(): Promise<void> {
    try {
      const r = await api.checkReady("record");
      if (!r.ok) {
        app.readiness = { action: "record", missing: r.missing, pending: [] };
        return;
      }
    } catch {
      // Start reports what went wrong.
    }
    this.sheetOpen = true;
  }

  /** The R key: only ever starts (opens the start sheet); a running recording is left alone. */
  record(): void {
    if (this.phase === "idle") void this.#openSheet();
    else if (this.phase === "recording") app.toast("info", "Already recording. Ctrl+. or the Stop button ends it.");
  }

  /** Starts a recording; resolves to an error message, or null. */
  async start(req: StartRequest): Promise<string | null> {
    try {
      const v = await api.startRecording(req);
      this.#apply(v);
      return null;
    } catch (e) {
      return errorText(e);
    }
  }

  /**
   * Ctrl+., the Stop buttons, and Ctrl+Alt+R or the tray with the window on
   * screen: always asks first (Enter or a second Ctrl+Alt+R confirms, Esc
   * keeps recording).
   */
  async stop(): Promise<void> {
    if (this.phase !== "recording" || this.#asking) return;
    const answer = app.confirm({ ...stopDialog(this.elapsedMs), danger: false });
    this.#asking = app.confirmRequest;
    let ok: boolean;
    try {
      ok = await answer;
    } finally {
      this.#asking = null;
    }
    if (!ok || this.phase !== "recording") return;
    this.#logLag();
    try {
      const r = await api.stopRecording();
      app.trackPostPass(r.job_id, r.meeting_id, this.title ?? "");
      app.toast(
        "info",
        r.transcribe
          ? "Recording stopped. Finalizing the transcript…"
          : "Recording stopped. No issues, so the live transcript is kept.",
      );
    } catch (e) {
      app.error(`Could not stop the recording (${errorText(e)}); press Ctrl+. again. The audio is safe: an interrupted recording is recovered at the next start`);
    }
  }

  dismissWarnings(): void {
    this.warningsSeen = this.warnings.length;
  }

  // ---- events ----------------------------------------------------------------

  #reset(): void {
    this.#utts.clear();
    this.rows = [];
    this.finalUntil = { mic: -1, loopback: -1 };
    this.devices = {};
    this.deviceNotice = null;
    this.degraded = {};
    this.warnings = [];
    this.warningsSeen = 0;
    this.mic = null;
    this.loopback = null;
    this.#lag = {};
  }

  /** A deleted meeting was the last recording: its rows and id go. */
  #forget(id: MeetingId): void {
    if (this.active || this.meetingId !== id) return;
    this.meetingId = null;
    this.title = null;
    this.startedAt = null;
    this.#reset();
  }

  #apply(v: LiveStateView): void {
    const prev = this.phase;
    if (v.phase === "starting" && prev !== "starting") this.#reset();
    this.phase = v.phase;
    this.error = v.error;
    this.queuePaused = v.queue_paused;
    if (v.meeting_id !== null) {
      if (v.meeting_id !== this.meetingId && prev !== "starting") this.#reset();
      this.meetingId = v.meeting_id;
      this.title = v.title;
      this.startedAt = v.started_at;
      this.devices = { ...v.devices, ...this.devices };
      this.degraded = v.degraded;
      if (v.warnings.length > this.warnings.length) this.warnings = v.warnings;
    }
    app.setPostPasses(v.post_passes);
    if (this.active) {
      if (!this.#ticker) this.#ticker = setInterval(() => (this.now = Date.now()), 250);
      this.now = Date.now();
    } else if (this.#ticker) {
      clearInterval(this.#ticker);
      this.#ticker = undefined;
    }
    const open = this.#opener.update(v.phase, v.meeting_id);
    if (open !== null) {
      app.open(open);
      void app.refreshMeetings();
    }
    if (prev === "stopping" && v.phase === "idle") void app.refreshMeetings();
  }

  #onWords(u: WordsUpdate): void {
    if (this.meetingId !== null && u.meeting_id !== this.meetingId) return;
    this.#utts.set(`${u.channel}:${u.utt}`, splitRows(u));
    const last = u.words[u.words.length - 1];
    if (last && this.startedAt !== null) {
      const lag = Date.now() - this.startedAt - last.end_ms;
      const l = (this.#lag[u.channel] ??= { n: 0, sum: 0, max: 0, first: null });
      l.n++;
      l.sum += lag;
      l.max = Math.max(l.max, lag);
      l.first ??= Date.now() - this.startedAt;
    }
    if (this.#rebuild === undefined) {
      this.#rebuild = setTimeout(() => {
        this.#rebuild = undefined;
        const all: LiveRow[] = [];
        for (const rows of this.#utts.values()) all.push(...rows);
        all.sort((a, b) => a.start_ms - b.start_ms || (a.channel === "mic" ? -1 : 1));
        this.rows = all;
      }, 50);
    }
  }

  #logLag(): void {
    const parts = Object.entries(this.#lag).map(
      ([ch, l]) =>
        `${ch}: first words at ${l.first ?? "?"} ms, lag avg ${Math.round(l.sum / Math.max(1, l.n))} ms max ${Math.round(l.max)} ms over ${l.n} updates`,
    );
    if (parts.length > 0) void api.log("info", `live latency (UI): ${parts.join("; ")}`);
  }

  #onToggled(t: Toggled): void {
    const via = t.via === "shortcut" ? "Ctrl+Alt+R" : "the tray";
    if (t.action === "started") {
      const again = via === "Ctrl+Alt+R" ? "Press it" : "Choose Stop";
      void app
        .cpuNotice()
        .then((cpu) => app.toast("info", `Recording started from ${via}. ${again} again to stop (it asks first).${cpu ? ` ${cpu}` : ""}`));
    } else if (t.action === "confirm_stop") {
      // The window is on screen: the usual "Stop recording?" dialog, or,
      // when it is already open, the second press answers it.
      const asking = this.#asking;
      if (stopPress(asking !== null && app.confirmRequest === asking) === "confirm") asking?.resolve(true);
      else void this.stop();
    } else if (t.action === "stop_armed") {
      app.toast("info", armedNote(t.via));
    } else if (t.action === "stopped") {
      app.toast("info", `Recording stopped from ${via}. Finalizing the transcript…`);
    } else {
      app.error(`Recording (${via}) did not start: ${t.message ?? "unknown error"}. The panel or Diagnostics says what to fix`);
      void api
        .checkReady("record")
        .then((r) => {
          if (!r.ok) app.readiness = { action: "record", missing: r.missing, pending: [] };
        })
        .catch(() => {});
    }
  }
}

export const live = new LiveStore();
