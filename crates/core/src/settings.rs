//! Settings keys and their defaults (`settings` table: key/value strings).
//!
//! The store keeps only what the user changed; [`effective`] lays the stored
//! values over [`DEFAULTS`], which is what the UI and the pipeline read.
//! Removing a key ([`crate::store::Store::remove_setting`]) restores its
//! default. The defaults are exported to TypeScript as
//! `generated/settingDefaults.ts` ([`typescript_module`]).

use crate::audio::echo::{clamp_threshold, EchoMode};
use crate::store::{Store, StoreResult};
use crate::types::ModelRole;
use crate::views::{selection_key, Settings};

/// Days a meeting's audio is kept; `0` keeps it forever. Pinned meetings
/// are exempt.
pub const AUDIO_RETENTION_DAYS: &str = "audio_retention_days";
/// `system`, `light` or `dark`.
pub const THEME: &str = "theme";
/// BCP-47 tag or `auto` for new recordings and imports.
pub const DEFAULT_LANGUAGE: &str = "default_language";
/// `always` or `on_issues` (`docs/01`, "Live labels").
pub const POST_PASS_MODE: &str = "post_pass_mode";
/// Keep the live run after the post-pass replaced it.
pub const KEEP_LIVE_RUNS: &str = "keep_live_runs";
/// Run and keep the standalone diarization pass (RTTM, JSON) with imports.
pub const KEEP_DIARIZATION_SEGMENTS: &str = "keep_diarization_segments";
/// Keep the 16 kHz intermediate WAV of imports.
pub const KEEP_INTERMEDIATE_WAV: &str = "keep_intermediate_wav";
/// Capture endpoint id for new recordings ("" = the default communications
/// microphone).
pub const RECORD_MIC_DEVICE: &str = "record.mic_device";
/// Render endpoint id whose loopback is recorded ("" = the default output,
/// following the app that plays).
pub const RECORD_LOOPBACK_DEVICE: &str = "record.loopback_device";
/// Diarize the mic of new recordings (several people in the room).
pub const RECORD_DIARIZE_MIC: &str = "record.diarize_mic";
/// Open the captures at 16 kHz mono via `AUTOCONVERTPCM` (`true`) or at the
/// mix format with rubato (`false`).
pub const CAPTURE_AUTOCONVERT: &str = "capture.autoconvert";
/// Automatic gain on the loopback before the WAV and the engine feed.
pub const CAPTURE_LOOPBACK_AGC: &str = "capture.loopback_agc";
/// Automatic gain on the mic.
pub const CAPTURE_MIC_AGC: &str = "capture.mic_agc";
/// Echo handling for recordings with speakers (plan 15): `auto` (the monitor
/// runs, the word gate engages on detected coupling), `off` (nothing runs,
/// nothing is gated) or `always` (gate regardless of detection) ([`echo_mode`]).
pub const ECHO_MODE: &str = "echo.mode";
/// Correlation at or above which a mic word in the gate's hysteresis band
/// (0.2 to 0.6) is dropped when its neighbours' median reaches it; 0.2 to
/// 0.6 accepted ([`echo_gate_threshold`]).
pub const ECHO_GATE_THRESHOLD: &str = "echo.gate_threshold";
/// Start `nemo-speech serve` at app launch instead of on the first Record
/// or when the start sheet opens.
pub const ENGINE_PRELOAD: &str = "engine.preload";
/// Minutes without a recording after which the live server is stopped
/// (frees its VRAM); `0` keeps it running.
pub const ENGINE_IDLE_MINUTES: &str = "engine.idle_minutes";
/// The user chose "Set up later" on the first-run screen: the app opens on
/// the library and each action says what it misses.
pub const SETUP_LATER: &str = "setup.later";
/// Low-VRAM mode: file imports use Nemotron 3.5 when it is installed (it
/// peaks at 1.8 GB against 2.6 GB for the EN model).
pub const ENGINE_LOW_VRAM: &str = "engine.low_vram";
/// Decoder lookahead of file jobs and the post-pass, no UI (docs/04 section
/// 5): "" or `model` = the ASR model's own value, `low` = none (the runtime
/// default, the low-latency setting live keeps), a number = that many 80 ms
/// frames for every model ([`file_lookahead`]).
pub const ENGINE_FILE_LOOKAHEAD: &str = "engine.file_lookahead";
/// Decoder lookahead of the live server, no UI (docs/plans/16 section 5):
/// "" or `model` = the ASR model's own `live_right_context` (none today, so
/// the runtime default), `low` = none (the runtime default, 1 frame), a
/// number = that many 80 ms frames where the model was trained for it
/// ([`live_lookahead`], [`crate::models::live_right_context`]).
pub const ENGINE_LIVE_LOOKAHEAD: &str = "engine.live_lookahead";
/// Diarizer preset of file jobs and the post-pass, no UI (docs/04 section
/// 5): "" or `model` = the diarizer's own value, `streaming` = none (the
/// runtime default, the setting live keeps), any other text = that preset
/// name for every diarizer ([`file_diar_preset`]).
pub const ENGINE_FILE_DIAR_PRESET: &str = "engine.file_diar_preset";
/// The first-run screen has set `default_language` from the system
/// language (done once).
pub const SETUP_LANGUAGE_SEEDED: &str = "setup.language_seeded";
/// Player volume, 0 to 1, for every meeting (times the normalisation).
pub const PLAYBACK_VOLUME: &str = "playback.volume";
/// The player raises a quiet track so its loud passages peak near -1 dBFS
/// (from the waveform peaks, at most +24 dB, with a limiter); the stored
/// audio does not change.
pub const PLAYBACK_NORMALIZE: &str = "playback.normalize";
/// Punctuation of the post-pass, imports, Restore and rebuilds (plan 16
/// section 3.8): `auto` (when a punctuation model is selected, installed and
/// covers the run's language) or `off` ([`crate::punct::setting`]).
pub const TRANSCRIPT_PUNCTUATION: &str = "transcript.punctuation";
/// ONNX Runtime intra-op threads of the punctuation model, no UI; clamped
/// to 1 to the machine's cores ([`crate::punct::threads`]).
pub const TRANSCRIPT_PUNCTUATION_THREADS: &str = "transcript.punctuation_threads";

