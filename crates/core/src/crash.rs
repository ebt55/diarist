//! Local crash capture (docs/plans/11-hardening.md, 8a). Nothing here sends
//! anything anywhere: a crash leaves a text report in `<data root>\logs`,
//! the app points at the newest one at its next launch, and "Copy report"
//! puts a copy on the clipboard with the profile folder replaced by `~`
//! ([`crate::diagnostics::scrub_profile`]).
//!
//! - **Rust panics** (app and CLI): [`install`] sets a panic hook that writes
//!   `crash-<utc>.txt` with the version, Windows build, GPU and driver
//!   (gathered on a background thread at start-up, never at panic time), the
//!   message and location, a backtrace from `Backtrace::force_capture` and the
//!   last 200 lines of the app log. The text is built in a buffer reserved at
//!   start-up and written with one call; the hook takes no lock the panicking
//!   thread could hold (only `try_lock` on its own buffers).
//! - **Native crashes** (Windows): an unhandled-exception filter hands the
//!   exception to a writer thread started by [`install`] (so a stack
//!   overflow still has a stack to write from). It writes `crash-<utc>.txt`
//!   (exception code, address, module and offset) and `crash-<utc>.dmp`
//!   (`MiniDumpWriteDump`, in process), then ends the process. Rust aborts
//!   and other fail-fast exits (`__fastfail`) end the process before any
//!   handler runs and leave no report.
//! - **Engine exits**: the file adapter and the live server's supervisor call
//!   [`record_engine_exit`] / [`record_engine_exit_later`], which write
//!   `engine-crash-<utc>.txt` with the exit code and the stderr tail.
//!
//! [`prune`] keeps the newest [`KEEP`] reports of each kind (app, engine);
//! [`install`] runs it at launch. The launch notice is [`pending_notice`],
//! acknowledged with [`acknowledge`] (the name of the newest acknowledged
//! report in `logs\.crash-acknowledged`). Reports a process wrote and then
//! survived (a panic in a background thread) are acknowledged by
//! [`clean_exit`], so the notice only says "closed unexpectedly" when the
//! process did not exit normally.

use std::backtrace::Backtrace;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, TryLockError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use ts_rs::TS;

/// File name prefix of app reports (panics and native crashes).
pub const APP_PREFIX: &str = "crash-";
/// File name prefix of engine exit reports.
pub const ENGINE_PREFIX: &str = "engine-crash-";
/// Reports kept per kind (app, engine); older ones are deleted at launch.
pub const KEEP: usize = 5;
/// Marker file in `logs\` naming the newest acknowledged report.
pub const ACK_FILE: &str = ".crash-acknowledged";
/// Lines of the app log copied into a report.
pub const LOG_TAIL_LINES: usize = 200;
/// At most this much of the end of the app log is read for the tail.
const LOG_TAIL_BYTES: usize = 256 * 1024;
/// Report text reserved at install (a report with a long backtrace and 200
/// log lines fits; a bigger one grows the buffer).
const TEXT_RESERVE: usize = 512 * 1024;
/// Lines of a live server's log kept in its exit report.
const SERVE_TAIL_LINES: usize = 60;

/// What a report is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CrashKind {
    /// A Rust panic in the app or the CLI.
    Panic,
    /// An access violation, stack overflow or other native exception.
    Native,
    /// The speech engine (`nemo-speech`) exited without being asked to.
    Engine,
}

/// One report in `logs\` (Diagnostics, the launch notice).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct CrashReport {
    /// File name of the text report, e.g. `crash-20261016T142233Z.txt`
    /// (the `.dmp` when only the minidump was written).
    pub name: String,
    pub kind: CrashKind,
    /// When it was written (Unix ms, from the file name).
    pub at_ms: i64,
    /// The minidump next to it, e.g. `crash-20261016T142233Z.dmp`.
    pub dump: Option<String>,
    /// Size of the text report (or of the dump when there is no text).
    pub bytes: u64,
}

/// What [`install`] needs.
#[derive(Debug, Clone)]
pub struct CrashConfig {
    /// `<data root>\logs`; created when the first report is written.
    pub logs_dir: PathBuf,
    /// Product and version for the report, e.g. "Diarist 0.1.0-beta.2".
    pub app: String,
    /// The app log whose tail goes into a report (`None` for the CLI, which
    /// logs to the console).
    pub app_log: Option<PathBuf>,
    /// Delete all but the newest [`KEEP`] reports of each kind now.
    pub prune: bool,
}

// ---- time stamps -------------------------------------------------------------------

/// Days since 1970-01-01 to (year, month, day) (H. Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe as i64 + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// (year, month, day) to days since 1970-01-01.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = u64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + u64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn split_secs(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400) as u32;
    (y, mo, d, s / 3_600, s / 60 % 60, s % 60)
}

/// `20261016T142233Z`, the file-name form.
fn write_stamp(out: &mut String, secs: i64) {
    let (y, mo, d, h, mi, s) = split_secs(secs);
    let _ = write!(out, "{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z");
}

/// `2026-10-16 14:22:33 UTC`, the readable form.
fn write_utc(out: &mut String, secs: i64) {
    let (y, mo, d, h, mi, s) = split_secs(secs);
    let _ = write!(out, "{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC");
}

