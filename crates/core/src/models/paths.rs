//! The data root and the folders under it (`docs/01`, "Data layout on disk").
//!
//! Resolution order:
//! 0. `DIARIST_DATA_DIR` (development and tests: run the app or the CLI
//!    against a scratch data root).
//! 1. Portable mode: a `portable.txt` next to the executable. The data root
//!    is `<exe dir>\data`, or the path on the file's first line (relative
//!    paths are taken from the exe dir).
//! 2. `HKCU\Software\Diarist\DataRoot` (REG_SZ), set when the user moves it.
//! 3. `%LOCALAPPDATA%\app.diarist.desktop` (Tauri's `app_local_data_dir`).

use std::path::{Component, Path, PathBuf};

use crate::error::{CoreError, Result};

/// Tauri bundle identifier: the default data root's folder name.
pub const APP_IDENTIFIER: &str = "app.diarist.desktop";
/// Registry key (under HKCU) and value naming a relocated data root.
pub const REGISTRY_KEY: &str = r"Software\Diarist";
pub const REGISTRY_VALUE: &str = "DataRoot";
/// Marker file next to the executable that turns on portable mode.
pub const PORTABLE_MARKER: &str = "portable.txt";
/// Environment variable that overrides the data root (development, tests).
pub const DATA_DIR_ENV: &str = "DIARIST_DATA_DIR";

/// Why the data root is where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataRootSource {
    /// Given explicitly (CLI `--data-dir`, `DIARIST_DATA_DIR`, tests).
    Explicit,
    Portable,
    Registry,
    Default,
}

/// The data root and its well-known children. `models\` and `engine\` can
/// be redirected (dev setups, tests); paths outside the root are stored
/// absolute in `model_files`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataPaths {
    pub root: PathBuf,
    pub source: DataRootSource,
    pub models_override: Option<PathBuf>,
    pub engine_override: Option<PathBuf>,
}

impl DataPaths {
    /// Uses `root` as given.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            source: DataRootSource::Explicit,
            models_override: None,
            engine_override: None,
        }
    }

    /// Uses `dir` instead of `<root>\models`. A relative `dir` is made
    /// absolute (from the current directory) so files found there are stored
    /// with paths that resolve the same way later.
    pub fn with_models_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.models_override = Some(absolute_or_same(dir.into()));
        self
    }

    /// Uses `dir` instead of `<root>\engine` (a folder of runtimes, or one
    /// runtime); made absolute like [`DataPaths::with_models_dir`].
    pub fn with_engine_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.engine_override = Some(absolute_or_same(dir.into()));
        self
    }

    /// Resolves the data root (`DIARIST_DATA_DIR`, portable marker,
    /// registry, default).
    pub fn resolve() -> Result<Self> {
        if let Some(dir) = std::env::var_os(DATA_DIR_ENV).filter(|d| !d.is_empty()) {
            // Native separators, so paths built under the root are uniform.
            return Ok(Self::at(
                PathBuf::from(dir).components().collect::<PathBuf>(),
            ));
        }
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        if let Some(dir) = &exe_dir {
            if let Some(root) = portable_root(dir) {
                return Ok(Self {
                    source: DataRootSource::Portable,
                    ..Self::at(root)
                });
            }
        }
        if let Some(root) = registry_root() {
            return Ok(Self {
                source: DataRootSource::Registry,
                ..Self::at(root)
            });
        }
        Ok(Self {
            source: DataRootSource::Default,
            ..Self::at(Self::default_root()?)
        })
    }

    /// `%LOCALAPPDATA%\app.diarist.desktop`: the data root when nothing
    /// else names one (Tauri's `app_local_data_dir`).
    pub fn default_root() -> Result<PathBuf> {
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .ok_or_else(|| CoreError::Models("cannot find %LOCALAPPDATA%".into()))?;
        Ok(local.join(APP_IDENTIFIER))
    }

    /// The root as Settings shows it, with why it cannot be moved, if so.
    pub fn info(&self) -> crate::views::DataRootInfo {
        let (source, move_blocked) = match self.source {
            DataRootSource::Default => ("default", None),
            DataRootSource::Registry => ("relocated", None),
            DataRootSource::Portable => (
                "portable",
                Some(format!(
                    "Portable mode: the data folder is set by {PORTABLE_MARKER} next to diarist.exe."
                )),
            ),
            DataRootSource::Explicit => (
                "explicit",
                Some(format!(
                    "The data folder is set by {DATA_DIR_ENV} (or --data-dir) for this run."
                )),
            ),
        };
        crate::views::DataRootInfo {
            root: self.root.display().to_string(),
            source: source.to_owned(),
            default_root: Self::default_root()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            move_blocked,
        }
    }

    /// `models\`, also `NEMO_SPEECH_MODEL_DIR` for `nemo-speech pull`.
    pub fn models(&self) -> PathBuf {
        self.models_override
            .clone()
            .unwrap_or_else(|| self.root.join("models"))
    }

    /// `engine\`: one folder per runtime version.
    pub fn engine(&self) -> PathBuf {
        self.engine_override
            .clone()
            .unwrap_or_else(|| self.root.join("engine"))
    }

    /// The SQLite database, in the plain form at any length (it is shown and
    /// logged). `SqliteStore` converts a long one for SQLite itself.
    pub fn db(&self) -> PathBuf {
        self.root.join("diarist.db")
    }

    /// `audio\`: `audio\<meeting uid>\...`.
    pub fn audio(&self) -> PathBuf {
        self.root.join("audio")
    }

    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// `path` relative to the root with forward slashes (the `model_files.path`
    /// form); paths outside the root are returned absolute.
    pub fn relative(&self, path: &Path) -> String {
        match path.strip_prefix(&self.root) {
            Ok(rel) => rel
                .components()
                .filter_map(|c| match c {
                    Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("/"),
            Err(_) => path.to_string_lossy().into_owned(),
        }
    }

    /// Inverse of [`DataPaths::relative`].
    pub fn absolute(&self, rel: &str) -> PathBuf {
        let p = Path::new(rel);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            rel.split('/')
                .fold(self.root.clone(), |acc, part| acc.join(part))
        }
    }
}

