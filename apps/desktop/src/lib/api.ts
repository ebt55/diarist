// Typed wrappers over the Tauri commands in src-tauri/src/{commands,live}.rs.
// Types come from diarist_core::views and diarist_core::live via ts-rs
// (`cargo test -p diarist-core`); argument names are snake_case on the wire,
// like the command contract.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AudioDevice } from "./generated/AudioDevice";
import type { DataRootInfo } from "./generated/DataRootInfo";
import type { DownloadItem } from "./generated/DownloadItem";
import type { EngineStatus } from "./generated/EngineStatus";
import type { JobEvent } from "./generated/JobEvent";
import type { HealthCheck } from "./generated/HealthCheck";
import type { JobId } from "./generated/JobId";
import type { LiveStateView } from "./generated/LiveStateView";
import type { LiveStatusView } from "./generated/LiveStatusView";
import type { MeetingDetail } from "./generated/MeetingDetail";
import type { MeetingId } from "./generated/MeetingId";
import type { MeetingSummary } from "./generated/MeetingSummary";
import type { ModelFile } from "./generated/ModelFile";
import type { ModelFileId } from "./generated/ModelFileId";
import type { ModelRole } from "./generated/ModelRole";
import type { MoveResult } from "./generated/MoveResult";
import type { Readiness } from "./generated/Readiness";
import type { RetentionPlan } from "./generated/RetentionPlan";
import type { RetentionReport } from "./generated/RetentionReport";
import type { RuntimePackage } from "./generated/RuntimePackage";
import type { RuntimeRelease } from "./generated/RuntimeRelease";
import type { SearchHit } from "./generated/SearchHit";
import type { SegmentId } from "./generated/SegmentId";
import type { SegmentView } from "./generated/SegmentView";
import type { Settings } from "./generated/Settings";
import type { SetupStatus } from "./generated/SetupStatus";
import type { SpeakerId } from "./generated/SpeakerId";
import type { StartRequest } from "./generated/StartRequest";
import type { StartupReport } from "./generated/StartupReport";
import type { StopSummary } from "./generated/StopSummary";
import type { TrackChannel } from "./generated/TrackChannel";
import type { UpdateCheck } from "./generated/UpdateCheck";
import type { Waveform } from "./generated/Waveform";
import type { Word } from "./generated/Word";
import type { WordId } from "./generated/WordId";
import type { WordsUpdate } from "./generated/WordsUpdate";

export type { Waveform };

