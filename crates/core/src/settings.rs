//! Settings keys and their defaults (`settings` table: key/value strings).
//!
//! The store keeps only what the user changed; [`effective`] lays the stored
//! values over [`DEFAULTS`], which is what the UI and the pipeline read.
//! Removing a key ([`crate::store::Store::remove_setting`]) restores its
//! default. The defaults are exported to TypeScript as
//! `generated/settingDefaults.ts` ([`typescript_module`]).

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

/// Every known key with its default. Selections (`selected_engine`,
/// `selected_model.asr`, `selected_model.diarizer`) default to "" (none;
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
    (RECORD_MIC_DEVICE, ""),
    (RECORD_LOOPBACK_DEVICE, ""),
    (RECORD_DIARIZE_MIC, "false"),
    (CAPTURE_AUTOCONVERT, "true"),
    (CAPTURE_LOOPBACK_AGC, "true"),
    (CAPTURE_MIC_AGC, "false"),
    (ENGINE_PRELOAD, "false"),
    (ENGINE_IDLE_MINUTES, "10"),
    (ENGINE_LOW_VRAM, "false"),
    (SETUP_LATER, "false"),
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
    format!(
        "// This file was generated from meet_core::settings::DEFAULTS. Do not edit this file manually.\n\n\
         /** Every settings key with its default; the store holds only changed values. */\n\
         export const SETTING_DEFAULTS = {{\n{entries}\n}} as const;\n\n\
         export type SettingKey = keyof typeof SETTING_DEFAULTS;\n"
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
