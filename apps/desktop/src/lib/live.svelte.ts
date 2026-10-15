// Live recording state: the `live:*` events folded into what the record
// controls and the live meeting view render. One recording at a time.
import { api, errorText, onEvent } from "./api";
import type { Channel } from "./generated/Channel";
import type { MeetingId } from "./generated/MeetingId";
import type { SpeakerId } from "./generated/SpeakerId";
import type {
  DegradedUpdate,
  DeviceChange,
  LevelsPayload,
  LiveStateView,
  LivePhase,
  LiveWarning,
  StartRequest,
  TextFinal,
  Toggled,
  WordsUpdate,
} from "./live";
import { ME_COLOR, SPEAKER_PALETTE } from "./generated/palette";
import { app } from "./state.svelte";

/** `meet_core::views::default_speaker_name` for labels without a speaker row. */
export function defaultSpeakerName(label: string): string {
  if (label === "me") return "Me";
  if (label === "spk_unknown") return "Unknown speaker";
  const spk = /^spk_(\d+)$/.exec(label);
  if (spk) return `Speaker ${Number(spk[1]) + 1}`;
  const user = /^user_(\d+)$/.exec(label);
  if (user) return `Speaker ${Number(user[1])}`;
  return label;
}

/** `meet_core::views::default_speaker_color`. */
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
  gain: number | null;
  /** Date.now() of the reading. */
  at: number;
}

/** Stopping a recording younger than this asks first. */
export const STOP_CONFIRM_MS = 10_000;
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
  warnings = $state.raw<LiveWarning[]>([]);
  /** Warnings before this index are dismissed. */
  warningsSeen = $state(0);
  error = $state<string | null>(null);
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
  #openedFor: MeetingId | null = null;
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
    void onEvent<LiveStateView>("live:state", (v) => this.#apply(v));
    void onEvent<WordsUpdate>("live:words", (u) => this.#onWords(u));
    void onEvent<TextFinal>("live:text_final", (t) => {
      const cur = this.finalUntil[t.channel] ?? -1;
      if (t.until_ms > cur) this.finalUntil = { ...this.finalUntil, [t.channel]: t.until_ms };
    });
    void onEvent<LevelsPayload>("live:levels", (l) => {
      const at = Date.now();
      if (l.mic) this.mic = { rms: l.mic.rms_dbfs, peak: l.mic.peak_dbfs, gain: l.mic.gain_db, at };
      if (l.loopback) {
        this.loopback = { rms: l.loopback.rms_dbfs, peak: l.loopback.peak_dbfs, gain: l.loopback.gain_db, at };
      }
    });
    void onEvent<DegradedUpdate>("live:degraded", (d) => {
      const next = { ...this.degraded };
      if (d.degraded) next[d.channel] = d;
      else delete next[d.channel];
      this.degraded = next;
    });
    void onEvent<LiveWarning>("live:warning", (w) => {
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

  /** R, the Record button: open the start sheet, or stop. */
  async toggle(): Promise<void> {
    if (this.phase === "idle") this.sheetOpen = true;
    else if (this.phase === "recording") await this.stop();
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

  async stop(): Promise<void> {
    if (this.phase !== "recording") return;
    if (this.elapsedMs < STOP_CONFIRM_MS) {
      const ok = await app.confirm({
        title: "Stop recording?",
        message: `Only ${Math.round(this.elapsedMs / 1000)} s recorded so far.`,
        confirmLabel: "Stop",
        danger: false,
      });
      if (!ok || this.phase !== "recording") return;
    }
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
      app.error(`Could not stop the recording: ${errorText(e)}`);
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

  #apply(v: LiveStateView): void {
    const prev = this.phase;
    if (v.phase === "starting" && prev !== "starting") this.#reset();
    this.phase = v.phase;
    this.error = v.error;
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
    if (v.phase === "recording" && v.meeting_id !== null && this.#openedFor !== v.meeting_id) {
      this.#openedFor = v.meeting_id;
      app.open(v.meeting_id);
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
      app.toast("info", `Recording started from ${via}. Press Ctrl+Alt+R again to stop.`);
    } else if (t.action === "stopped") {
      app.toast("info", `Recording stopped from ${via}. Finalizing the transcript…`);
    } else {
      app.error(`Recording (${via}): ${t.message ?? "failed"}`);
    }
  }
}

export const live = new LiveStore();