fn absolute_or_same(p: PathBuf) -> PathBuf {
    if p.is_absolute() {
        p
    } else {
        std::path::absolute(&p).unwrap_or(p)
    }
}

fn portable_root(exe_dir: &Path) -> Option<PathBuf> {
    let marker = exe_dir.join(PORTABLE_MARKER);
    let text = std::fs::read_to_string(marker).ok()?;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty());
    Some(match line {
        Some(l) if Path::new(l).is_absolute() => PathBuf::from(l),
        Some(l) => exe_dir.join(l),
        None => exe_dir.join("data"),
    })
}

fn registry_root() -> Option<PathBuf> {
    read_registry_path(REGISTRY_KEY)
}

/// The `DataRoot` value under `HKCU\<key_path>`, if set and not empty.
#[cfg(windows)]
pub(super) fn read_registry_path(key_path: &str) -> Option<PathBuf> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    let key = HSTRING::from(key_path);
    let value = HSTRING::from(REGISTRY_VALUE);
    let mut size = 0u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &value,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if rc.is_err() || size < 2 {
        return None;
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if rc.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let s = String::from_utf16_lossy(&buf[..len]);
    let s = s.trim();
    (!s.is_empty()).then(|| PathBuf::from(s))
}

#[cfg(not(windows))]
pub(super) fn read_registry_path(_key_path: &str) -> Option<PathBuf> {
    None
}

/// The relocated data root stored in `HKCU\Software\Diarist\DataRoot`,
/// if any (what [`DataPaths::resolve`] uses when there is no portable marker).
pub fn registry_data_root() -> Option<PathBuf> {
    registry_root()
}

/// Points `HKCU\Software\Diarist\DataRoot` at `root`, or removes the
/// value (and the key when it is empty) for `None`, so the default root is
/// used again. The uninstaller reads the same value.
pub fn set_registry_root(root: Option<&Path>) -> Result<()> {
    write_registry_path(REGISTRY_KEY, root)
}