/// Every known key with its default. Selections (`selected_engine`,
/// `selected_model.asr`, `selected_model.diarizer`,
/// `selected_model.punctuation`) default to "" (none;
/// the model scan picks the recommended files).
pub const DEFAULTS: &[(&str, &str)] = &[
    (AUDIO_RETENTION_DAYS, "30"),
    (THEME, "system"),
    (DEFAULT_LANGUAGE, "en"),
    (POST_PASS_MODE, "always"),
    (KEEP_LIVE_RUNS, "false"),
    (KEEP_DIARIZATION_SEGMENTS, "false"),
    (KEEP_INTERMEDIATE_WAV, "false"),
    ("selected_engine", ""),
    ("selected_model.asr", ""),
    ("selected_model.diarizer", ""),
    ("selected_model.punctuation", ""),
    (RECORD_MIC_DEVICE, ""),
    (RECORD_LOOPBACK_DEVICE, ""),
    (RECORD_DIARIZE_MIC, "false"),
    (CAPTURE_AUTOCONVERT, "true"),
    (CAPTURE_LOOPBACK_AGC, "true"),
    (CAPTURE_MIC_AGC, "false"),
    (ECHO_MODE, "auto"),
    (ECHO_GATE_THRESHOLD, "0.5"),
    (ENGINE_PRELOAD, "false"),
    (ENGINE_IDLE_MINUTES, "10"),
    (ENGINE_LOW_VRAM, "false"),
    (ENGINE_FILE_LOOKAHEAD, ""),
    (ENGINE_LIVE_LOOKAHEAD, ""),
    (ENGINE_FILE_DIAR_PRESET, ""),
    (SETUP_LATER, "false"),
    (SETUP_LANGUAGE_SEEDED, "false"),
    (PLAYBACK_VOLUME, "1.0"),
    (PLAYBACK_NORMALIZE, "true"),
    (TRANSCRIPT_PUNCTUATION, "auto"),
    (TRANSCRIPT_PUNCTUATION_THREADS, "4"),
];

