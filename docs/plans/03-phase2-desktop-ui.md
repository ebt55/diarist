# Phase 2: desktop UI

Status: draft, to be delegated once Phase 0 is merged. Runs in parallel with Phase 1 against an in-memory mock store; wiring to the real `meet_core::db` is a short follow-up chunk.

## Goal

A fast, keyboard-friendly Tauri 2 + Svelte 5 interface over the data model: a library of meetings with full-text search, a meeting view with a virtualized speaker-coloured transcript and audio playback with click-to-seek, speaker rename/merge/colour, import of audio files with progress, export, settings and a diagnostics pane.

## Owns

`apps/desktop/**`. May add a mock store under `apps/desktop/src-tauri/src/mock.rs`. Must not touch `crates/`.

## Command contract (Tauri commands, all return `Result<T, String>`)

```
list_meetings() -> Vec<MeetingSummary>
get_meeting(id) -> MeetingDetail { meeting, speakers: Vec<SpeakerView> }
get_segments(id, offset, limit) -> Vec<SegmentView>   // speaker name + colour resolved, seq ordered
get_words(id, from_ms, to_ms) -> Vec<Word>
search(query, limit) -> Vec<SearchHit { meeting_id, meeting_title, segment_id, start_ms, snippet }>
import_file(path, title?) -> job_id                    // emits events job:progress, job:done, job:error
cancel_job(job_id)
rename_speaker(speaker_id, name) / set_speaker_color(speaker_id, hex) / merge_speakers(from, into) / unmerge_speaker(id) / reassign_segment(segment_id, speaker_id)
set_meeting_title(id, title) / delete_meeting(id)
export_meeting(id, format) -> path                     // uses the dialog plugin for the save location
get_audio_url(id, channel) -> string                   // asset-protocol URL for the stored WAV
get_settings() / set_setting(key, value)
engine_status() -> EngineStatus                        // installed flags, dirs, gpu name; stub values from the mock for now
```

Events: `job:progress {job_id, done_ms, total_ms}`, `job:done {job_id, meeting_id}`, `job:error {job_id, message}`, `transcript:words` and `transcript:relabel` (reserved for live mode; define the payload types now, do not implement producers).

Types mirror `meet_core::types` plus view structs (`MeetingSummary`, `SpeakerView`, `SegmentView`, `SearchHit`, `EngineStatus`) defined in `src-tauri/src/views.rs` and generated or hand-mirrored in `src/lib/types.ts`.

## Screens

1. **Library.** Left: search box (Ctrl+K focuses it) and the meeting list sorted by date, each row with title, date, duration, speaker count. Search results replace the list and show snippets with `<mark>` highlights; Enter opens the meeting at that segment. Top: Import button (Ctrl+O) and drag-and-drop zone for files.
2. **Meeting view.** Header: editable title, date, duration, status chip. Speaker legend: colour chip, name (double-click or R to rename), menu with merge-into and colour. Transcript: virtualized (virtua) list of segments; each row shows a monospace timestamp, speaker chip, text; the word under the playhead is highlighted; clicking a word seeks audio. Audio bar: play/pause (Space), seek, speed. Right-click a segment: reassign speaker. Export menu (md, txt, json, srt, vtt).
3. **Import progress.** Toast or inline row in the library showing percent and elapsed; error with "show log".
4. **Settings.** Data directory, audio retention toggle, theme, engine status with Install button (calls a stub for now).
5. **Diagnostics.** Tail of the app log (from tauri-plugin-log) and engine stderr of the last job.

## Design rules

- Plain CSS, custom properties, light and dark. No component library. Dense but readable: 14 px body, monospace timestamps, speaker colours as subtle left borders plus chip, high contrast in both themes.
- Keyboard first: every action in the list above has a shortcut; focus states visible.
- Window hidden until first paint (already in scaffold). Target under 300 ms from `show()` to interactive on a library of 500 meetings (mock generates them).
- Bundle: keep the JS under 150 kB gzipped. Svelte 5 runes; stores only where needed.

## Acceptance

- `bun run build` and `bun run tauri build --debug` pass; app opens to the library populated by the mock (500 meetings, 3 with 2,000 segments each) and scrolls smoothly in the transcript view.
- All commands above are implemented against the mock store and exercised by the UI.
- `cargo clippy` clean for `src-tauri`; `bun run check` (svelte-check) clean.
- Screenshots of library, meeting view (light and dark) saved to `docs/screenshots/` for review.

## Out of scope

Real DB wiring (next chunk), live recording controls (Phase 3), engine installer flow (Phase 5).
