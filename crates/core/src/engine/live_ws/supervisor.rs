//! [`ServeSupervisor`]: owns the long-lived `nemo-speech serve` process
//! (research/08 section 4).
//!
//! Launch line, with `NEMO_SPEECH_HTTP_API_KEY` set to a per-launch random
//! key, the reduced `PATH` and `NEMO_SPEECH_MODEL_DIR`:
//!
//! `nemo-speech --json serve --asr-model <asr> --diar-model <diar> --device <device>
//!  --host 127.0.0.1 --port <free> --no-ui --asr.batching.state_arena_slots 2
//!  --asr.endpointing.enable=true`
//!
//! The listener opens only after the models load (4.6 to 5.9 s), so
//! readiness is `GET /ready` returning 200. A monitor thread restarts the
//! server with exponential backoff when it exits, gives up after five
//! launches in a row that never became ready, and stops it after an idle
//! timeout without sessions. [`ServeSupervisor::ensure_ready`] (re)starts
//! it on demand.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;
use ts_rs::TS;

use crate::engine::process;
use crate::error::{CoreError, Result};

/// How the server is launched.
#[derive(Debug, Clone)]
pub struct ServeConfig {
    /// Runtime folder (holds `bin\nemo-speech.exe`).
    pub engine_dir: PathBuf,
    /// `NEMO_SPEECH_MODEL_DIR`.
    pub models_dir: PathBuf,
    pub asr_model: PathBuf,
    pub diar_model: PathBuf,
    /// `cuda:0`, `vulkan:0`, `cpu`: the runtime package's
    /// ([`crate::models::device`]; the default from the folder name, else
    /// `auto`). The Vulkan and CPU packages refuse `cuda:0`.
    pub device: String,
    /// `--asr.batching.state_arena_slots` (one per live session).
    pub slots: u32,
    /// Stop the server after this long without sessions (`None`: never).
    pub idle_timeout: Option<Duration>,
    /// Give up on a launch that is not ready after this long.
    pub ready_timeout: Duration,
    /// The server's stdout and stderr are appended here (else discarded).
    pub log_path: Option<PathBuf>,
    /// Extra flags appended verbatim.
    pub extra_args: Vec<String>,
}

impl ServeConfig {
    pub fn new(
        engine_dir: impl Into<PathBuf>,
        models_dir: impl Into<PathBuf>,
        asr_model: impl Into<PathBuf>,
        diar_model: impl Into<PathBuf>,
    ) -> Self {
        let engine_dir: PathBuf = engine_dir.into();
        Self {
            device: crate::models::device::device_for_dir(&engine_dir).into(),
            engine_dir,
            models_dir: models_dir.into(),
            asr_model: asr_model.into(),
            diar_model: diar_model.into(),
            slots: 2,
            idle_timeout: Some(Duration::from_secs(30 * 60)),
            ready_timeout: Duration::from_secs(90),
            log_path: None,
            extra_args: Vec::new(),
        }
    }

    /// The argument list after the executable, for `port`.
    pub fn args(&self, port: u16) -> Vec<std::ffi::OsString> {
        let mut a: Vec<std::ffi::OsString> = vec![
            "--json".into(),
            "serve".into(),
            "--asr-model".into(),
            process::path_arg(&self.asr_model),
            "--diar-model".into(),
            process::path_arg(&self.diar_model),
        ];
        for s in [
            "--device".to_owned(),
            self.device.clone(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            port.to_string(),
            "--no-ui".into(),
            "--asr.batching.state_arena_slots".into(),
            self.slots.max(1).to_string(),
            "--asr.endpointing.enable=true".into(),
        ] {
            a.push(s.into());
        }
        a.extend(self.extra_args.iter().map(Into::into));
        a
    }
}

/// State of the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(export)]
pub enum ServeStatus {
    Stopped,
    /// Launched, waiting for `/ready`.
    Starting {
        port: u16,
    },
    Ready {
        port: u16,
    },
    /// Exited; relaunching after a backoff.
    Restarting {
        attempt: u32,
        last_error: String,
    },
    /// Gave up (five failed launches); `ensure_ready` tries again.
    Failed {
        message: String,
    },
}

/// What [`ServeSupervisor::status`] reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct ServeSnapshot {
    pub status: ServeStatus,
    /// When the server last became ready (`None` unless it is ready now).
    #[serde(skip)]
    #[ts(skip)]
    pub ready_since: Option<Instant>,
    /// How long it has been ready (`None` unless it is ready now).
    pub uptime_ms: Option<u64>,
    /// Restarts after the server had been ready (crashes it came back from).
    pub restarts: u32,
    /// Process id of the running server.
    pub pid: Option<u32>,
}