/// The default of `key`, if it is a known key.
pub fn default_of(key: &str) -> Option<&'static str> {
    DEFAULTS.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// Stored values laid over the defaults.
pub fn with_defaults(stored: Settings) -> Settings {
    let mut all = Settings(
        DEFAULTS
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
    );
    all.0.extend(stored.0);
    all
}

/// The store's settings laid over the defaults.
pub fn effective(store: &dyn Store) -> StoreResult<Settings> {
    Ok(with_defaults(store.get_settings()?))
}

/// The value of `key`: stored, else its default, else "".
pub fn get<'a>(settings: &'a Settings, key: &str) -> &'a str {
    settings
        .0
        .get(key)
        .map(String::as_str)
        .or_else(|| default_of(key))
        .unwrap_or("")
}

/// `1`, `true`, `yes` or `on` (any case) is true; `0`, `false`, `no` or
/// `off` is false.
fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// A boolean setting: the stored value when it reads as one, else the
/// key's default, else false.
pub fn get_bool(settings: &Settings, key: &str) -> bool {
    parse_bool(get(settings, key))
        .or_else(|| default_of(key).and_then(parse_bool))
        .unwrap_or(false)
}

/// A non-negative integer setting: the stored value when it parses, else
/// the key's default, else 0.
pub fn get_u64(settings: &Settings, key: &str) -> u64 {
    get(settings, key)
        .trim()
        .parse()
        .ok()
        .or_else(|| default_of(key).and_then(|d| d.parse().ok()))
        .unwrap_or(0)
}

/// How long the live server may sit without a recording before it is
/// stopped (`engine.idle_minutes`; `None` for 0: never).
pub fn engine_idle_timeout(settings: &Settings) -> Option<std::time::Duration> {
    match get_u64(settings, ENGINE_IDLE_MINUTES) {
        0 => None,
        m => Some(std::time::Duration::from_secs(m.min(7 * 24 * 60) * 60)),
    }
}

/// The `engine.file_lookahead` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileLookahead {
    /// The ASR model's `file_right_context` (manifest entry or sidecar).
    Model,
    /// No flag: the runtime default.
    Low,
    /// This value for every model.
    Frames(u32),
}

impl FileLookahead {
    /// The lookahead to pass for a model whose own value is `model`.
    pub fn resolve(self, model: Option<u32>) -> Option<u32> {
        match self {
            Self::Model => model,
            Self::Low => None,
            Self::Frames(n) => Some(n),
        }
    }
}

/// `engine.file_lookahead`: `low` (any case), a non-negative integer, else
/// (`""`, `model`, anything unreadable) the model's own value.
pub fn file_lookahead(settings: &Settings) -> FileLookahead {
    let v = get(settings, ENGINE_FILE_LOOKAHEAD).trim();
    if v.eq_ignore_ascii_case("low") {
        FileLookahead::Low
    } else if let Ok(n) = v.parse::<u32>() {
        FileLookahead::Frames(n)
    } else {
        FileLookahead::Model
    }
}

/// The `engine.live_lookahead` setting (docs/plans/16 section 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveLookahead {
    /// The ASR model's `live_right_context` (manifest entry or sidecar).
    Model,
    /// No flag: the runtime default (1 frame, 160 ms).
    Low,
    /// This value for every model that was trained for it; another model
    /// keeps its own ([`crate::models::live_right_context`]).
    Frames(u32),
    /// Text that is none of the above, kept for the log line; read as
    /// [`Self::Model`].
    Unreadable(String),
}

/// `engine.live_lookahead`: `""` or `model` (any case), `low` (any case), a
/// non-negative integer, else [`LiveLookahead::Unreadable`] (`-1`, `high`).
pub fn live_lookahead(settings: &Settings) -> LiveLookahead {
    let v = get(settings, ENGINE_LIVE_LOOKAHEAD).trim();
    if v.is_empty() || v.eq_ignore_ascii_case("model") {
        LiveLookahead::Model
    } else if v.eq_ignore_ascii_case("low") {
        LiveLookahead::Low
    } else if let Ok(n) = v.parse::<u32>() {
        LiveLookahead::Frames(n)
    } else {
        LiveLookahead::Unreadable(v.to_owned())
    }
}