/** `diarist_core::export::ExportFormat` on the wire (its file extension). */
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
  /** Rename while recording, anchored at a live line's meeting-time range. */
  renameSpeakerAt: (speaker_id: SpeakerId, name: string, start_ms: number, end_ms: number) =>
    invoke<void>("rename_speaker_at", { speaker_id, name, start_ms, end_ms }),
  setSpeakerColor: (speaker_id: SpeakerId, color: string) =>
    invoke<void>("set_speaker_color", { speaker_id, color }),
  mergeSpeakers: (from: SpeakerId, into: SpeakerId) =>
    invoke<void>("merge_speakers", { from, into }),
  unmergeSpeaker: (speaker_id: SpeakerId) => invoke<void>("unmerge_speaker", { speaker_id }),
  markReviewed: (speaker_id: SpeakerId) => invoke<void>("mark_reviewed", { speaker_id }),
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
  exportMeetingTo: (id: MeetingId, format: ExportFormat, path: string) =>
    invoke<string>("export_meeting_to", { id, format, path }),

  getAudioUrl: (id: MeetingId, channel: TrackChannel | null = null) =>
    invoke<string>("get_audio_url", { id, channel }),
  getWaveform: (id: MeetingId, buckets: number) =>
    invoke<Waveform>("get_waveform", { id, buckets }),

  getSettings: () => invoke<Settings>("get_settings"),
  setSetting: (key: string, value: string) => invoke<void>("set_setting", { key, value }),
  removeSetting: (key: string) => invoke<void>("remove_setting", { key }),
  engineStatus: () => invoke<EngineStatus>("engine_status"),
  listModelFiles: () => invoke<ModelFile[]>("list_model_files"),
  selectModel: (role: ModelRole, id: ModelFileId) => invoke<void>("select_model", { role, id }),
  rescanModels: () => invoke<ModelFile[]>("rescan_models"),
  installEngine: (name: string | null = null) => invoke<string>("install_engine", { name }),
  pullModel: (name: string) => invoke<string>("pull_model", { name }),
  recommendedDownloads: () => invoke<DownloadItem[]>("recommended_downloads"),
  openModelsFolder: () => invoke<void>("open_models_folder"),
  openEngineFolder: () => invoke<void>("open_engine_folder"),
  openDownloadFolder: (name: string) => invoke<void>("open_download_folder", { name }),
  openDataFolder: () => invoke<void>("open_data_folder"),
  openLogFolder: () => invoke<void>("open_log_folder"),
  setupStatus: () => invoke<SetupStatus>("setup_status"),
  checkReady: (action: "import" | "record") => invoke<Readiness>("check_ready", { action }),
  runtimeReleases: () => invoke<RuntimeRelease[]>("runtime_releases"),
  installRuntime: (pkg: RuntimePackage) => invoke<string>("install_runtime", { package: pkg }),
  addModelUrl: (url: string, sha256: string | null) => invoke<string>("add_model_url", { url, sha256 }),
  addModelFile: (path: string, link: boolean) => invoke<string>("add_model_file", { path, link }),
  healthCheck: () => invoke<HealthCheck[]>("health_check"),
  openSoundSettings: () => invoke<void>("open_sound_settings"),

  dataRootInfo: () => invoke<DataRootInfo>("data_root_info"),
  moveDataRoot: (to: string) => invoke<MoveResult>("move_data_root", { to }),
  restartApp: () => invoke<void>("restart_app"),
  retentionPreview: () => invoke<RetentionPlan>("retention_preview"),
  applyRetention: () => invoke<RetentionReport>("apply_retention"),
  getAutostart: () => invoke<boolean>("get_autostart"),
  setAutostart: (enabled: boolean) => invoke<boolean>("set_autostart", { enabled }),
  checkForUpdate: () => invoke<UpdateCheck>("check_for_update"),
  installUpdate: () => invoke<void>("install_update"),

  listAudioDevices: () => invoke<AudioDevice[]>("list_audio_devices"),
  prepareRecording: () => invoke<void>("prepare_recording"),
  startRecording: (opts: StartRequest) => invoke<LiveStateView>("start_recording", { opts }),
  stopRecording: () => invoke<StopSummary>("stop_recording"),
  recordingStatus: () => invoke<LiveStatusView>("recording_status"),
  liveSnapshot: (meeting_id: MeetingId) => invoke<WordsUpdate[]>("live_snapshot", { meeting_id }),
  rerunPostPass: (meeting_id: MeetingId) => invoke<JobId>("rerun_post_pass", { meeting_id }),

  readLogTail: (max_lines: number) => invoke<string>("read_log_tail", { max_lines }),
  lastEngineStderr: () => invoke<string | null>("last_engine_stderr"),
  diagnosticsReport: () => invoke<string>("diagnostics_report"),
  startupReport: () => invoke<StartupReport>("startup_report"),
  log: (level: "info" | "warn" | "error", message: string) =>
    invoke<void>("ui_log", { level, message }).catch(() => {}),
};

/** Subscribes to job:progress, job:done, job:error and job:cancelled. */
export async function onJobEvent(cb: (event: JobEvent) => void): Promise<UnlistenFn> {
  const names = ["job:progress", "job:done", "job:error", "job:cancelled"] as const;
  const unlisten = await Promise.all(names.map((n) => listen<JobEvent>(n, (e) => cb(e.payload))));
  return () => unlisten.forEach((u) => u());
}

/** Subscribes to one backend event (`live:*`, `models:progress`). */
export function onEvent<T>(name: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  return listen<T>(name, (e) => cb(e.payload));
}
