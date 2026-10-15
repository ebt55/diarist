# Adjudication of contested review points (Fable 5.1, 15 Oct 2026)

Process: the Opus 5.5 review (research/05) and the Fable architect disagreed on four points. A separate Fable 5.1 agent adjudicated with web-checked evidence. The architect accepted all four verdicts; the Opus reviewer was asked to concur or object (research/05 records the final state).

## A. Capture-path resampling: verdict Opus

- `AUTOCONVERTPCM` inserts a channel matrixer and sample-rate converter as needed; `SRC_DEFAULT_QUALITY` picks the better converter. Loopback is shared-mode only and autoconvert is a shared-mode feature. MS Learn: audclnt-streamflags-xxx-constants.
- In production: anarlog opens every render endpoint with wasapi-rs `EventsShared { autoconvert: true }` at a fixed 44.1 kHz so the published rate never changes across endpoint switches; wasapi-rs 0.26 sets `AUTOCONVERTPCM | SRC_DEFAULT_QUALITY` and ORs it with `LOOPBACK`. Mark Heath reports 16 kHz mono capture via the flag worked well.
- No public measurement of `SRC_DEFAULT_QUALITY` exists; counter-evidence is old and driver-specific (SDL with an RME driver). Loopback audio has already been through Teams/Zoom codecs and noise suppression; robustness decides WER, not filter shape.
- On a rate change both designs lose the stream (`AUDCLNT_E_DEVICE_INVALIDATED`) and reopen; with autoconvert the reopened stream is still 16 kHz mono, so WAV writer, anchors and engine feed only see a gap.

Decision: open mic and loopback at 16 kHz mono PCM16 with `AUTOCONVERTPCM | SRC_DEFAULT_QUALITY`; that stream is both engine feed and archive. rubato stays only for file import. Setting `capture.autoconvert = false` falls back to mix format plus rubato for a bad OEM converter. Poll with a timeout rather than wait on loopback events (Chromium notes they are unreliable when nothing renders). Spike check later: sweep plus speech through 48 kHz and 44.1 kHz endpoints, compare spectrograms and sample counts against QPC.

## B. Stored playback audio: verdict Opus (FLAC) plus Fable's retention; no Media Foundation

- Pure-Rust FLAC encoders are mature: flacenc 0.5.1 (Dec 2025, 734k downloads); flac-codec 1.3.3 (Aug 2026, RFC 9639, modeled on the reference implementation, reference test suite ported). symphonia already decodes FLAC.
- Measured with ffmpeg on 16 kHz mono PCM16: AMI ES2002a 40.7 MB to 12.4 MB (30 percent); a Zoom recording 67.2 MB to 26.6 MB (40 percent). Zero-filled gaps cost nothing.
- Media Foundation's AAC encoder is AAC-LC only, needs 16-bit PCM at 44.1 or 48 kHz input, and its lowest rate is 96 kbps: 16 kHz speech would be upsampled 3x first, 43 MB/h lossy, plus unsafe COM. If a lossy tier is ever wanted, Opus at 24 to 32 kbps (11 to 14 MB/h, native in Chromium) wins.
- Storage at 3 h/day: stereo WAV 230 MB/h, 252 GB/yr, 20.7 GB at 30-day retention; stereo FLAC about 90 MB/h, about 100 GB/yr, 8 GB at 30 days.

Decision: per-channel 16 kHz WAVs while recording (crash-safe); after the post-pass encode one stereo FLAC (L mic, R loopback) with flac-codec, register it as the `playback` track, delete the WAVs; retention 30 days, pinned exempt; imports keep the original. Chromium supports FLAC; the asset protocol's 206 support covers seeking. Hard-panned me/them is unpleasant in headphones: route `<audio>` through a Web Audio gain matrix, mono by default, with a solo-channel toggle. Risk: flac-codec is a one-maintainer crate; test by decoding output bit-exactly with symphonia; flacenc is a drop-in swap.

## C. Install and data layout: verdict hybrid (Fable's structure, Tauri's paths)

- Tauri's NSIS template (installer.nsi) in currentUser mode sets `$INSTDIR = $LOCALAPPDATA\${PRODUCTNAME}`; with productName `meet-assist` that is exactly the old data root. `NsisConfig` has no `installDir` field, so changing the default means forking the template.
- The uninstaller checkbox is unchecked by default, skipped when `$UpdateMode = 1`, and deletes `$APPDATA\${BUNDLEID}` and `$LOCALAPPDATA\${BUNDLEID}`; with identifier `app.meetassist.desktop` that is `%LOCALAPPDATA%\app.meetassist.desktop`, which is Tauri's `app_local_data_dir()`. `app_data_dir()` and `app_config_dir()` are Roaming. POSTUNINSTALL hooks run after the app-data block and can read `$DeleteAppDataCheckboxState` and `$UpdateMode`.

Decision: install dir stays Tauri's default (`%LOCALAPPDATA%\meet-assist`); a forked template is a liability with Tauri 3 in alpha and the directory page lets users move it. Data root default `%LOCALAPPDATA%\app.meetassist.desktop` (db, audio, engine, models, logs; nothing in Roaming), user-relocatable with the chosen root written to `HKCU\Software\meet-assist\DataRoot` so the app and the NSIS hook both find it; portable mode `<exe dir>\data`. The uninstall checkbox remains the "wipe everything" path; a POSTUNINSTALL hook, when the box is unticked and not updating, offers "Remove downloaded engine and models (1.6 GB)? Transcripts and recordings are kept." Risk: a user who ticks the box loses transcripts; that is what the label says, off by default, never shown during updates.

## D. Edits surviving a new run: verdict Fable, with two tightenings

Cases: (1) live "Alice" split by the post-pass into X and Y: greedy one-to-one matches X and flags Y `needs_review`, which is right because a split usually means two people the live diarizer conflated. (2) two live labels merged into "Bob", post-pass yields one Z: Z matches the root with the largest overlap and gets Bob; replaying a label-keyed merge would be a no-op at best, wrong at worst. (3) a reassigned segment shifted by 0.5 s: an 80-percent-of-segment rule drops the edit silently when re-segmentation makes the segment larger; 80-percent-of-edit flips the whole segment.

Tightenings adopted: (a) replay reassigns at word level: new words whose midpoints fall inside the edit range belong to the edit's target; if that is the whole segment, reassign it, otherwise split at the word boundary and reassign the covered part; replay in `created_at` order. (b) resolve the target display name against the new run after carry-over; if absent, create a speaker row (`user_N`, `needs_review = 1`) rather than dropping the edit. Ignore carry-over matches under about 5 s of speech so spurious live labels cannot claim names. Risk: a genuine one-to-many shows one unnamed badge; one click merges it.

## Accepted dispositions reopened

- `VACUUM INTO` is a legitimate, transactionally consistent backup method when written to a temp file, flushed and renamed; the backup API is also fine. The ban is lifted.
- NVIDIA sysmem fallback is bounded by shared GPU memory and off once "Prefer No Sysmem Fallback" is set, so the degrade path must still handle a hard allocation failure (an `Error { Oom }` from the engine), not only slowdowns.