/// Sets the `DataRoot` value under `HKCU\<key_path>`, or removes it (and
/// the key when that leaves it empty) for `None`.
#[cfg(windows)]
pub(super) fn write_registry_path(key_path: &str, root: Option<&Path>) -> Result<()> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyW, RegDeleteKeyW, RegDeleteValueW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, REG_SZ,
    };

    let key_name = HSTRING::from(key_path);
    let value = HSTRING::from(REGISTRY_VALUE);
    let fail = |what: &str, rc: WIN32_ERROR| {
        CoreError::Models(format!(
            "cannot {what} HKCU\\{key_path}\\{REGISTRY_VALUE}: {}",
            rc.to_hresult().message()
        ))
    };
    let mut key = HKEY::default();
    let rc = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, &key_name, &mut key) };
    if rc.is_err() {
        return Err(fail("open", rc));
    }
    let rc = match root {
        Some(p) => {
            let wide: Vec<u16> = p
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let bytes: Vec<u8> = wide.iter().flat_map(|c| c.to_le_bytes()).collect();
            unsafe { RegSetValueExW(key, &value, None, REG_SZ, Some(&bytes)) }
        }
        None => match unsafe { RegDeleteValueW(key, &value) } {
            rc if rc == ERROR_FILE_NOT_FOUND => WIN32_ERROR(0),
            rc => rc,
        },
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    if rc.is_err() {
        return Err(fail("write", rc));
    }
    if root.is_none() {
        // Fails (and is ignored) while the key still holds other values.
        unsafe {
            let _ = RegDeleteKeyW(HKEY_CURRENT_USER, &key_name);
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub(super) fn write_registry_path(_key_path: &str, _root: Option<&Path>) -> Result<()> {
    Err(CoreError::Models(
        "a relocated data root is stored in the Windows registry".into(),
    ))
}

/// Free bytes available to the user on the volume holding `path` (the
/// nearest existing ancestor is asked). `None` when unknown.
pub fn free_disk_bytes(path: &Path) -> Option<u64> {
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        probe = probe.parent()?.to_path_buf();
    }
    free_bytes_impl(&probe)
}

#[cfg(windows)]
fn free_bytes_impl(path: &Path) -> Option<u64> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut free = 0u64;
    unsafe {
        GetDiskFreeSpaceExW(
            &HSTRING::from(path.as_os_str()),
            Some(&mut free),
            None,
            None,
        )
    }
    .ok()?;
    Some(free)
}

#[cfg(not(windows))]
fn free_bytes_impl(_path: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn children_and_relative_paths() {
        let p = DataPaths::at("D:/data");
        assert_eq!(p.models(), Path::new("D:/data").join("models"));
        assert_eq!(p.db(), Path::new("D:/data").join("diarist.db"));
        let file = p.models().join("nvidia").join("x.gguf");
        assert_eq!(p.relative(&file), "models/nvidia/x.gguf");
        assert_eq!(p.absolute("models/nvidia/x.gguf"), file);
        assert_eq!(
            p.relative(Path::new("E:/elsewhere/y.gguf")),
            "E:/elsewhere/y.gguf"
        );
        // Relative overrides become absolute, so stored paths resolve later.
        let p = DataPaths::at("D:/data").with_models_dir("models");
        assert!(p.models().is_absolute());
        assert_eq!(
            p.absolute(&p.relative(&p.models().join("a.gguf"))),
            p.models().join("a.gguf")
        );
    }

    /// A 264-character data folder (the KNOWN-ISSUES repro): the database
    /// opens, migrates and answers there, a backup into another long folder
    /// (a data-folder move) reopens, and the path stays in the plain form
    /// everywhere it is kept or shown.
    #[cfg(windows)]
    #[test]
    fn the_database_opens_and_backs_up_in_a_data_folder_longer_than_max_path() {
        use crate::engine::process::long_test_dir;
        use crate::store::{NewMeeting, RunStore, SqliteStore, Store};
        use crate::types::{MeetingStatus, Source};
        let tmp = tempfile::tempdir().unwrap();
        let root = long_test_dir(tmp.path(), 264);
        let p = DataPaths::at(&root);
        assert_eq!(p.db(), root.join("diarist.db"));
        assert!(!p.db().to_string_lossy().starts_with(r"\\"), "{:?}", p.db());
        let store = SqliteStore::open(p.db()).unwrap();
        assert_eq!(store.path(), Some(p.db().as_path()));
        assert_eq!(store.audio_dir("u"), Some(root.join("audio").join("u")));
        store
            .create_meeting(NewMeeting {
                title: "Deep".into(),
                source: Source::File,
                source_path: None,
                language: "en".into(),
                diarize_mic: false,
                started_at: 1_700_000_000_000,
                status: MeetingStatus::Done,
            })
            .unwrap();
        assert!(root.join("diarist.db").is_file());

        let to = long_test_dir(&tmp.path().join("to"), 270).join("diarist.db");
        store.backup(&to).unwrap();
        let copy = SqliteStore::open(&to).unwrap();
        assert_eq!(copy.list_meetings().unwrap()[0].title, "Deep");
        assert!(!PathBuf::from(format!("{}.tmp", to.display())).exists());
    }

    #[test]
    fn portable_marker_wins() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(portable_root(dir.path()), None);
        std::fs::write(dir.path().join(PORTABLE_MARKER), "").unwrap();
        assert_eq!(portable_root(dir.path()), Some(dir.path().join("data")));
        std::fs::write(dir.path().join(PORTABLE_MARKER), "\n  store \n").unwrap();
        assert_eq!(portable_root(dir.path()), Some(dir.path().join("store")));
    }

    #[test]
    fn resolves_somewhere_and_reports_free_space() {
        let p = DataPaths::resolve().unwrap();
        assert!(p.root.is_absolute());
        if cfg!(windows) {
            let free = free_disk_bytes(&p.root.join("not").join("yet")).unwrap();
            assert!(free > 0);
        }
    }
}
