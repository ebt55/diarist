//! Crash capture for the CLI and the hidden `diarist-cli crash` verb that
//! reproduces each kind of report (docs/plans/11-hardening.md 8a):
//!
//! - `--panic`: a Rust panic, `logs\crash-<utc>.txt`.
//! - `--segv`: an access violation, `logs\crash-<utc>.txt` and `.dmp`.
//! - `--engine`: the engine crashing during a job, `logs\engine-crash-<utc>.txt`.
//!   A copy of this exe stands in for `nemo-speech.exe` in a scratch runtime
//!   folder; run as that (with [`STAND_IN_ENV`] set) it prints two stderr
//!   lines and exits with STATUS_ACCESS_VIOLATION, and the real file adapter
//!   (`NemoCliEngine`) runs it and writes the report.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use diarist_core::crash::{self, CrashConfig, CrashKind};
use diarist_core::engine::{Engine, EngineEvent, FileJob, NemoCliEngine};
use diarist_core::models::DataPaths;
use diarist_core::types::JobId;

/// Set for the stand-in engine of `crash --engine`.
const STAND_IN_ENV: &str = "DIARIST_CRASH_TEST_ENGINE";
/// STATUS_ACCESS_VIOLATION.
const ACCESS_VIOLATION: u32 = 0xC000_0005;

/// The data root `Ctx::open` will use (without opening the database).
fn data_paths(data_dir: Option<&Path>) -> Option<DataPaths> {
    match data_dir {
        Some(d) => Some(DataPaths::at(d.components().collect::<PathBuf>())),
        None => DataPaths::resolve().ok(),
    }
}

/// Crash reports for this process go to `<data root>\logs` (old ones are
/// pruned to the newest five). The CLI has no app log: its output is the
/// console.
pub fn install(data_dir: Option<&Path>) {
    let Some(paths) = data_paths(data_dir) else {
        return;
    };
    crash::install(CrashConfig {
        logs_dir: paths.logs(),
        app: format!("diarist-cli {}", diarist_core::VERSION),
        app_log: None,
        prune: true,
    });
}

/// This process is the stand-in `nemo-speech.exe` of `crash --engine`.
pub fn is_stand_in_engine() -> bool {
    std::env::var_os(STAND_IN_ENV).is_some_and(|v| v == "1")
        && std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.eq_ignore_ascii_case("nemo-speech")))
            .unwrap_or(false)
}

/// The stand-in engine: a little stderr, then the exit status of an access
/// violation (without a real fault, so Windows Error Reporting stays out).
pub fn stand_in_engine() -> ExitCode {
    eprintln!("[nemo-speech] transcribe session started (stand-in for diarist-cli crash --engine)");
    eprintln!("[nemo-speech] simulated fault while decoding");
    std::process::exit(ACCESS_VIOLATION as i32)
}

/// `diarist-cli crash --panic|--segv|--engine`.
pub fn run(data_dir: Option<&Path>, panic: bool, segv: bool, engine: bool) -> Result<(), String> {
    let paths = data_paths(data_dir).ok_or("cannot resolve the data root")?;
    let logs = paths.logs();
    if panic {
        eprintln!(
            "panicking on purpose; the report goes to {}",
            logs.display()
        );
        panic!("deliberate panic from `diarist-cli crash --panic`");
    }
    if segv {
        eprintln!(
            "writing to address 0x8 on purpose; the report and minidump go to {}",
            logs.display()
        );
        // SAFETY: none; this is the crash being tested.
        unsafe {
            std::ptr::write_volatile(std::hint::black_box(8usize) as *mut u32, 0xDEAD);
        }
        return Err("the access violation did not happen".into());
    }
    if engine {
        return engine_crash(&logs);
    }
    Err("pass --panic, --segv or --engine".into())
}

/// Runs a real file job on the stand-in engine and reports where the
/// engine exit report went.
fn engine_crash(logs: &Path) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("diarist-crash-engine-{}", std::process::id()));
    let result = engine_crash_in(&dir, logs);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn engine_crash_in(dir: &Path, logs: &Path) -> Result<(), String> {
    let err = |e: std::io::Error| e.to_string();
    std::fs::create_dir_all(dir.join("bin")).map_err(err)?;
    let exe = std::env::current_exe().map_err(err)?;
    std::fs::copy(&exe, dir.join("bin").join("nemo-speech.exe")).map_err(err)?;
    for f in ["asr.gguf", "diar.gguf", "input.wav"] {
        std::fs::write(dir.join(f), b"").map_err(err)?;
    }
    // The child inherits it; only a copy named nemo-speech.exe acts on it.
    std::env::set_var(STAND_IN_ENV, "1");
    let before: Vec<String> = engine_reports(logs);
    let engine = NemoCliEngine::new(dir, dir, dir.join("asr.gguf"), dir.join("diar.gguf"));
    let rx = engine
        .transcribe_file(FileJob {
            job_id: JobId::new(),
            wav_path: dir.join("input.wav"),
            diarize: true,
            language: "en".into(),
            diar_preset: Default::default(),
        })
        .map_err(|e| e.to_string())?;
    let mut failure = None;
    while let Ok(ev) = rx.recv_timeout(Duration::from_secs(60)) {
        match ev {
            EngineEvent::Error { code, message, .. } => {
                failure = Some(format!(
                    "{code:?}: {}",
                    message.lines().next().unwrap_or("")
                ));
                break;
            }
            EngineEvent::Done { .. } => break,
            _ => {}
        }
    }
    std::env::remove_var(STAND_IN_ENV);
    let failure = failure.ok_or("the stand-in engine did not fail")?;
    eprintln!("engine job failed as intended: {failure}");
    let new = engine_reports(logs)
        .into_iter()
        .find(|n| !before.contains(n));
    match new {
        Some(name) => {
            println!("{}", logs.join(name).display());
            Ok(())
        }
        None => Err(format!(
            "no engine-crash report appeared in {}",
            logs.display()
        )),
    }
}

fn engine_reports(logs: &Path) -> Vec<String> {
    crash::list_reports(logs)
        .into_iter()
        .filter(|r| r.kind == CrashKind::Engine)
        .map(|r| r.name)
        .collect()
}
