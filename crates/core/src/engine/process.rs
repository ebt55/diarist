//! Spawning `nemo-speech` the way research/08 section 0 requires: the
//! engine's own `bin` first on a reduced `PATH` (so no installed CUDA
//! toolkit's DLLs are picked up), `NEMO_SPEECH_MODEL_DIR` set to our models
//! folder (so nothing lands in the user cache), no console window, and the
//! executable in the extended-length form when its path is long, so a deep
//! data folder still starts the engine ([`exe_arg`]).

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::error::{CoreError, Result};

/// File name of the runtime binary inside `<engine>/bin`.
pub const EXE_NAME: &str = if cfg!(windows) {
    "nemo-speech.exe"
} else {
    "nemo-speech"
};

/// `<engine_dir>/bin/nemo-speech.exe`.
pub fn exe_path(engine_dir: &Path) -> PathBuf {
    engine_dir.join("bin").join(EXE_NAME)
}

/// True when `dir` holds an extracted runtime (`bin/nemo-speech.exe`).
pub fn is_engine_dir(dir: &Path) -> bool {
    exe_path(dir).is_file()
}

/// Paths at least this long get the `\\?\` prefix in [`path_arg`].
const LONG_PATH: usize = 240;

/// A file path as a runtime argument. nemo-speech 0.2.0 cannot open paths of
/// `MAX_PATH` (260) characters or more ("model file does not exist"), but
/// accepts the extended-length form, so long absolute paths are passed as
/// `\\?\C:\...` (normalised: backslashes, no `.` or `..`). Short paths, UNC
/// paths and other platforms pass through unchanged.
pub fn path_arg(path: &Path) -> OsString {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        if path.as_os_str().len() >= LONG_PATH {
            if let Ok(abs) = std::path::absolute(path) {
                let mut comps = abs.components();
                if let Some(Component::Prefix(p)) = comps.next() {
                    if matches!(p.kind(), Prefix::Disk(_)) {
                        let mut out = OsString::from(r"\\?\");
                        out.push(p.as_os_str());
                        for c in comps {
                            match c {
                                Component::Normal(n) => {
                                    out.push("\\");
                                    out.push(n);
                                }
                                Component::RootDir | Component::CurDir => {}
                                Component::ParentDir | Component::Prefix(_) => {
                                    return path.as_os_str().to_owned()
                                }
                            }
                        }
                        return out;
                    }
                }
            }
        }
    }
    path.as_os_str().to_owned()
}

/// `<engine>\bin;%SystemRoot%\System32;%SystemRoot%`.
pub fn reduced_path(engine_dir: &Path) -> OsString {
    let mut parts = vec![engine_dir.join("bin")];
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        parts.push(root.join("System32"));
        parts.push(root);
    } else {
        parts.push(PathBuf::from("/usr/bin"));
        parts.push(PathBuf::from("/bin"));
    }
    std::env::join_paths(parts).unwrap_or_default()
}

/// The executable as `Command` gets it. `CreateProcessW` refuses an
/// application name of `MAX_PATH` (260) characters or more in the plain
/// form ("the system cannot find the path specified", os error 3, even with
/// `LongPathsEnabled` set) but starts the extended-length form at any
/// length, and std passes a path with a separator as the application name
/// unchanged. So a deep data folder gets `\\?\C:\...\bin\nemo-speech.exe`
/// through [`path_arg`]; short paths are unchanged. Checked on Windows 11
/// with a 329-character path (plan 16, chunk C).
pub fn exe_arg(engine_dir: &Path) -> OsString {
    path_arg(&exe_path(engine_dir))
}

/// A `Command` for the runtime with the reduced `PATH`, the model directory
/// (when given), stdin closed and no console window. Global flags such as
/// `--json` are the caller's business. Starts at any path length
/// ([`exe_arg`]).
pub fn command(engine_dir: &Path, models_dir: Option<&Path>) -> Command {
    let mut cmd = Command::new(exe_arg(engine_dir));
    cmd.env("PATH", reduced_path(engine_dir));
    if let Some(dir) = models_dir {
        cmd.env("NEMO_SPEECH_MODEL_DIR", dir);
    }
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Runs `cmd` to completion with stdout and stderr captured, killing it
/// after `timeout`.
pub fn run_with_timeout(cmd: Command, timeout: Duration) -> Result<Output> {
    run_with_timeout_ticking(cmd, timeout, &mut || None)
}

/// [`run_with_timeout`], calling `tick` about every 250 ms while the child
/// runs (progress polling). When `tick` returns a reason, the child is
/// killed and the reason becomes the error.
pub fn run_with_timeout_ticking(
    mut cmd: Command,
    timeout: Duration,
    tick: &mut dyn FnMut() -> Option<String>,
) -> Result<Output> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| CoreError::Engine(format!("spawn {:?}: {e}", cmd.get_program())))?;
    let stdout = drain(&mut child, true);
    let stderr = drain(&mut child, false);
    let start = Instant::now();
    let mut last_tick = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if last_tick.elapsed() >= Duration::from_millis(250) {
            last_tick = Instant::now();
            if let Some(why) = tick() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CoreError::Engine(why));
            }
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CoreError::Engine(format!(
                "{:?} timed out after {} s",
                cmd.get_program(),
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Reads a child pipe on its own thread so a full pipe never blocks the child.
fn drain(child: &mut Child, stdout: bool) -> std::thread::JoinHandle<Vec<u8>> {
    let pipe: Option<Box<dyn Read + Send>> = if stdout {
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>)
    } else {
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>)
    };
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    })
}

