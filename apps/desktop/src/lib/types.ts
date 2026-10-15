// Mirrors crates/core/src/types.rs (serde JSON shape). Keep in sync by hand
// until type generation lands; field names are the SQLite column names.

export type Source = "live" | "file";
export type Channel = "mic" | "loopback" | "source";
export type MeetingStatus = "recording" | "processing" | "done" | "failed";

export interface Meeting {
  id: string;
  title: string;
  source: Source;
  source_path: string | null;
  /** Unix ms, wall clock. */
  started_at: number;
  ended_at: number | null;
  duration_ms: number | null;
  status: MeetingStatus;
  engine_info: Record<string, unknown> | null;
  created_at: number;
  updated_at: number;
}
