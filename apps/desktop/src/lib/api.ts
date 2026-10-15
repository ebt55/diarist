// Typed wrappers over Tauri commands (apps/desktop/src-tauri/src/lib.rs).
// Types are generated from meet_core::views by ts-rs (`cargo test -p meet-core`).
import { invoke } from "@tauri-apps/api/core";
import type { MeetingSummary } from "./generated/MeetingSummary";

export function listMeetings(): Promise<MeetingSummary[]> {
  return invoke<MeetingSummary[]>("list_meetings");
}
