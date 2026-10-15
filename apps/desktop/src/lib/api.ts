// Typed wrappers over Tauri commands (apps/desktop/src-tauri/src/lib.rs).
import { invoke } from "@tauri-apps/api/core";
import type { Meeting } from "./types";

export function listMeetings(): Promise<Meeting[]> {
  return invoke<Meeting[]>("list_meetings");
}
