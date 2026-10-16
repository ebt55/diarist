// Local crash reports (src-tauri/src/crash.rs, diarist_core::crash;
// docs/plans/11-hardening.md 8a). Reports are files in the log folder and
// are never sent anywhere. WebView errors reach the app log through an
// initialization script registered in src-tauri/src/lib.rs, not from here.
import { invoke } from "@tauri-apps/api/core";
import type { CrashKind } from "./generated/CrashKind";
import type { CrashReport } from "./generated/CrashReport";

export type { CrashReport };

export const crashApi = {
  /** The report the previous run left, if the user has not seen it yet. */
  notice: () => invoke<CrashReport | null>("crash_notice"),
  /** Every report in the log folder, newest first. */
  list: () => invoke<CrashReport[]>("crash_reports"),
  /** A text report with the user's profile folder shown as `~`. */
  text: (name: string) => invoke<string>("crash_report_text", { name }),
  acknowledge: (name: string) => invoke<void>("acknowledge_crash", { name }),
  /** Opens the log folder with the file selected. */
  reveal: (name: string) => invoke<void>("reveal_crash_report", { name }),
};

export const CRASH_KIND_LABEL: Record<CrashKind, string> = {
  panic: "Diarist error (panic)",
  native: "Diarist crash",
  engine: "Engine exit",
};

/** Shown wherever a minidump is offered. */
export const DUMP_NOTE =
  "The .dmp file holds part of the program's memory and may contain transcript text; it is for your own debugging and is never copied.";
