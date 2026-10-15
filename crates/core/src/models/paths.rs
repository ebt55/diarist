//! The data root and the folders under it (`docs/01`, "Data layout on disk").
//!
//! Resolution order:
//! 0. `MEET_ASSIST_DATA_DIR` (development and tests: run the app or the CLI
//!    against a scratch data root).
//! 1. Portable mode: a `portable.txt` next to the executable. The data root
//!    is `<exe dir>\data`, or the path on the file's first line (relative
//!    paths are taken from the exe dir).
//! 2. `HKCU\Software\meet-assist\DataRoot` (REG_SZ), set when the user moves it.
//! 3. `%LOCALAPPDATA%\app.meetassist.desktop` (Tauri's `app_local_data_dir`).

use std::path::{Component, Path, PathBuf};

use crate::error::{CoreError, Result};

/// Tauri bundle identifier: the default data root's folder name.
pub const APP_IDENTIFIER: &str = "app.meetassist.desktop";
/// Registry key (under HKCU) and value naming a relocated data root.
pub const REGISTRY_KEY: &str = r"Software\meet-assist";
pub const REGISTRY_VALUE: &str = "DataRoot";
/// Marker file next to the executable that turns on portable mode.
pub const PORTABLE_MARKER: &str = "portable.txt";
/// Environment variable that overrides the data root (development, tests).
pub const DATA_DIR_ENV: &str = "MEET_ASSIST_DATA_DIR";

/// Why the data root is where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataRootSource {
    /// Given explicitly (CLI `--data-dir`, `MEET_ASSIST_DATA_DIR`, tests).
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

    /// Uses `dir` instead of `<root>\models`.
    pub fn with_models_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.models_override = Some(dir.into());
        self
    }

    /// Uses `dir` instead of `<root>\engine` (a folder of runtimes, or one runtime).
    pub fn with_engine_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.engine_override = Some(dir.into());
        self
    }

    /// Resolves the data root (`MEET_ASSIST_DATA_DIR`, portable marker,
    /// registry, default).
    pub fn resolve() -> Result<Self> {
        if let Some(dir) = std::env::var_os(DATA_DIR_ENV).filter(|d| !d.is_empty()) {
            return Ok(Self::at(dir));
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
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .ok_or_else(|| CoreError::Models("cannot find %LOCALAPPDATA%".into()))?;
        Ok(Self {
            source: DataRootSource::Default,
            ..Self::at(local.join(APP_IDENTIFIER))
        })
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

    /// The SQLite database.
    pub fn db(&self) -> PathBuf {
        self.root.join("meet-assist.db")
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

#[cfg(windows)]
fn registry_root() -> Option<PathBuf> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    let key = HSTRING::from(REGISTRY_KEY);
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
fn registry_root() -> Option<PathBuf> {
    None
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
        assert_eq!(p.db(), Path::new("D:/data").join("meet-assist.db"));
        let file = p.models().join("nvidia").join("x.gguf");
        assert_eq!(p.relative(&file), "models/nvidia/x.gguf");
        assert_eq!(p.absolute("models/nvidia/x.gguf"), file);
        assert_eq!(
            p.relative(Path::new("E:/elsewhere/y.gguf")),
            "E:/elsewhere/y.gguf"
        );
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