/// Unix ms of a `20261016T142233Z` stamp.
fn parse_stamp(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 16 || b[8] != b'T' || b[15] != b'Z' {
        return None;
    }
    if !b[..8].iter().chain(&b[9..15]).all(u8::is_ascii_digit) {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s[r].parse::<u32>().ok();
    let (y, mo, d) = (num(0..4)?, num(4..6)?, num(6..8)?);
    let (h, mi, sec) = (num(9..11)?, num(11..13)?, num(13..15)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let days = days_from_civil(i64::from(y), mo, d);
    Some((days * 86_400 + i64::from(h * 3_600 + mi * 60 + sec)) * 1_000)
}

// ---- report names --------------------------------------------------------------------

/// A parsed report stem: `crash-<stamp>[-N]` or `engine-crash-<stamp>[-N]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct StemKey {
    at_ms: i64,
    seq: u32,
}

/// `(engine?, key)` of a report stem (no extension).
fn parse_stem(stem: &str) -> Option<(bool, StemKey)> {
    let (engine, rest) = match stem.strip_prefix(ENGINE_PREFIX) {
        Some(r) => (true, r),
        None => (false, stem.strip_prefix(APP_PREFIX)?),
    };
    let (stamp, seq) = match rest.split_once('-') {
        Some((s, n)) => (s, n.parse::<u32>().ok().filter(|&n| n >= 2)?),
        None => (rest, 1),
    };
    Some((
        engine,
        StemKey {
            at_ms: parse_stamp(stamp)?,
            seq,
        },
    ))
}

/// `(stem, extension)` of a report file name (`.txt` or `.dmp`).
fn split_name(name: &str) -> Option<(&str, &str)> {
    let (stem, ext) = name.rsplit_once('.')?;
    matches!(ext, "txt" | "dmp").then_some((stem, ext))
}

/// The kind an app report's first line names.
fn kind_from_first_line(path: &Path) -> CrashKind {
    let mut buf = [0u8; 96];
    let n = File::open(path)
        .and_then(|mut f| f.read(&mut buf))
        .unwrap_or(0);
    if String::from_utf8_lossy(&buf[..n]).contains(": panic") {
        CrashKind::Panic
    } else {
        CrashKind::Native
    }
}

/// Every report in `logs`, newest first.
pub fn list_reports(logs: &Path) -> Vec<CrashReport> {
    let mut found = list_stems(logs);
    found.sort_by(|a, b| b.key.cmp(&a.key).then_with(|| a.stem.cmp(&b.stem)));
    found
        .into_iter()
        .map(|s| {
            let txt = format!("{}.txt", s.stem);
            let dmp = format!("{}.dmp", s.stem);
            let kind = if s.engine {
                CrashKind::Engine
            } else if s.has_txt {
                kind_from_first_line(&logs.join(&txt))
            } else {
                CrashKind::Native
            };
            let name = if s.has_txt { txt } else { dmp.clone() };
            let bytes = std::fs::metadata(logs.join(&name)).map_or(0, |m| m.len());
            CrashReport {
                name,
                kind,
                at_ms: s.key.at_ms,
                dump: s.has_dmp.then_some(dmp),
                bytes,
            }
        })
        .collect()
}

#[derive(Debug)]
struct FoundStem {
    stem: String,
    engine: bool,
    key: StemKey,
    has_txt: bool,
    has_dmp: bool,
}

fn list_stems(logs: &Path) -> Vec<FoundStem> {
    let Ok(dir) = std::fs::read_dir(logs) else {
        return Vec::new();
    };
    let mut out: Vec<FoundStem> = Vec::new();
    for entry in dir.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some((stem, ext)) = split_name(name) else {
            continue;
        };
        let Some((engine, key)) = parse_stem(stem) else {
            continue;
        };
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let i = match out.iter().position(|f| f.stem == stem) {
            Some(i) => i,
            None => {
                out.push(FoundStem {
                    stem: stem.to_owned(),
                    engine,
                    key,
                    has_txt: false,
                    has_dmp: false,
                });
                out.len() - 1
            }
        };
        if ext == "txt" {
            out[i].has_txt = true;
        } else {
            out[i].has_dmp = true;
        }
    }
    out
}

/// Deletes all but the newest `keep` reports of each kind (app, engine),
/// text and minidump together. Returns the deleted file names.
pub fn prune(logs: &Path, keep: usize) -> Vec<String> {
    let mut stems = list_stems(logs);
    stems.sort_by_key(|s| std::cmp::Reverse(s.key));
    let mut deleted = Vec::new();
    for engine in [false, true] {
        for s in stems.iter().filter(|s| s.engine == engine).skip(keep) {
            for (has, ext) in [(s.has_txt, "txt"), (s.has_dmp, "dmp")] {
                let name = format!("{}.{ext}", s.stem);
                if has && std::fs::remove_file(logs.join(&name)).is_ok() {
                    deleted.push(name);
                }
            }
        }
    }
    deleted
}

/// The newest acknowledged report (`None`: nothing acknowledged yet).
fn acknowledged(logs: &Path) -> Option<StemKey> {
    let text = std::fs::read_to_string(logs.join(ACK_FILE)).ok()?;
    let stem = text.trim();
    let stem = split_name(stem).map_or(stem, |(s, _)| s);
    parse_stem(stem).map(|(_, k)| k)
}

/// The newest app report (panic or native crash) newer than the
/// acknowledged one: what the launch notice shows. Engine exits are only
/// listed in Diagnostics (the app did not close).
pub fn pending_notice(logs: &Path) -> Option<CrashReport> {
    let ack = acknowledged(logs);
    list_reports(logs)
        .into_iter()
        .filter(|r| r.kind != CrashKind::Engine)
        .find(|r| {
            let key = split_name(&r.name)
                .and_then(|(stem, _)| parse_stem(stem))
                .map(|(_, k)| k);
            key.is_some_and(|k| ack.is_none_or(|a| k > a))
        })
}