impl ServeSnapshot {
    pub fn is_ready(&self) -> bool {
        matches!(self.status, ServeStatus::Ready { .. })
    }
}

/// Where a ready server listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeEndpoint {
    pub port: u16,
    pub api_key: String,
}

impl ServeEndpoint {
    /// `ws://127.0.0.1:<port>/v1/audio/transcriptions/realtime`.
    pub fn realtime_url(&self) -> String {
        format!(
            "ws://127.0.0.1:{}/v1/audio/transcriptions/realtime",
            self.port
        )
    }
}

struct State {
    status: ServeStatus,
    child: Option<Child>,
    endpoint: Option<ServeEndpoint>,
    launched_at: Instant,
    /// Launches in a row that never became ready.
    failures: u32,
    /// Restarts after the server had been ready (diagnostics).
    restarts: u32,
    /// When the status last became `Ready`.
    ready_since: Option<Instant>,
    retry_at: Option<Instant>,
    want_running: bool,
    sessions: usize,
    idle_since: Instant,
    shutdown: bool,
}

struct Inner {
    cfg: ServeConfig,
    state: Mutex<State>,
    changed: Condvar,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Supervises one `nemo-speech serve`. Cheap to clone.
#[derive(Clone)]
pub struct ServeSupervisor {
    inner: Arc<Inner>,
}

/// Counts an open session for the idle timeout; dropping it ends the count.
pub struct SessionLease {
    inner: Weak<Inner>,
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            let mut st = inner.lock();
            st.sessions = st.sessions.saturating_sub(1);
            if st.sessions == 0 {
                st.idle_since = Instant::now();
            }
        }
    }
}

/// Launches that never became ready before giving up.
const MAX_FAILURES: u32 = 5;

impl ServeSupervisor {
    /// A supervisor with its monitor thread; nothing is launched until
    /// [`Self::start`] or [`Self::ensure_ready`].
    pub fn new(cfg: ServeConfig) -> Self {
        let inner = Arc::new(Inner {
            cfg,
            state: Mutex::new(State {
                status: ServeStatus::Stopped,
                child: None,
                endpoint: None,
                launched_at: Instant::now(),
                failures: 0,
                restarts: 0,
                ready_since: None,
                retry_at: None,
                want_running: false,
                sessions: 0,
                idle_since: Instant::now(),
                shutdown: false,
            }),
            changed: Condvar::new(),
        });
        let weak = Arc::downgrade(&inner);
        std::thread::Builder::new()
            .name("serve-supervisor".into())
            .spawn(move || monitor(weak))
            .expect("spawn serve supervisor");
        Self { inner }
    }

    pub fn config(&self) -> &ServeConfig {
        &self.inner.cfg
    }

    /// Starts the server in the background (no-op when running).
    pub fn start(&self) {
        let mut st = self.inner.lock();
        st.want_running = true;
        if matches!(st.status, ServeStatus::Failed { .. }) {
            st.failures = 0;
            st.status = ServeStatus::Stopped;
        }
        st.idle_since = Instant::now();
        self.inner.changed.notify_all();
    }

