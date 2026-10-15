//! Spawning `nemo-speech` the way research/08 section 0 requires: the
//! engine's own `bin` first on a reduced `PATH` (so no installed CUDA
//! toolkit's DLLs are picked up), `NEMO_SPEECH_MODEL_DIR` set to our models
//! folder (so nothing lands in the user cache), no console window.

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

/// A `Command` for the runtime with the reduced `PATH`, the model directory
/// (when given), stdin closed and no console window. Global flags such as
/// `--json` are the caller's business.
pub fn command(engine_dir: &Path, models_dir: Option<&Path>) -> Command {
    let mut cmd = Command::new(exe_path(engine_dir));
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
pub fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<Output> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| CoreError::Engine(format!("spawn {:?}: {e}", cmd.get_program())))?;
    let stdout = drain(&mut child, true);
    let stderr = drain(&mut child, false);
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
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