/// Marks `name` (a report file name or stem) and everything older as seen.
/// Never moves the mark backwards.
pub fn acknowledge(logs: &Path, name: &str) -> std::io::Result<()> {
    let stem = split_name(name).map_or(name, |(s, _)| s);
    let Some((_, key)) = parse_stem(stem) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("not a crash report: {name}"),
        ));
    };
    if acknowledged(logs).is_some_and(|a| a >= key) {
        return Ok(());
    }
    std::fs::create_dir_all(logs)?;
    std::fs::write(logs.join(ACK_FILE), format!("{stem}\n"))
}

/// A report's text for "Copy report", with the profile folder as `~`.
/// `name` must be a report file name in `logs` (no path).
pub fn read_report(logs: &Path, name: &str) -> std::io::Result<String> {
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("not a crash report: {name}"),
        )
    };
    let (stem, ext) = split_name(name).ok_or_else(invalid)?;
    if name.contains(['/', '\\', ':']) || parse_stem(stem).is_none() {
        return Err(invalid());
    }
    if ext == "dmp" {
        return Ok(format!(
            "{name} is a minidump (binary); its text report was not written. \
             Attach the file itself if you are asked for it."
        ));
    }
    let bytes = std::fs::read(logs.join(name))?;
    Ok(crate::diagnostics::scrub_profile(&String::from_utf8_lossy(
        &bytes,
    )))
}

// ---- the installed state ----------------------------------------------------------------

/// Buffers reserved at install so a report is built without growing them.
struct Scratch {
    text: String,
    tail: Vec<u8>,
}

impl Scratch {
    fn new(text: usize) -> Self {
        Self {
            text: String::with_capacity(text),
            tail: Vec::with_capacity(LOG_TAIL_BYTES),
        }
    }
}

struct State {
    logs: PathBuf,
    app: String,
    app_log: Option<PathBuf>,
    /// `diarist.exe, pid 1234, started 2026-10-16 14:00:01 UTC, exe base 0x7ff6a0000000`.
    process: String,
    /// `os:`, `gpu:` and `adapters:` lines, filled in by a background thread.
    system: OnceLock<String>,
    scratch: Mutex<Scratch>,
    /// Stems this process wrote ([`clean_exit`]).
    written: Mutex<Vec<String>>,
}

impl State {
    fn new(cfg: &CrashConfig) -> Self {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "?".into());
        let mut process = format!("{exe}, pid {}, started ", std::process::id());
        write_utc(&mut process, now_secs());
        if let Some(base) = exe_base() {
            let _ = write!(process, ", exe base {base:#x}");
        }
        Self {
            logs: cfg.logs_dir.clone(),
            app: cfg.app.clone(),
            app_log: cfg.app_log.clone(),
            process,
            system: OnceLock::new(),
            scratch: Mutex::new(Scratch::new(TEXT_RESERVE)),
            written: Mutex::new(Vec::new()),
        }
    }

    fn note_written(&self, stem: &str) {
        if let Ok(mut w) = self.written.try_lock() {
            w.push(stem.to_owned());
        }
    }
}

static STATE: OnceLock<State> = OnceLock::new();
/// A report is being written (a second panic meanwhile only gets the default hook).
static IN_REPORT: AtomicBool = AtomicBool::new(false);

/// Runs `f` on the reserved buffers, or on fresh ones when another thread
/// holds them (two crashes at once).
fn with_scratch<R>(st: &State, f: impl FnOnce(&mut Scratch) -> R) -> R {
    match st.scratch.try_lock() {
        Ok(mut g) => f(&mut g),
        Err(TryLockError::Poisoned(p)) => f(&mut p.into_inner()),
        Err(TryLockError::WouldBlock) => f(&mut Scratch::new(64 * 1024)),
    }
}

/// The `os:`, `gpu:` and `adapters:` lines.
fn system_lines() -> String {
    let mut s = String::new();
    let os = crate::diagnostics::os_version();
    let _ = writeln!(s, "os: {}", if os.is_empty() { "?" } else { &os });
    match crate::gpu::gpu_info(0) {
        Some(g) => {
            let _ = writeln!(
                s,
                "gpu: {} | driver {} | CUDA {} | {} MB VRAM",
                g.name,
                if g.driver_version.is_empty() {
                    "?"
                } else {
                    &g.driver_version
                },
                g.cuda_version.as_deref().unwrap_or("?"),
                g.vram_total_mb
            );
        }
        None => {
            let _ = writeln!(s, "gpu: no NVIDIA GPU found (NVML not available)");
        }
    }
    let adapters = crate::gpu::adapters();
    if !adapters.is_empty() {
        let list: Vec<String> = adapters
            .iter()
            .map(|a| format!("{} ({}, {} MB)", a.name, a.vendor, a.vram_mb))
            .collect();
        let _ = writeln!(s, "adapters: {}", list.join("; "));
    }
    s
}