    /// Starts the server if needed and waits until it is ready.
    pub fn ensure_ready(&self, timeout: Duration) -> Result<ServeEndpoint> {
        self.start();
        let deadline = Instant::now() + timeout;
        let mut st = self.inner.lock();
        loop {
            match (&st.status, &st.endpoint) {
                (ServeStatus::Ready { .. }, Some(ep)) => return Ok(ep.clone()),
                (ServeStatus::Failed { message }, _) => {
                    return Err(CoreError::Engine(format!("nemo-speech serve: {message}")))
                }
                _ => {}
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(CoreError::Engine(format!(
                    "nemo-speech serve not ready after {} s ({:?})",
                    timeout.as_secs(),
                    st.status
                )));
            }
            st = self
                .inner
                .changed
                .wait_timeout(st, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// State, uptime, restart count and process id.
    pub fn status(&self) -> ServeSnapshot {
        let st = self.inner.lock();
        let ready = matches!(st.status, ServeStatus::Ready { .. });
        let ready_since = st.ready_since.filter(|_| ready);
        ServeSnapshot {
            status: st.status.clone(),
            ready_since,
            uptime_ms: ready_since.map(|t| t.elapsed().as_millis() as u64),
            restarts: st.restarts,
            pid: st.child.as_ref().map(Child::id),
        }
    }

    /// Restarts after the server had been ready once.
    pub fn restarts(&self) -> u32 {
        self.inner.lock().restarts
    }

    /// Counts a session until the lease is dropped.
    pub fn lease(&self) -> SessionLease {
        let mut st = self.inner.lock();
        st.sessions += 1;
        SessionLease {
            inner: Arc::downgrade(&self.inner),
        }
    }

    /// Stops the server (it stays stopped until started again).
    pub fn stop(&self) {
        let mut st = self.inner.lock();
        st.want_running = false;
        kill(&mut st);
        st.status = ServeStatus::Stopped;
        self.inner.changed.notify_all();
    }

    /// The process id of the running server, if any.
    pub fn pid(&self) -> Option<u32> {
        self.inner.lock().child.as_ref().map(Child::id)
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // The last handle is gone (the monitor holds only a weak one): the
        // server must not outlive us.
        let st = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        st.shutdown = true;
        kill(st);
    }
}

fn kill(st: &mut State) {
    if let Some(mut c) = st.child.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    st.endpoint = None;
}

/// A free localhost port (bind to 0, read it, release it).
fn free_port() -> Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

/// A random 32-hex-digit key (two ULIDs' random bits).
fn random_key() -> String {
    let a = ulid::Ulid::generate().random();
    let b = ulid::Ulid::generate().random();
    format!("{:016x}{:016x}", a as u64 ^ (b >> 64) as u64, b as u64)
}

/// `GET /ready` → true on HTTP 200.
pub(crate) fn http_ready(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    if s.write_all(b"GET /ready HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]);
    head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.0 200")
}

fn launch(cfg: &ServeConfig, st: &mut State) -> std::result::Result<(), String> {
    let port = free_port().map_err(|e| e.to_string())?;
    let key = random_key();
    let mut cmd = process::command(&cfg.engine_dir, Some(&cfg.models_dir));
    cmd.env("NEMO_SPEECH_HTTP_API_KEY", &key);
    cmd.args(cfg.args(port));
    match cfg.log_path.as_ref().and_then(|p| {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        OpenOptions::new().create(true).append(true).open(p).ok()
    }) {
        Some(mut f) => {
            let _ = writeln!(f, "\n$ {cmd:?}");
            let f2 = f.try_clone().map_err(|e| e.to_string())?;
            cmd.stdout(Stdio::from(f)).stderr(Stdio::from(f2));
        }
        None => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("cannot start {:?}: {e}", cmd.get_program()))?;
    st.child = Some(child);
    st.endpoint = Some(ServeEndpoint { port, api_key: key });
    st.launched_at = Instant::now();
    st.status = ServeStatus::Starting { port };
    Ok(())
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis((1_000u64 << attempt.min(5)).min(30_000))
}

fn monitor(weak: Weak<Inner>) {
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let mut st = inner.lock();
        if st.shutdown {
            kill(&mut st);
            return;
        }
        let cfg = &inner.cfg;
        let mut notify = false;
        // Exited?
        let exited = st
            .child
            .as_mut()
            .and_then(|c| c.try_wait().ok().flatten())
            .map(|s| s.to_string());
        if let Some(code) = exited {
            let was_ready = matches!(st.status, ServeStatus::Ready { .. });
            st.child = None;
            st.endpoint = None;
            if was_ready {
                st.restarts += 1;
            } else {
                st.failures += 1;
            }
            if st.want_running {
                if st.failures >= MAX_FAILURES {
                    st.status = ServeStatus::Failed {
                        message: format!(
                            "exited {} times before ready (last: {code})",
                            st.failures
                        ),
                    };
                } else {
                    let attempt = st.failures.max(1);
                    st.retry_at = Some(Instant::now() + backoff(attempt - 1));
                    st.status = ServeStatus::Restarting {
                        attempt,
                        last_error: format!("exited: {code}"),
                    };
                }
            } else {
                st.status = ServeStatus::Stopped;
            }
            // A crash after it was ready, or the launch it gave up on, leaves
            // `logs\engine-crash-*.txt` (docs/plans/11 8a), off this lock.
            if st.want_running && (was_ready || st.failures >= MAX_FAILURES) {
                crate::crash::record_engine_exit_later(
                    "nemo-speech serve",
                    code.clone(),
                    cfg.log_path.clone(),
                );
            }
            notify = true;
        }
        // Launch when wanted.
        let failed = matches!(st.status, ServeStatus::Failed { .. });
        if st.want_running
            && st.child.is_none()
            && !failed
            && st.retry_at.is_none_or(|t| Instant::now() >= t)
        {
            st.retry_at = None;
            if let Err(e) = launch(cfg, &mut st) {
                st.failures += 1;
                st.status = if st.failures >= MAX_FAILURES {
                    ServeStatus::Failed { message: e }
                } else {
                    st.retry_at = Some(Instant::now() + backoff(st.failures - 1));
                    ServeStatus::Restarting {
                        attempt: st.failures,
                        last_error: e,
                    }
                };
            }
            notify = true;
        }
        // Ready yet?
        if let ServeStatus::Starting { port } = st.status {
            drop(st);
            let ready = http_ready(port);
            st = inner.lock();
            if st.status == (ServeStatus::Starting { port }) {
                if ready {
                    st.status = ServeStatus::Ready { port };
                    st.ready_since = Some(Instant::now());
                    st.failures = 0;
                    st.idle_since = Instant::now();
                    notify = true;
                } else if st.launched_at.elapsed() > cfg.ready_timeout {
                    kill(&mut st);
                    st.failures += 1;
                    st.retry_at = Some(Instant::now() + backoff(st.failures - 1));
                    st.status = if st.failures >= MAX_FAILURES {
                        ServeStatus::Failed {
                            message: format!("not ready after {} s", cfg.ready_timeout.as_secs()),
                        }
                    } else {
                        ServeStatus::Restarting {
                            attempt: st.failures,
                            last_error: "not ready in time".into(),
                        }
                    };
                    notify = true;
                }
            }
        }
        // Idle timeout.
        if let (ServeStatus::Ready { .. }, Some(idle)) = (&st.status, cfg.idle_timeout) {
            if st.sessions == 0 && st.idle_since.elapsed() >= idle {
                st.want_running = false;
                kill(&mut st);
                st.status = ServeStatus::Stopped;
                notify = true;
            }
        }
        if notify {
            inner.changed.notify_all();
        }
        let wait = if matches!(st.status, ServeStatus::Starting { .. }) {
            Duration::from_millis(200)
        } else {
            Duration::from_millis(500)
        };
        let _ = inner.changed.wait_timeout(st, wait);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_line_and_key() {
        let mut cfg = ServeConfig::new("E:/engine", "E:/models", "E:/m/asr.gguf", "E:/m/diar.gguf");
        cfg.extra_args.push("--verbose".into());
        let args: Vec<String> = cfg
            .args(8766)
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(&args[..2], ["--json", "serve"]);
        let joined = args.join(" ");
        assert!(joined.contains("--asr-model E:/m/asr.gguf --diar-model E:/m/diar.gguf"));
        assert!(joined.contains("--host 127.0.0.1 --port 8766 --no-ui"));
        assert!(joined.contains("--asr.batching.state_arena_slots 2"));
        assert!(joined.contains("--asr.endpointing.enable=true"));
        assert!(joined.ends_with("--verbose"));
        let k = random_key();
        assert_eq!(k.len(), 32);
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(k, random_key());
        assert_eq!(
            ServeEndpoint {
                port: 9,
                api_key: k
            }
            .realtime_url(),
            "ws://127.0.0.1:9/v1/audio/transcriptions/realtime"
        );
    }

    /// The live server on the Vulkan or CPU package passes that package's
    /// device: they refuse `cuda:0` (exit code 2).
    #[test]
    fn serve_line_takes_the_package_device() {
        for (package, device) in [
            ("nemo-speech-0.2.0-windows-x86_64-cuda", "cuda:0"),
            ("nemo-speech-0.2.0-windows-x86_64-vulkan", "vulkan:0"),
            ("nemo-speech-0.2.0-windows-x86_64-cpu", "cpu"),
            ("custom", "auto"),
        ] {
            let cfg = ServeConfig::new(
                format!("E:/engine/{package}"),
                "E:/models",
                "E:/m/asr.gguf",
                "E:/m/diar.gguf",
            );
            let args: Vec<String> = cfg
                .args(8766)
                .into_iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert_eq!(
                args,
                [
                    "--json",
                    "serve",
                    "--asr-model",
                    "E:/m/asr.gguf",
                    "--diar-model",
                    "E:/m/diar.gguf",
                    "--device",
                    device,
                    "--host",
                    "127.0.0.1",
                    "--port",
                    "8766",
                    "--no-ui",
                    "--asr.batching.state_arena_slots",
                    "2",
                    "--asr.endpointing.enable=true"
                ],
                "{package}"
            );
        }
        // The app and the CLI set the selection's device.
        let mut cfg = ServeConfig::new("E:/engine", "E:/models", "a.gguf", "d.gguf");
        cfg.device = "vulkan:0".into();
        let joined = cfg
            .args(1)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("--device vulkan:0 --host"), "{joined}");
    }

    #[test]
    fn missing_runtime_fails_after_retries() {
        let dir = tempfile::tempdir().unwrap();
        let sup = ServeSupervisor::new(ServeConfig::new(
            dir.path(),
            dir.path(),
            dir.path().join("asr.gguf"),
            dir.path().join("diar.gguf"),
        ));
        let s = sup.status();
        assert_eq!(s.status, ServeStatus::Stopped);
        assert_eq!((s.uptime_ms, s.restarts, s.pid), (None, 0, None));
        assert!(!s.is_ready());
        // Spawning fails every time: Restarting with backoff, then Failed.
        // (1 + 2 + 4 + 8 s of backoff is too long for a unit test, so only
        // the first transition is checked.)
        sup.start();
        let t = Instant::now();
        while !matches!(
            sup.status().status,
            ServeStatus::Restarting { .. } | ServeStatus::Failed { .. }
        ) {
            assert!(t.elapsed() < Duration::from_secs(5), "{:?}", sup.status());
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(sup.ensure_ready(Duration::from_millis(200)).is_err());
        sup.stop();
        assert_eq!(sup.status().status, ServeStatus::Stopped);
    }

    #[test]
    fn ready_probe_reads_the_status_line() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let h = std::thread::spawn(move || {
            let rq = server.recv().unwrap();
            assert_eq!(rq.url(), "/ready");
            rq.respond(tiny_http::Response::from_string("{\"ready\":true}"))
                .unwrap();
        });
        assert!(http_ready(port));
        h.join().unwrap();
        assert!(!http_ready(free_port().unwrap()));
    }
}