/// The `engine.file_diar_preset` setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileDiarPreset {
    /// The diarizer's `file_preset` (manifest entry or sidecar).
    Model,
    /// No flag: the runtime's streaming default.
    Streaming,
    /// This preset for every diarizer, passed verbatim.
    Named(String),
}

impl FileDiarPreset {
    /// The preset to pass for a diarizer whose own value is `model`.
    pub fn resolve(&self, model: Option<String>) -> Option<String> {
        match self {
            Self::Model => model,
            Self::Streaming => None,
            Self::Named(n) => Some(n.clone()),
        }
    }
}

/// `engine.file_diar_preset`: `""` or `model` (any case), `streaming` (any
/// case), else the trimmed text as a preset name.
pub fn file_diar_preset(settings: &Settings) -> FileDiarPreset {
    let v = get(settings, ENGINE_FILE_DIAR_PRESET).trim();
    if v.is_empty() || v.eq_ignore_ascii_case("model") {
        FileDiarPreset::Model
    } else if v.eq_ignore_ascii_case("streaming") {
        FileDiarPreset::Streaming
    } else {
        FileDiarPreset::Named(v.to_owned())
    }
}

/// `echo.mode`: `off`, `always`, anything else `auto`.
pub fn echo_mode(settings: &Settings) -> EchoMode {
    EchoMode::parse(get(settings, ECHO_MODE))
}

/// `echo.gate_threshold`, clamped to 0.2 to 0.6; unreadable: 0.5.
pub fn echo_gate_threshold(settings: &Settings) -> f32 {
    get(settings, ECHO_GATE_THRESHOLD)
        .trim()
        .parse::<f32>()
        .map_or(crate::audio::echo::GATE_THRESHOLD, clamp_threshold)
}

/// The language for new imports and recordings (`default_language`, "en"
/// when blank).
pub fn default_language(settings: &Settings) -> String {
    let lang = get(settings, DEFAULT_LANGUAGE).trim();
    if lang.is_empty() {
        "en".to_owned()
    } else {
        lang.to_owned()
    }
}