/// Turns on crash capture for this process: the panic hook, the native
/// exception filter (Windows), engine exit reports, and (with
/// `cfg.prune`) deletes old reports. Call once, early; later calls do
/// nothing. Returns the file names [`prune`] deleted.
pub fn install(cfg: CrashConfig) -> Vec<String> {
    if STATE.get().is_some() {
        return Vec::new();
    }
    let pruned = if cfg.prune {
        prune(&cfg.logs_dir, KEEP)
    } else {
        Vec::new()
    };
    if STATE.set(State::new(&cfg)).is_err() {
        return pruned;
    }
    // NVML and the registry take a moment: never on the caller's thread.
    let _ = std::thread::Builder::new()
        .name("crash-sysinfo".into())
        .spawn(|| {
            if let Some(st) = STATE.get() {
                let _ = st.system.set(system_lines());
            }
        });
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        on_panic(info);
        previous(info);
    }));
    #[cfg(windows)]
    native::install();
    pruned
}

/// The process exits normally: reports it wrote and survived (a panic in a
/// background thread) do not make the next launch say "closed
/// unexpectedly". Reports from other processes stay pending.
pub fn clean_exit() {
    let Some(st) = STATE.get() else { return };
    let written = match st.written.lock() {
        Ok(w) => w.clone(),
        Err(p) => p.into_inner().clone(),
    };
    if written.is_empty() {
        return;
    }
    let ack = acknowledged(&st.logs);
    let mut newest: Option<(StemKey, String)> = None;
    for s in list_stems(&st.logs).into_iter().filter(|s| !s.engine) {
        if ack.is_some_and(|a| s.key <= a) {
            continue;
        }
        if !written.contains(&s.stem) {
            return; // another process crashed meanwhile: keep its notice
        }
        if newest.as_ref().is_none_or(|(k, _)| s.key > *k) {
            newest = Some((s.key, s.stem));
        }
    }
    if let Some((_, stem)) = newest {
        let _ = acknowledge(&st.logs, &stem);
    }
}

// ---- writing reports ------------------------------------------------------------------