/// The last `max` bytes of `s` (on a char boundary), trimmed.
pub fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s.trim();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_paths_get_the_extended_prefix() {
        let short = Path::new(r"C:\data\models\a.gguf");
        assert_eq!(path_arg(short), short.as_os_str());
        if cfg!(windows) {
            // `..` is resolved (GetFullPathNameW) before the prefix is added.
            let long = format!(r"C:/data/{}/x/../model.gguf", "d".repeat(250));
            let arg = path_arg(Path::new(&long)).to_string_lossy().into_owned();
            assert!(arg.starts_with(r"\\?\C:\data\"), "{arg}");
            assert!(arg.ends_with(r"\model.gguf") && !arg.contains('/'), "{arg}");
            assert!(!arg.contains(r"\x\"), "{arg}");
        }
    }

    /// A folder under `base` whose path is `len` characters long, made of
    /// nested 40-character segments (the KNOWN-ISSUES repro used 264).
    #[cfg(windows)]
    fn long_dir(base: &Path, len: usize) -> PathBuf {
        let mut dir = std::path::absolute(base).unwrap();
        let mut i = 0;
        while dir.as_os_str().len() < len {
            let left = len - dir.as_os_str().len() - 1;
            let seg: String = format!("d{i}{}", "x".repeat(60))
                .chars()
                .take(left.clamp(1, 40))
                .collect();
            dir.push(seg);
            i += 1;
        }
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(dir.as_os_str().len(), len, "{}", dir.display());
        dir
    }

    /// `engine\<name>` under a 264-character data root, linked to the real
    /// engine folder with a junction (`mklink /J` takes the `\\?\` form of a
    /// long link path). `None` when the link cannot be made.
    #[cfg(windows)]
    fn link_engine_under_long_root(base: &Path, engine: &Path) -> Option<PathBuf> {
        let root = long_dir(base, 264);
        std::fs::create_dir_all(root.join("engine")).unwrap();
        let name = engine.file_name()?;
        let link = root.join("engine").join(name);
        let made = Command::new("cmd")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(path_arg(&link))
            .arg(engine)
            .output()
            .is_ok_and(|o| o.status.success());
        (made && is_engine_dir(&link)).then_some(link)
    }

    #[cfg(windows)]
    #[test]
    fn starts_an_engine_whose_path_is_longer_than_max_path() {
        // A stand-in runtime: this test binary copied to
        // `<264-character root>\engine\fake\bin\nemo-speech.exe` (331
        // characters), which plain `Command::new` cannot start (os error 3).
        // Asked to list one test, it prints that test's name. (Copies of
        // System32 tools are refused on some machines, os error 5.)
        let tmp = tempfile::tempdir().unwrap();
        let engine = long_dir(tmp.path(), 264).join("engine").join("fake");
        std::fs::create_dir_all(engine.join("bin")).unwrap();
        std::fs::copy(std::env::current_exe().unwrap(), exe_path(&engine)).unwrap();
        assert!(exe_path(&engine).as_os_str().len() > 260);
        assert!(exe_arg(&engine).to_string_lossy().starts_with(r"\\?\"));
        let name = "engine::process::tests::tail_respects_char_boundaries";
        let mut cmd = command(&engine, Some(tmp.path()));
        cmd.args(["--list", "--exact", name]);
        let out = run_with_timeout(cmd, Duration::from_secs(60)).unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains(name),
            "{out:?}"
        );
        // Short paths are passed as they are.
        let short = Path::new(r"C:\e\x");
        assert_eq!(exe_arg(short), exe_path(short).as_os_str());
    }

    /// The real runtime's `doctor` from a 264-character data root, when
    /// `DIARIST_ENGINE_DIR` names an extracted runtime (it touches the GPU,
    /// so it is not run by default).
    #[cfg(windows)]
    #[test]
    fn real_doctor_from_a_long_data_root_when_available() {
        let Some(engine) = std::env::var_os("DIARIST_ENGINE_DIR").map(PathBuf::from) else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let link = link_engine_under_long_root(tmp.path(), &engine)
            .expect("mklink /J under the long root");
        assert!(exe_path(&link).as_os_str().len() > 300);
        let report = crate::models::doctor(&link).unwrap();
        assert!(!report.version.is_empty(), "{report:?}");
        // Remove the junction itself, never what it points to.
        std::fs::remove_dir(&link).unwrap();
        assert!(is_engine_dir(&engine));
    }

    #[test]
    fn reduced_path_puts_the_engine_first() {
        let p = reduced_path(Path::new("E:/engine/x"));
        let parts: Vec<PathBuf> = std::env::split_paths(&p).collect();
        assert_eq!(parts[0], Path::new("E:/engine/x").join("bin"));
        assert_eq!(parts.len(), 3);
        assert!(!is_engine_dir(Path::new("E:/definitely/not/here")));
    }

    #[test]
    fn tail_respects_char_boundaries() {
        assert_eq!(tail("  short ", 100), "short");
        let s = format!("{}é end", "x".repeat(10));
        assert_eq!(tail(&s, 5), "end");
        assert!(tail(&s, 6).ends_with("end"));
    }
}
