// Typed wrappers over the Tauri commands in src-tauri/src/commands.rs.
// Types come from meet_core::views via ts-rs (`cargo test -p meet-core`);
// argument names are snake_case on the wire, like the command contract.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { EngineStatus } from "./generated/EngineStatus";
import type { JobEvent } from "./generated/JobEvent";
import type { JobId } from "./generated/JobId";
import type { LiveRelabelEvent } from "./generated/LiveRelabelEvent";
import type { LiveWordsEvent } from "./generated/LiveWordsEvent";
import type { MeetingDetail } from "./generated/MeetingDetail";
import type { MeetingId } from "./generated/MeetingId";
import type { MeetingSummary } from "./generated/MeetingSummary";
import type { ModelFile } from "./generated/ModelFile";
import type { ModelFileId } from "./generated/ModelFileId";
import type { ModelRole } from "./generated/ModelRole";
import type { SearchHit } from "./generated/SearchHit";
import type { SegmentId } from "./generated/SegmentId";
import type { SegmentView } from "./generated/SegmentView";
import type { Settings } from "./generated/Settings";
import type { SpeakerId } from "./generated/SpeakerId";
import type { TrackChannel } from "./generated/TrackChannel";
import type { Word } from "./generated/Word";
import type { WordId } from "./generated/WordId";

/**
 * Peak envelope of the playback track (`get_waveform`). Not generated: it is
 * a src-tauri type until core grows `views::Waveform` (Phase 2 report).
 */
export interface Waveform {
  channels: number;
  sample_rate: number;
  duration_ms: number;
  /** peaks[channel][bucket], 0..255. */
  peaks: number[][];
}

export type ExportFormat = "md" | "txt" | "json" | "srt" | "vtt";

/** Error text from a rejected command (commands reject with a string). */
export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

export const api = {
  listMeetings: () => invoke<MeetingSummary[]>("list_meetings"),
  getMeeting: (id: MeetingId) => invoke<MeetingDetail>("get_meeting", { id }),
  getSegments: (id: MeetingId, offset: number, limit: number) =>
    invoke<SegmentView[]>("get_segments", { id, offset, limit }),
  getWords: (id: MeetingId, from_ms: number, to_ms: number) =>
    invoke<Word[]>("get_words", { id, from_ms, to_ms }),
  search: (query: string, limit: number) => invoke<SearchHit[]>("search", { query, limit }),

  importFile: (path: string, title: string | null = null) =>
    invoke<JobId>("import_file", { path, title }),
  cancelJob: (job_id: JobId) => invoke<void>("cancel_job", { job_id }),

  renameSpeaker: (speaker_id: SpeakerId, name: string, anchor_segment_id: SegmentId | null) =>
    invoke<void>("rename_speaker", { speaker_id, name, anchor_segment_id }),
  setSpeakerColor: (speaker_id: SpeakerId, color: string) =>
    invoke<void>("set_speaker_color", { speaker_id, color }),
  mergeSpeakers: (from: SpeakerId, into: SpeakerId) =>
    invoke<void>("merge_speakers", { from, into }),
  unmergeSpeaker: (speaker_id: SpeakerId) => invoke<void>("unmerge_speaker", { speaker_id }),
  reassignSegment: (segment_id: SegmentId, speaker_id: SpeakerId) =>
    invoke<void>("reassign_segment", { segment_id, speaker_id }),
  splitSegment: (segment_id: SegmentId, word_id: WordId) =>
    invoke<SegmentId>("split_segment", { segment_id, word_id }),
  setMeetingTitle: (id: MeetingId, title: string) =>
    invoke<void>("set_meeting_title", { id, title }),
  setAudioKeep: (id: MeetingId, keep: boolean) => invoke<void>("set_audio_keep", { id, keep }),
  deleteMeeting: (id: MeetingId) => invoke<void>("delete_meeting", { id }),
  exportMeeting: (id: MeetingId, format: ExportFormat) =>
    invoke<string | null>("export_meeting", { id, format }),

  getAudioUrl: (id: MeetingId, channel: TrackChannel | null = null) =>
    invoke<string>("get_audio_url", { id, channel }),
  getWaveform: (id: MeetingId, buckets: number) =>
    invoke<Waveform>("get_waveform", { id, buckets }),

  getSettings: () => invoke<Settings>("get_settings"),
  setSetting: (key: string, value: string) => invoke<void>("set_setting", { key, value }),
  engineStatus: () => invoke<EngineStatus>("engine_status"),
  listModelFiles: () => invoke<ModelFile[]>("list_model_files"),
  selectModel: (role: ModelRole, id: ModelFileId) => invoke<void>("select_model", { role, id }),
  rescanModels: () => invoke<ModelFile[]>("rescan_models"),
  openModelsFolder: () => invoke<void>("open_models_folder"),
  openDataFolder: () => invoke<void>("open_data_folder"),

  readLogTail: (max_lines: number) => invoke<string>("read_log_tail", { max_lines }),
  lastEngineStderr: () => invoke<string | null>("last_engine_stderr"),
  log: (level: "info" | "warn" | "error", message: string) =>
    invoke<void>("ui_log", { level, message }).catch(() => {}),
};

/** Subscribes to job:progress, job:done and job:error. */
export async function onJobEvent(cb: (event: JobEvent) => void): Promise<UnlistenFn> {
  const names = ["job:progress", "job:done", "job:error"] as const;
  const unlisten = await Promise.all(names.map((n) => listen<JobEvent>(n, (e) => cb(e.payload))));
  return () => unlisten.forEach((u) => u());
}

/** `transcript:words` (live mode, Phase 3; no producer yet). */
export function onLiveWords(cb: (event: LiveWordsEvent) => void): Promise<UnlistenFn> {
  return listen<LiveWordsEvent>("transcript:words", (e) => cb(e.payload));
}

/** `transcript:relabel` (live mode, Phase 3; no producer yet). */
export function onLiveRelabel(cb: (event: LiveRelabelEvent) => void): Promise<UnlistenFn> {
  return listen<LiveRelabelEvent>("transcript:relabel", (e) => cb(e.payload));
}