/// Creates `<prefix><stamp>[-N].txt` (no file with that stem exists yet)
/// and returns its stem and handle.
fn create_report(logs: &Path, prefix: &str) -> std::io::Result<(String, File)> {
    std::fs::create_dir_all(logs)?;
    let mut base = String::with_capacity(48);
    base.push_str(prefix);
    write_stamp(&mut base, now_secs());
    for seq in 1..100u32 {
        let stem = if seq == 1 {
            base.clone()
        } else {
            format!("{base}-{seq}")
        };
        if logs.join(format!("{stem}.dmp")).exists() {
            continue;
        }
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(logs.join(format!("{stem}.txt")))
        {
            Ok(f) => return Ok((stem, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other(
        "too many crash reports in one second",
    ))
}

/// The lines every report starts with.
fn write_header(out: &mut String, st: &State, title: &str, now: i64) {
    let _ = writeln!(out, "{title}");
    out.push_str("time: ");
    write_utc(out, now);
    out.push('\n');
    let _ = writeln!(out, "app: {} ({})", st.app, st.process);
    match st.system.get() {
        Some(s) => out.push_str(s),
        None => out.push_str("os, gpu: not gathered yet (within a second of start-up)\n"),
    }
}

/// The last `lines` lines of `path`, read into `buf` (reserved).
fn write_log_tail(out: &mut String, path: Option<&Path>, buf: &mut Vec<u8>, lines: usize) {
    let Some(path) = path else {
        out.push_str("\napp log: none (this process logs to its console)\n");
        return;
    };
    buf.clear();
    let read = File::open(path).and_then(|mut f| {
        let len = f.metadata()?.len();
        let start = len.saturating_sub(LOG_TAIL_BYTES as u64);
        f.seek(SeekFrom::Start(start))?;
        f.take(LOG_TAIL_BYTES as u64).read_to_end(buf)?;
        Ok(start > 0)
    });
    match read {
        Err(e) => {
            let _ = writeln!(out, "\napp log {}: not readable ({e})", path.display());
        }
        Ok(cut) => {
            let mut text: &[u8] = buf;
            if cut {
                // Drop the partial first line.
                let nl = text.iter().position(|&b| b == b'\n').map_or(0, |i| i + 1);
                text = &text[nl..];
            }
            while text.last().is_some_and(|b| matches!(b, b'\n' | b'\r')) {
                text = &text[..text.len() - 1];
            }
            let mut start = 0;
            let mut seen = 0;
            for (i, &b) in text.iter().enumerate().rev() {
                if b == b'\n' {
                    seen += 1;
                    if seen == lines {
                        start = i + 1;
                        break;
                    }
                }
            }
            let tail = &text[start..];
            let count = if tail.is_empty() {
                0
            } else {
                tail.iter().filter(|&&b| b == b'\n').count() + 1
            };
            let _ = writeln!(out, "\nlast {count} lines of {}:", path.display());
            out.push_str(&String::from_utf8_lossy(tail));
            out.push('\n');
        }
    }
}

/// The text of a panic report.
#[allow(clippy::too_many_arguments)]
fn panic_text(
    out: &mut String,
    tail_buf: &mut Vec<u8>,
    st: &State,
    message: &str,
    location: &str,
    thread: &str,
    backtrace: &dyn std::fmt::Display,
    now: i64,
) {
    write_header(out, st, "Diarist crash report: panic", now);
    let _ = writeln!(out, "thread: {thread}");
    let _ = writeln!(out, "message: {message}");
    let _ = writeln!(out, "location: {location}");
    let _ = writeln!(out, "\nbacktrace:\n{backtrace:#}");
    write_log_tail(out, st.app_log.as_deref(), tail_buf, LOG_TAIL_LINES);
}

fn on_panic(info: &std::panic::PanicHookInfo<'_>) {
    let Some(st) = STATE.get() else { return };
    if IN_REPORT.swap(true, Ordering::SeqCst) {
        return;
    }
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
        .unwrap_or("(a panic without a message)");
    let mut location = String::with_capacity(160);
    match info.location() {
        Some(l) => {
            let _ = write!(location, "{l}");
        }
        None => location.push('?'),
    }
    let thread = std::thread::current();
    let backtrace = Backtrace::force_capture();
    if let Ok((stem, mut file)) = create_report(&st.logs, APP_PREFIX) {
        with_scratch(st, |s| {
            s.text.clear();
            panic_text(
                &mut s.text,
                &mut s.tail,
                st,
                message,
                &location,
                thread.name().unwrap_or("(unnamed)"),
                &backtrace,
                now_secs(),
            );
            let _ = file.write_all(s.text.as_bytes());
        });
        st.note_written(&stem);
    }
    IN_REPORT.store(false, Ordering::SeqCst);
}

/// The name of a few NTSTATUS and exception codes seen in crashes.
pub fn exception_name(code: u32) -> Option<&'static str> {
    Some(match code {
        0xC000_0005 => "access violation",
        0xC000_0006 => "in-page error (a mapped file could not be read)",
        0xC000_001D => "illegal instruction",
        0xC000_0094 => "integer division by zero",
        0xC000_0096 => "privileged instruction",
        0xC000_00FD => "stack overflow",
        0xC000_0135 => "a required DLL was not found",
        0xC000_0139 => "a DLL entry point was not found",
        0xC000_0142 => "a DLL failed to initialise",
        0xC000_013A => "terminated by Ctrl+C",
        0xC000_0374 => "heap corruption",
        0xC000_0409 => "fail-fast exit (stack buffer overrun or abort)",
        0xC000_0417 => "invalid C runtime parameter",
        0xC000_0420 => "assertion failure",
        0xC000_0602 => "fail-fast exception",
        0xC06D_007E => "delay-loaded module not found",
        0xC06D_007F => "delay-loaded procedure not found",
        0x4000_0015 => "fatal application exit (abort)",
        0x8000_0003 => "breakpoint",
        0xE06D_7363 => "unhandled C++ exception",
        _ => return None,
    })
}

/// `exit code: 0xc0000005 (access violation)` from an `ExitStatus` text.
pub fn describe_exit(exit: &str) -> String {
    let hex = exit
        .split_once("0x")
        .map(|(_, h)| {
            h.chars()
                .take_while(char::is_ascii_hexdigit)
                .collect::<String>()
        })
        .and_then(|h| u32::from_str_radix(&h, 16).ok());
    match hex.and_then(exception_name) {
        Some(name) => format!("{exit} ({name})"),
        None => exit.to_owned(),
    }
}

/// The text of an engine exit report.
fn engine_text(
    out: &mut String,
    tail_buf: &mut Vec<u8>,
    st: &State,
    engine: &str,
    exit: &str,
    stderr: &str,
    now: i64,
) {
    write_header(out, st, "Diarist engine exit report", now);
    let _ = writeln!(out, "engine: {engine}");
    let _ = writeln!(out, "exit: {}", describe_exit(exit));
    let stderr = stderr.trim_end();
    if stderr.is_empty() {
        out.push_str("\nstderr: (empty)\n");
    } else {
        out.push_str("\nstderr (last lines):\n");
        for l in stderr.lines() {
            let _ = writeln!(out, "  {l}");
        }
    }
    write_log_tail(out, st.app_log.as_deref(), tail_buf, LOG_TAIL_LINES);
}

/// Persists an engine exit nobody asked for as `engine-crash-<utc>.txt`:
/// `engine` names the process (`nemo-speech transcribe`), `exit` is the
/// `ExitStatus` text, `stderr` its tail. Does nothing unless [`install`]
/// ran (tests, library use). Returns the report's path.
pub fn record_engine_exit(engine: &str, exit: &str, stderr: &str) -> Option<PathBuf> {
    let st = STATE.get()?;
    let (stem, mut file) = create_report(&st.logs, ENGINE_PREFIX).ok()?;
    let mut text = String::with_capacity(16 * 1024);
    let mut tail = Vec::new();
    engine_text(&mut text, &mut tail, st, engine, exit, stderr, now_secs());
    file.write_all(text.as_bytes()).ok()?;
    st.note_written(&stem);
    let path = st.logs.join(format!("{stem}.txt"));
    let _ = prune(&st.logs, KEEP);
    Some(path)
}

/// Like [`record_engine_exit`] for a process whose stderr went to `log`
/// (the live server's `serve.log`), on a short-lived thread so the caller
/// (holding the supervisor's lock) does no file I/O.
pub fn record_engine_exit_later(engine: &'static str, exit: String, log: Option<PathBuf>) {
    if STATE.get().is_none() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("engine-exit-report".into())
        .spawn(move || {
            let stderr = log
                .as_deref()
                .map(|p| read_tail_lines(p, SERVE_TAIL_LINES))
                .unwrap_or_default();
            let _ = record_engine_exit(engine, &exit, &stderr);
        });
}

/// The last `lines` lines of a text file (at most 64 KB are read).
pub fn read_tail_lines(path: &Path, lines: usize) -> String {
    const MAX: u64 = 64 * 1024;
    let mut buf = Vec::new();
    let ok = File::open(path).and_then(|mut f| {
        let len = f.metadata()?.len();
        f.seek(SeekFrom::Start(len.saturating_sub(MAX)))?;
        f.take(MAX).read_to_end(&mut buf)
    });
    if ok.is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let all: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// Base address of the module holding this code (the exe: diarist-core is
/// linked statically), so backtrace addresses can be turned into offsets.
fn exe_base() -> Option<usize> {
    #[cfg(windows)]
    {
        let (_, base) = native::module_of(exe_base as *const () as usize);
        (base != 0).then_some(base)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
mod native {
    //! The unhandled-exception filter and its writer thread.

    use std::ffi::c_void;
    use std::fmt::Write as _;
    use std::io::Write as _;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Diagnostics::Debug::{
        MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump,
        SetUnhandledExceptionFilter, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_EXECUTE_HANDLER,
        EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MINIDUMP_TYPE,
    };
    use windows::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION};
    use windows::Win32::System::ProcessStatus::K32GetMappedFileNameW;
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
    };

    use super::{create_report, with_scratch, write_header, write_log_tail, State, APP_PREFIX};

    static WRITER: OnceLock<std::thread::Thread> = OnceLock::new();
    static WRITER_TID: AtomicU32 = AtomicU32::new(0);
    static POINTERS: AtomicPtr<EXCEPTION_POINTERS> = AtomicPtr::new(std::ptr::null_mut());
    static FAULT_TID: AtomicU32 = AtomicU32::new(0);
    static CLAIMED: AtomicBool = AtomicBool::new(false);
    static DONE: AtomicBool = AtomicBool::new(false);
    /// The faulting thread waits this long for the report, then lets
    /// Windows handle the exception as if there were no filter.
    const WAIT: Duration = Duration::from_secs(20);

    pub(super) fn install() {
        let spawned = std::thread::Builder::new()
            .name("crash-writer".into())
            .stack_size(1 << 20)
            .spawn(writer);
        let Ok(handle) = spawned else { return };
        if WRITER.set(handle.thread().clone()).is_err() {
            return;
        }
        unsafe {
            SetUnhandledExceptionFilter(Some(filter));
        }
    }

    /// Runs on the faulting thread (possibly at the end of its stack): no
    /// allocation, no locks; the writer thread does the work.
    unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
        let tid = unsafe { GetCurrentThreadId() };
        if info.is_null()
            || tid == WRITER_TID.load(Ordering::SeqCst)
            || CLAIMED.swap(true, Ordering::SeqCst)
        {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let Some(writer) = WRITER.get() else {
            return EXCEPTION_CONTINUE_SEARCH;
        };
        FAULT_TID.store(tid, Ordering::SeqCst);
        POINTERS.store(info.cast_mut(), Ordering::SeqCst);
        writer.unpark();
        let start = Instant::now();
        while !DONE.load(Ordering::SeqCst) {
            if start.elapsed() > WAIT {
                return EXCEPTION_CONTINUE_SEARCH;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Reported: end the process now (no second dump by Windows Error
        // Reporting, which could otherwise send one to Microsoft).
        EXCEPTION_EXECUTE_HANDLER
    }

    fn writer() {
        WRITER_TID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
        let info = loop {
            std::thread::park();
            let p = POINTERS.load(Ordering::SeqCst);
            if !p.is_null() {
                break p;
            }
        };
        if let Some(st) = super::STATE.get() {
            write_report(st, info, FAULT_TID.load(Ordering::SeqCst));
        }
        DONE.store(true, Ordering::SeqCst);
    }

    /// The file name and base address of the module mapped at `addr`
    /// (`VirtualQuery` and the mapped file name: no loader lock).
    pub(super) fn module_of(addr: usize) -> (String, usize) {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        let n = unsafe {
            VirtualQuery(
                Some(addr as *const c_void),
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if n == 0 || mbi.AllocationBase.is_null() {
            return ("?".into(), 0);
        }
        let base = mbi.AllocationBase as usize;
        let mut buf = [0u16; 520];
        let len =
            unsafe { K32GetMappedFileNameW(GetCurrentProcess(), base as *const c_void, &mut buf) }
                as usize;
        if len == 0 {
            return ("?".into(), base);
        }
        let full = String::from_utf16_lossy(&buf[..len.min(buf.len())]);
        let name = full.rsplit('\\').next().unwrap_or(&full).to_owned();
        (name, base)
    }

    fn write_dump(path: &Path, info: *const EXCEPTION_POINTERS, tid: u32) -> std::io::Result<u64> {
        use std::os::windows::io::AsRawHandle;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        let exception = MINIDUMP_EXCEPTION_INFORMATION {
            ThreadId: tid,
            ExceptionPointers: info.cast_mut(),
            ClientPointers: false.into(),
        };
        // Thread stacks and the module list, not the heap. Stacks can still
        // hold fragments of transcript text or the live server's key, which
        // no scrub can remove: the dump is never copied, only shown.
        let kind = MINIDUMP_TYPE(MiniDumpWithThreadInfo.0 | MiniDumpWithUnloadedModules.0);
        unsafe {
            MiniDumpWriteDump(
                GetCurrentProcess(),
                GetCurrentProcessId(),
                HANDLE(file.as_raw_handle()),
                kind,
                Some(&exception),
                None,
                None,
            )
        }
        .map_err(std::io::Error::other)?;
        Ok(file.metadata()?.len())
    }

    fn write_report(st: &State, info: *const EXCEPTION_POINTERS, tid: u32) {
        let rec = unsafe { &*(*info).ExceptionRecord };
        let code = rec.ExceptionCode.0 as u32;
        let addr = rec.ExceptionAddress as usize;
        let (module, base) = module_of(addr);
        let Ok((stem, mut file)) = create_report(&st.logs, APP_PREFIX) else {
            return;
        };
        let dump_name = format!("{stem}.dmp");
        with_scratch(st, |s| {
            let out = &mut s.text;
            out.clear();
            write_header(
                out,
                st,
                "Diarist crash report: native exception",
                super::now_secs(),
            );
            let _ = write!(
                out,
                "exception: {code:#010X} ({}",
                super::exception_name(code).unwrap_or("unknown exception")
            );
            if matches!(code, 0xC000_0005 | 0xC000_0006) && rec.NumberParameters >= 2 {
                let op = match rec.ExceptionInformation[0] {
                    0 => "reading",
                    1 => "writing",
                    8 => "executing",
                    _ => "accessing",
                };
                let _ = write!(out, ", {op} address {:#x}", rec.ExceptionInformation[1]);
            }
            out.push_str(")\n");
            let _ = writeln!(
                out,
                "address: {addr:#x} in {module} + {:#x}",
                addr.wrapping_sub(base)
            );
            let _ = writeln!(out, "thread id: {tid}");
            let _ = writeln!(out, "minidump: {dump_name} (next to this file)");
            write_log_tail(
                out,
                st.app_log.as_deref(),
                &mut s.tail,
                super::LOG_TAIL_LINES,
            );
            let _ = file.write_all(out.as_bytes());
        });
        st.note_written(&stem);
        if let Err(e) = write_dump(&st.logs.join(&dump_name), info, tid) {
            let _ = writeln!(file, "\nminidump not written: {e}");
            let _ = std::fs::remove_file(st.logs.join(&dump_name));
        }
        let _ = file.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(logs: &Path, app_log: Option<PathBuf>) -> State {
        let st = State::new(&CrashConfig {
            logs_dir: logs.to_path_buf(),
            app: "Diarist 9.9.9".into(),
            app_log,
            prune: false,
        });
        let _ = st
            .system
            .set("os: Windows 11 (build 26200)\ngpu: Test GPU | driver 1.0\n".into());
        st
    }

    #[test]
    fn stamps_round_trip() {
        // 2026-10-16 14:22:33 UTC
        let secs = 1_792_160_553;
        let mut s = String::new();
        write_stamp(&mut s, secs);
        assert_eq!(s, "20261016T142233Z");
        assert_eq!(parse_stamp(&s), Some(secs * 1_000));
        let mut u = String::new();
        write_utc(&mut u, secs);
        assert_eq!(u, "2026-10-16 14:22:33 UTC");
        for secs in [0, 951_782_400, 4_102_444_799, 1_709_164_800] {
            let mut s = String::new();
            write_stamp(&mut s, secs);
            assert_eq!(parse_stamp(&s), Some(secs * 1_000), "{s}");
        }
        assert_eq!(parse_stamp("20261316T142233Z"), None);
        assert_eq!(parse_stamp("2026101xT142233Z"), None);
    }

    #[test]
    fn report_names() {
        let k = |s| parse_stem(s);
        assert_eq!(k("crash-20261016T142233Z").map(|(e, _)| e), Some(false));
        assert_eq!(
            k("engine-crash-20261016T142233Z-3").map(|(e, key)| (e, key.seq)),
            Some((true, 3))
        );
        for bad in [
            "crash-acknowledged",
            ".crash-acknowledged",
            "crash-20261016T142233Z-1",
            "crash-20261016T142233Z-x",
            "crash-20261016",
            "diarist",
        ] {
            assert_eq!(k(bad), None, "{bad}");
        }
        assert!(
            k("crash-20261016T142233Z-10").unwrap().1 > k("crash-20261016T142233Z-9").unwrap().1
        );
        assert_eq!(split_name("crash-x.log"), None);
    }

    fn touch(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn lists_prunes_and_acknowledges() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path();
        assert!(list_reports(logs).is_empty());
        assert_eq!(pending_notice(logs), None);
        for day in 10..17 {
            touch(
                logs,
                &format!("crash-202610{day}T120000Z.txt"),
                "Diarist crash report: panic\n",
            );
            touch(logs, &format!("engine-crash-202610{day}T120000Z.txt"), "x");
        }
        touch(logs, "crash-20261016T120000Z.dmp", "MDMP");
        touch(
            logs,
            "crash-20261016T120000Z-2.txt",
            "Diarist crash report: native exception\n",
        );
        touch(logs, "diarist.log", "log");
        touch(logs, ACK_FILE, "nothing");
        let all = list_reports(logs);
        assert_eq!(all.len(), 15);
        assert_eq!(all[0].name, "crash-20261016T120000Z-2.txt");
        assert_eq!(all[0].kind, CrashKind::Native);
        assert_eq!(all[1].kind, CrashKind::Panic);
        assert_eq!(all[1].dump.as_deref(), Some("crash-20261016T120000Z.dmp"));
        assert!(all.iter().any(|r| r.kind == CrashKind::Engine));

        let deleted = prune(logs, KEEP);
        // 8 app stems and 7 engine stems: 3 + 2 deleted, the oldest first.
        assert_eq!(deleted.len(), 5, "{deleted:?}");
        assert!(deleted.contains(&"crash-20261010T120000Z.txt".to_owned()));
        assert!(deleted.contains(&"engine-crash-20261011T120000Z.txt".to_owned()));
        assert!(!deleted.iter().any(|d| d.contains("20261016")));
        assert!(logs.join("crash-20261016T120000Z.dmp").is_file());
        assert!(logs.join("diarist.log").is_file());
        assert_eq!(list_reports(logs).len(), 10);

        // Nothing acknowledged (the marker is unreadable): the newest app report.
        let n = pending_notice(logs).unwrap();
        assert_eq!(n.name, "crash-20261016T120000Z-2.txt");
        acknowledge(logs, "crash-20261015T120000Z").unwrap();
        assert_eq!(pending_notice(logs).unwrap().name, n.name);
        acknowledge(logs, &n.name).unwrap();
        assert_eq!(pending_notice(logs), None);
        // Never backwards.
        acknowledge(logs, "crash-20261014T120000Z.txt").unwrap();
        assert_eq!(pending_notice(logs), None);
        // Engine exits never raise the notice.
        touch(logs, "engine-crash-20261017T120000Z.txt", "x");
        assert_eq!(pending_notice(logs), None);
        touch(logs, "crash-20261017T120000Z.dmp", "MDMP");
        let n = pending_notice(logs).unwrap();
        assert_eq!(
            (n.name.as_str(), n.kind),
            ("crash-20261017T120000Z.dmp", CrashKind::Native)
        );
        assert!(acknowledge(logs, "../evil").is_err());
    }

    #[test]
    fn read_report_checks_the_name_and_scrubs_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path();
        let profile =
            crate::diagnostics::profile_dir().unwrap_or_else(|| r"C:\Users\someone".into());
        touch(
            logs,
            "crash-20261016T120000Z.txt",
            &format!("Diarist crash report: panic\nlog: {profile}\\AppData\\Local\\app.diarist.desktop\\logs\n"),
        );
        let text = read_report(logs, "crash-20261016T120000Z.txt").unwrap();
        if crate::diagnostics::profile_dir().is_some() {
            assert!(text.contains(r"log: ~\AppData\Local"), "{text}");
            assert!(!text.contains(&profile));
        }
        for bad in [
            "../crash-20261016T120000Z.txt",
            "diarist.log",
            "C:crash-20261016T120000Z.txt",
            "x.txt",
        ] {
            assert!(read_report(logs, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn panic_report_has_header_message_backtrace_and_log_tail() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("diarist.log");
        let lines: Vec<String> = (1..=300).map(|i| format!("log line {i}")).collect();
        std::fs::write(&log, lines.join("\n") + "\n").unwrap();
        let st = state(dir.path(), Some(log.clone()));
        let mut out = String::new();
        let mut buf = Vec::new();
        panic_text(
            &mut out,
            &mut buf,
            &st,
            "index out of bounds",
            "src/x.rs:1:2",
            "main",
            &"   0: frame",
            1_792_160_553,
        );
        for want in [
            "Diarist crash report: panic\ntime: 2026-10-16 14:22:33 UTC\napp: Diarist 9.9.9 (",
            "os: Windows 11 (build 26200)\ngpu: Test GPU | driver 1.0\n",
            "thread: main\nmessage: index out of bounds\nlocation: src/x.rs:1:2\n",
            "backtrace:\n   0: frame",
            "last 200 lines of ",
            "log line 101\n",
            "log line 300\n",
        ] {
            assert!(out.contains(want), "{want:?} missing from:\n{out}");
        }
        assert!(!out.contains("log line 100\n"));
        // No app log (the CLI).
        let st = state(dir.path(), None);
        out.clear();
        panic_text(&mut out, &mut buf, &st, "m", "l", "t", &"bt", 0);
        assert!(out.contains("app log: none"));
        assert_eq!(kind_from_first_line(&log), CrashKind::Native);
    }

    #[test]
    fn engine_report_names_the_exit() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path(), None);
        let mut out = String::new();
        engine_text(
            &mut out,
            &mut Vec::new(),
            &st,
            "nemo-speech transcribe",
            "exit code: 0xc0000005",
            "[nemo-speech] started\nboom\n",
            0,
        );
        assert!(out.starts_with("Diarist engine exit report\n"), "{out}");
        assert!(out.contains(
            "engine: nemo-speech transcribe\nexit: exit code: 0xc0000005 (access violation)\n"
        ));
        assert!(out.contains("stderr (last lines):\n  [nemo-speech] started\n  boom\n"));
        assert_eq!(describe_exit("exit code: 3"), "exit code: 3");
        assert_eq!(
            describe_exit("exit code: 0xc0000409"),
            "exit code: 0xc0000409 (fail-fast exit (stack buffer overrun or abort))"
        );
        let log = dir.path().join("serve.log");
        std::fs::write(&log, "a\n\nb\nc\n").unwrap();
        assert_eq!(read_tail_lines(&log, 2), "b\nc");
        // Not installed in tests: nothing is written.
        assert_eq!(record_engine_exit("x", "exit code: 1", ""), None);
    }

    #[test]
    fn create_report_never_reuses_a_stem() {
        let dir = tempfile::tempdir().unwrap();
        let (a, _) = create_report(dir.path(), APP_PREFIX).unwrap();
        let (b, _) = create_report(dir.path(), APP_PREFIX).unwrap();
        assert_ne!(a, b);
        assert!(parse_stem(&a).is_some() && parse_stem(&b).is_some());
        #[cfg(windows)]
        {
            let (name, base) = native::module_of(create_report as *const () as usize);
            assert!(
                base != 0 && name.to_ascii_lowercase().ends_with(".exe"),
                "{name}"
            );
        }
    }
}