/// The TypeScript module with [`DEFAULTS`]
/// (`apps/desktop/src/lib/generated/settingDefaults.ts`).
pub fn typescript_module() -> String {
    debug_assert!(DEFAULTS
        .iter()
        .any(|(k, _)| *k == selection_key(ModelRole::Asr)));
    let entries = DEFAULTS
        .iter()
        .map(|(k, v)| format!("  {k:?}: {v:?},"))
        .collect::<Vec<_>>()
        .join("\n");
    let (min, max) = crate::audio::echo::GATE_THRESHOLD_RANGE;
    let (keep, drop) = (
        crate::audio::echo::GATE_KEEP_UNDER,
        crate::audio::echo::GATE_DROP_AT,
    );
    format!(
        "// This file was generated from diarist_core::settings::DEFAULTS. Do not edit this file manually.\n\n\
         /** Every settings key with its default; the store holds only changed values. */\n\
         export const SETTING_DEFAULTS = {{\n{entries}\n}} as const;\n\n\
         export type SettingKey = keyof typeof SETTING_DEFAULTS;\n\n\
         /** `echo.gate_threshold`'s accepted range and the word gate's band around it (`diarist_core::audio::echo`). */\n\
         export const ECHO_GATE = {{ min: {min:?}, max: {max:?}, keepUnder: {keep:?}, dropAt: {drop:?} }} as const;\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryStore;

    #[test]
    fn defaults_fill_gaps_and_removal_restores_them() {
        let store = MemoryStore::new();
        let s = effective(&store).unwrap();
        assert_eq!(get(&s, AUDIO_RETENTION_DAYS), "30");
        assert_eq!(get(&s, POST_PASS_MODE), "always");
        assert!(!get_bool(&s, KEEP_LIVE_RUNS));
        assert_eq!(default_language(&s), "en");
        assert_eq!(get(&s, "selected_model.asr"), "");
        assert_eq!(get(&s, RECORD_MIC_DEVICE), "");
        assert!(get_bool(&s, CAPTURE_AUTOCONVERT) && get_bool(&s, CAPTURE_LOOPBACK_AGC));
        assert!(!get_bool(&s, CAPTURE_MIC_AGC) && !get_bool(&s, RECORD_DIARIZE_MIC));
        assert!(get_bool(&s, PLAYBACK_NORMALIZE));
        assert_eq!(get(&s, PLAYBACK_VOLUME), "1.0");
        // Punctuation: on when a model is installed, 4 threads, no selection.
        assert_eq!(get(&s, TRANSCRIPT_PUNCTUATION), "auto");
        assert_eq!(get_u64(&s, TRANSCRIPT_PUNCTUATION_THREADS), 4);
        assert_eq!(get(&s, &selection_key(ModelRole::Punctuation)), "");
        // Echo handling: on detection, at 0.5; clamped, unreadable is 0.5.
        assert_eq!(
            (echo_mode(&s), echo_gate_threshold(&s)),
            (EchoMode::Auto, 0.5)
        );
        store.set_setting(ECHO_MODE, "Off").unwrap();
        store.set_setting(ECHO_GATE_THRESHOLD, "0.9").unwrap();
        let e = effective(&store).unwrap();
        assert_eq!(
            (echo_mode(&e), echo_gate_threshold(&e)),
            (EchoMode::Off, 0.6)
        );
        store.set_setting(ECHO_GATE_THRESHOLD, "high").unwrap();
        assert_eq!(echo_gate_threshold(&effective(&store).unwrap()), 0.5);
        store.remove_setting(ECHO_MODE).unwrap();
        store.remove_setting(ECHO_GATE_THRESHOLD).unwrap();
        // A value that is not a boolean falls back to the default.
        store.set_setting(CAPTURE_AUTOCONVERT, "maybe").unwrap();
        assert!(get_bool(&effective(&store).unwrap(), CAPTURE_AUTOCONVERT));
        store.set_setting(CAPTURE_AUTOCONVERT, "off").unwrap();
        assert!(!get_bool(&effective(&store).unwrap(), CAPTURE_AUTOCONVERT));
        store.set_setting(DEFAULT_LANGUAGE, "de").unwrap();
        store.set_setting(KEEP_LIVE_RUNS, "1").unwrap();
        let s = effective(&store).unwrap();
        assert_eq!(default_language(&s), "de");
        assert!(get_bool(&s, KEEP_LIVE_RUNS));
        store.remove_setting(DEFAULT_LANGUAGE).unwrap();
        assert_eq!(default_language(&effective(&store).unwrap()), "en");
        // Engine lifecycle: lazy start, 10 idle minutes by default.
        assert!(!get_bool(&s, ENGINE_PRELOAD) && !get_bool(&s, ENGINE_LOW_VRAM));
        assert_eq!(
            engine_idle_timeout(&s),
            Some(std::time::Duration::from_secs(600))
        );
        store.set_setting(ENGINE_IDLE_MINUTES, "0").unwrap();
        assert_eq!(engine_idle_timeout(&effective(&store).unwrap()), None);
        store.set_setting(ENGINE_IDLE_MINUTES, "soon").unwrap();
        assert_eq!(
            get_u64(&effective(&store).unwrap(), ENGINE_IDLE_MINUTES),
            10
        );
        assert_eq!(get(&Settings::default(), "unknown"), "");
    }

    #[test]
    fn file_lookahead_forms() {
        let store = MemoryStore::new();
        let read = |v: Option<&str>| {
            match v {
                Some(v) => store.set_setting(ENGINE_FILE_LOOKAHEAD, v).unwrap(),
                None => store.remove_setting(ENGINE_FILE_LOOKAHEAD).unwrap(),
            }
            file_lookahead(&effective(&store).unwrap())
        };
        // The default, `model` and anything unreadable: the model's value.
        for v in [
            None,
            Some(""),
            Some("model"),
            Some(" Model "),
            Some("-1"),
            Some("high"),
        ] {
            let got = read(v);
            assert_eq!(got, FileLookahead::Model, "{v:?}");
            assert_eq!(got.resolve(Some(13)), Some(13));
            assert_eq!(got.resolve(None), None, "no value, no flag");
        }
        // `low`: no flag for any model.
        let low = read(Some("LOW"));
        assert_eq!(low, FileLookahead::Low);
        assert_eq!(low.resolve(Some(13)), None);
        // A number: that value for every model, also one without its own.
        let six = read(Some(" 6 "));
        assert_eq!(six, FileLookahead::Frames(6));
        assert_eq!(six.resolve(Some(13)), Some(6));
        assert_eq!(six.resolve(None), Some(6));
        assert_eq!(read(Some("0")).resolve(Some(13)), Some(0));
    }

    #[test]
    fn live_lookahead_forms() {
        let store = MemoryStore::new();
        let read = |v: Option<&str>| {
            match v {
                Some(v) => store.set_setting(ENGINE_LIVE_LOOKAHEAD, v).unwrap(),
                None => store.remove_setting(ENGINE_LIVE_LOOKAHEAD).unwrap(),
            }
            live_lookahead(&effective(&store).unwrap())
        };
        // The default and `model`: the model's own value.
        assert_eq!(default_of(ENGINE_LIVE_LOOKAHEAD), Some(""));
        for v in [None, Some(""), Some("model"), Some(" MODEL ")] {
            assert_eq!(read(v), LiveLookahead::Model, "{v:?}");
        }
        // `low`: no flag.
        assert_eq!(read(Some("Low")), LiveLookahead::Low);
        // A number, trimmed; 0 is a value (80 ms), not "unset".
        assert_eq!(read(Some(" 6 ")), LiveLookahead::Frames(6));
        assert_eq!(read(Some("0")), LiveLookahead::Frames(0));
        assert_eq!(read(Some("13")), LiveLookahead::Frames(13));
        // Anything else is kept for the log line and read as the model's.
        for v in ["-1", "high", "6.5", "6 frames"] {
            assert_eq!(read(Some(v)), LiveLookahead::Unreadable(v.into()), "{v}");
        }
        // The file lookahead is a separate key.
        store.set_setting(ENGINE_LIVE_LOOKAHEAD, "6").unwrap();
        assert_eq!(
            file_lookahead(&effective(&store).unwrap()),
            FileLookahead::Model
        );
    }

    #[test]
    fn file_diar_preset_forms() {
        let store = MemoryStore::new();
        let read = |v: Option<&str>| {
            match v {
                Some(v) => store.set_setting(ENGINE_FILE_DIAR_PRESET, v).unwrap(),
                None => store.remove_setting(ENGINE_FILE_DIAR_PRESET).unwrap(),
            }
            file_diar_preset(&effective(&store).unwrap())
        };
        let own = || Some("v3-offline".to_owned());
        // The default and `model`: the diarizer's value, none without one.
        for v in [None, Some(""), Some("model"), Some(" MODEL ")] {
            let got = read(v);
            assert_eq!(got, FileDiarPreset::Model, "{v:?}");
            assert_eq!(got.resolve(own()).as_deref(), Some("v3-offline"));
            assert_eq!(got.resolve(None), None);
        }
        // `streaming`: no flag for any diarizer.
        let streaming = read(Some("Streaming"));
        assert_eq!(streaming, FileDiarPreset::Streaming);
        assert_eq!(streaming.resolve(own()), None);
        // Anything else: that name, verbatim, for every diarizer.
        let named = read(Some(" v3-streaming "));
        assert_eq!(named, FileDiarPreset::Named("v3-streaming".into()));
        assert_eq!(named.resolve(own()).as_deref(), Some("v3-streaming"));
        assert_eq!(named.resolve(None).as_deref(), Some("v3-streaming"));
    }

    /// Writes `generated/settingDefaults.ts` (name matches the `export_bindings`
    /// filter in docs/DEV.md).
    #[test]
    fn export_bindings_setting_defaults() {
        let module = typescript_module();
        assert!(module.contains("  \"audio_retention_days\": \"30\","));
        assert!(module.contains("  \"selected_model.asr\": \"\","));
        if let Some(dir) = std::env::var_os("TS_RS_EXPORT_DIR") {
            let path = std::path::PathBuf::from(dir).join("settingDefaults.ts");
            if std::fs::read_to_string(&path).ok().as_deref() != Some(module.as_str()) {
                std::fs::write(path, module).unwrap();
            }
        }
    }
}
