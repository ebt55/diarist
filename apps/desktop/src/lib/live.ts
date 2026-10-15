// Payloads of the live recording commands and `live:*` events.
//
// TODO(phase3b): these mirror `meet_core::live` (WordsUpdate, LevelsUpdate,
// DegradedUpdate, LiveWarning, DeviceChange, LiveIssues, RecorderStatus,
// ServeStatus, audio::capture::DeviceInfo) and src-tauri/src/live.rs by hand.
// Generate them with ts-rs once the core derives TS for the live types.
import type { Channel } from "./generated/Channel";
import type { JobId } from "./generated/JobId";
import type { MeetingId } from "./generated/MeetingId";
import type { RunId } from "./generated/RunId";
import type { SpeakerId } from "./generated/SpeakerId";

export type LivePhase = "idle" | "starting" | "recording" | "stopping";

/** One live word; times are meeting milliseconds. */
export interface LiveWord {
  start_ms: number;
  end_ms: number;
  text: string;
  /** `me`, `spk_N` or `spk_unknown`; provisional while recording. */
  label: string;
  /** The live run's speaker row (rename it like any speaker). */
  speaker_id: SpeakerId | null;
}

/** `live:words`: replaces every earlier `rev` of the same channel and `utt`. */
export interface WordsUpdate {
  meeting_id: MeetingId;
  run_id: RunId;
  channel: Channel;
  utt: number;
  rev: number;
  words: LiveWord[];
}

/** `live:text_final`: text of `channel` before `until_ms` is final. */
export interface TextFinal {
  channel: Channel;
  until_ms: number;
}

export interface LevelOut {
  at_ms: number;
  rms_dbfs: number;
  peak_dbfs: number;
  /** Gain the automatic gain control applied (0 on a channel without it). */
  gain_db: number | null;
}

/** `live:levels`, at most 10 Hz; `null` for a channel without a new reading. */
export interface LevelsPayload {
  mic: LevelOut | null;
  loopback: LevelOut | null;
}

/** `live:degraded`. */
export interface DegradedUpdate {
  channel: Channel;
  degraded: boolean;
  reason: string;
}

export type ChangeReason = "initial" | "invalidated" | "default_changed" | "followed" | "resumed" | "requested";

/** `live:device_changed`. */
export interface DeviceChange {
  channel: Channel;
  device_id: string;
  name: string;
  reason: ChangeReason;
  at_ms: number;
}

/** `live:warning` (`at` is the unix ms the app received it). */
export type LiveWarning =
  | { kind: "loopback_silent"; seconds: number; at?: number }
  | { kind: "loopback_quiet"; device: string; peak_dbfs?: number; gain_db?: number; at?: number }
  | { kind: "capture"; channel: Channel; message: string; at?: number };

export interface PostPassView {
  job_id: JobId;
  meeting_id: MeetingId | null;
  title: string;
}

/** `live:state` and `recording_status().state`. */
export interface LiveStateView {
  phase: LivePhase;
  meeting_id: MeetingId | null;
  run_id: RunId | null;
  title: string | null;
  /** Unix ms of meeting time zero. */
  started_at: number | null;
  elapsed_ms: number;
  devices: Record<string, DeviceChange>;
  degraded: Record<string, DegradedUpdate>;
  warnings: LiveWarning[];
  post_passes: PostPassView[];
  error: string | null;
}

/** `live:toggled`: the global shortcut or the tray acted. */
export interface Toggled {
  action: "started" | "stopped" | "failed";
  via: "shortcut" | "tray";
  meeting_id: MeetingId | null;
  message: string | null;
}

export interface StartRequest {
  title: string | null;
  language: string | null;
  diarize_mic: boolean;
  /** Capture endpoint id; null = the default communications microphone. */
  mic: string | null;
  /** Render endpoint id; null = the default output, following the app that plays. */
  loopback: string | null;
}

export interface LiveIssues {
  degraded: number;
  engine_down: number;
  restarts: number;
  gaps: number;
  gap_ms: number;
  jumps: number;
  device_changes: number;
}

export interface StopView {
  meeting_id: MeetingId;
  live_run: RunId;
  duration_ms: number;
  job_id: JobId;
  transcribe: boolean;
  issues: LiveIssues;
}

export interface AudioDevice {
  id: string;
  name: string;
  flow: "render" | "capture";
  is_default: boolean;
  is_default_comms: boolean;
}

export type ServeStatus =
  | { state: "stopped" }
  | { state: "starting"; port: number }
  | { state: "ready"; port: number }
  | { state: "restarting"; attempt: number; last_error: string }
  | { state: "failed"; message: string };

export interface ServeView {
  status: ServeStatus;
  uptime_ms: number | null;
  restarts: number;
  pid: number | null;
  resident_vram_mb: number | null;
  asr_model: string;
  diar_model: string;
  log_path: string | null;
}

export interface CaptureStatus {
  device_id: string | null;
  device_name: string | null;
  reopens: number;
  overflow_packets: number;
  last_error: string | null;
}

export interface RecorderStatus {
  meeting_id: MeetingId;
  run_id: RunId;
  elapsed_ms: number;
  issues: LiveIssues;
  captures: Record<string, CaptureStatus>;
  revisions: Record<string, number>;
}

export interface LiveStatusView {
  state: LiveStateView;
  recorder: RecorderStatus | null;
  server: ServeView | null;
  queue_running: JobId | null;
  queue_waiting: JobId[];
  shortcut: string;
  shortcut_error: string | null;
}

/** One item of "Download recommended" (`recommended_downloads`). */
export interface DownloadItem {
  kind: "engine" | "model";
  name: string;
  label: string;
  size_bytes: number;
  installed: boolean;
}

/** `models:progress` (src-tauri `events::DownloadProgress`; TODO: a view type). */
export interface DownloadProgress {
  kind: "engine" | "model";
  name: string;
  done_bytes: number;
  total_bytes: number | null;
  finished: boolean;
  error: string | null;
}

/** Text for a warning banner. */
export function warningText(w: LiveWarning): string {
  switch (w.kind) {
    case "loopback_silent":
      return `No system audio for ${w.seconds} s while the microphone hears speech. Is the right output selected?`;
    case "loopback_quiet":
      return `System audio from ${w.device || "the output"} is very quiet; raise its Windows volume or pick another output.`;
    case "capture":
      return `${w.channel === "mic" ? "Microphone" : "System audio"}: ${w.message}`;
    default:
      return JSON.stringify(w);
  }
}
