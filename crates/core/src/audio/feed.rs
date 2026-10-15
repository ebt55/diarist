//! The engine feed (`docs/01`, "Capture robustness"): a bounded queue of
//! 100 ms frames between a channel's pipeline and its live session, drained
//! by a worker thread that owns the session.
//!
//! The pipeline side ([`FeedTx::offer`]) never blocks: when the queue holds
//! its capacity (10 s by default) the feed drops to record-only and reports
//! [`FeedTransition::Degraded`]; once the worker has drained it below the
//! low-water mark it reports `Recovered` and resumes. The WAV gets every
//! sample either way. The worker ([`spawn_feed_worker`]) gives frames that
//! are exactly zero a 1-LSB dither before the engine (runtime bug #48; the
//! WAV keeps the clean signal), forwards the session's events, and reopens
//! the session when it fails.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::audio::pipeline::Frame;
use crate::engine::{EngineEvent, LiveSession};
use crate::error::Result;
use crate::types::Channel;

/// Frames per channel the feed holds before degrading (10 s).
pub const FEED_CAPACITY: usize = 100;
/// Frames below which a degraded feed recovers (2 s).
pub const FEED_RECOVER_BELOW: usize = 20;

/// A change of the feed state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedTransition {
    /// The engine fell behind by the whole queue: record-only.
    Degraded,
    /// The engine caught up: frames flow again.
    Recovered,
}

/// The pipeline's end of the feed.
pub struct FeedTx {
    tx: SyncSender<Frame>,
    queued: Arc<AtomicUsize>,
    capacity: usize,
    recover_below: usize,
    degraded: bool,
    /// The worker is gone: everything is dropped from now on.
    closed: bool,
    dropped: u64,
}

/// The worker's end of the feed.
pub struct FeedRx {
    rx: Receiver<Frame>,
    queued: Arc<AtomicUsize>,
}

/// A feed holding at most `capacity` frames that recovers below `recover_below`.
pub fn feed(capacity: usize, recover_below: usize) -> (FeedTx, FeedRx) {
    let capacity = capacity.max(1);
    let (tx, rx) = mpsc::sync_channel(capacity);
    let queued = Arc::new(AtomicUsize::new(0));
    (
        FeedTx {
            tx,
            queued: Arc::clone(&queued),
            capacity,
            recover_below: recover_below.min(capacity - 1),
            degraded: false,
            closed: false,
            dropped: 0,
        },
        FeedRx { rx, queued },
    )
}

impl FeedTx {
    /// Queues a frame without blocking; returns a state change, if any.
    pub fn offer(&mut self, frame: Frame) -> Option<FeedTransition> {
        if self.closed {
            self.dropped += 1;
            return None;
        }
        let mut change = None;
        if self.degraded {
            if self.queued.load(Ordering::SeqCst) <= self.recover_below {
                self.degraded = false;
                change = Some(FeedTransition::Recovered);
            } else {
                self.dropped += 1;
                return None;
            }
        }
        if self.queued.load(Ordering::SeqCst) >= self.capacity {
            return self.degrade();
        }
        self.queued.fetch_add(1, Ordering::SeqCst);
        match self.tx.try_send(frame) {
            Ok(()) => change,
            Err(TrySendError::Full(_)) => {
                self.queued.fetch_sub(1, Ordering::SeqCst);
                // Recovered and degraded again in one call: report the latter.
                self.degrade()
            }
            Err(TrySendError::Disconnected(_)) => {
                self.queued.fetch_sub(1, Ordering::SeqCst);
                self.closed = true;
                self.degrade()
            }
        }
    }

    fn degrade(&mut self) -> Option<FeedTransition> {
        self.dropped += 1;
        if self.degraded {
            None
        } else {
            self.degraded = true;
            Some(FeedTransition::Degraded)
        }
    }

    pub fn is_degraded(&self) -> bool {
        self.degraded
    }

    /// Frames not sent to the engine.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Frames waiting for the worker.
    pub fn queued(&self) -> usize {
        self.queued.load(Ordering::SeqCst)
    }
}

impl FeedRx {
    pub fn recv_timeout(&self, d: Duration) -> std::result::Result<Frame, RecvTimeoutError> {
        let f = self.rx.recv_timeout(d)?;
        self.queued.fetch_sub(1, Ordering::SeqCst);
        Ok(f)
    }
}

/// Replaces an all-zero frame with ±1 LSB noise (xorshift); returns whether
/// it did. Frames with any signal are left alone.
pub fn dither_silence(pcm: &mut [i16], state: &mut u32) -> bool {
    if pcm.is_empty() || pcm.iter().any(|&s| s != 0) {
        return false;
    }
    for s in pcm.iter_mut() {
        let mut x = *state | 1;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        *state = x;
        *s = if x & 1 == 0 { 1 } else { -1 };
    }
    true
}

/// What the worker reports.
#[derive(Debug, Clone, PartialEq)]
pub enum FeedNotice {
    /// An event from the session (utterances, finals, `Done`, `Error`).
    Engine(EngineEvent),
    /// The session failed or could not be opened; frames are dropped until it reopens.
    Down { reason: String },
    /// The session (re)opened after being down.
    Up { restarts: u32 },
}

/// Counters of a finished worker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedReport {
    pub frames_pushed: u64,
    pub frames_dithered: u64,
    /// Frames received while no session was open.
    pub frames_lost: u64,
    pub restarts: u32,
}

/// Opens a live session for the worker.
pub type SessionOpener = Box<dyn FnMut() -> Result<Box<dyn LiveSession>> + Send>;
/// Receives the worker's notices.
pub type FeedNotify = Arc<dyn Fn(Channel, FeedNotice) + Send + Sync>;

/// A running feed worker.
pub struct FeedWorker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<FeedReport>>,
}

impl FeedWorker {
    /// Waits for the worker to drain the queue (the pipeline dropped its
    /// [`FeedTx`]), close the session and forward its last events.
    pub fn join(mut self) -> FeedReport {
        self.handle
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default()
    }

    /// Asks the worker to stop without waiting for the queue to drain.
    pub fn abort(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// How long `close` may take to deliver the last events.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(15);

/// Starts the worker: frames from `rx` go to the session `open` returns;
/// a failed session is reopened every `retry_every`.
pub fn spawn_feed_worker(
    channel: Channel,
    rx: FeedRx,
    mut open: SessionOpener,
    notify: FeedNotify,
    retry_every: Duration,
) -> FeedWorker {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_w = Arc::clone(&stop);
    let handle = std::thread::Builder::new()
        .name(format!("feed-{channel}"))
        .spawn(move || {
            let mut report = FeedReport::default();
            let mut session: Option<Box<dyn LiveSession>> = None;
            let mut down_since: Option<Instant> = None;
            let mut ever_up = false;
            let mut dither = 0x9E37_79B9u32 ^ (channel as u32).wrapping_mul(0x85EB_CA6B);
            let try_open = |open: &mut SessionOpener,
                            session: &mut Option<Box<dyn LiveSession>>,
                            report: &mut FeedReport,
                            ever_up: &mut bool,
                            down_since: &mut Option<Instant>| {
                match open() {
                    Ok(s) => {
                        *session = Some(s);
                        if *ever_up {
                            report.restarts += 1;
                            notify(
                                channel,
                                FeedNotice::Up {
                                    restarts: report.restarts,
                                },
                            );
                        } else if down_since.is_some() {
                            notify(channel, FeedNotice::Up { restarts: 0 });
                        }
                        *ever_up = true;
                        *down_since = None;
                    }
                    Err(e) => {
                        if down_since.is_none() {
                            *down_since = Some(Instant::now());
                            notify(
                                channel,
                                FeedNotice::Down {
                                    reason: e.to_string(),
                                },
                            );
                        }
                    }
                }
            };
            try_open(
                &mut open,
                &mut session,
                &mut report,
                &mut ever_up,
                &mut down_since,
            );
            let mut last_try = Instant::now();
            loop {
                if stop_w.load(Ordering::SeqCst) {
                    break;
                }
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(mut frame) => match session.as_mut() {
                        Some(s) => {
                            if dither_silence(&mut frame.pcm, &mut dither) {
                                report.frames_dithered += 1;
                            }
                            match s.push(&frame.pcm, frame.pts_us) {
                                Ok(()) => report.frames_pushed += 1,
                                Err(e) => {
                                    session = None;
                                    down_since = Some(Instant::now());
                                    last_try = Instant::now();
                                    notify(
                                        channel,
                                        FeedNotice::Down {
                                            reason: e.to_string(),
                                        },
                                    );
                                }
                            }
                        }
                        None => report.frames_lost += 1,
                    },
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                if let Some(s) = session.as_mut() {
                    let mut failed = None;
                    loop {
                        match s.events().try_recv() {
                            Ok(ev) => {
                                if let EngineEvent::Error { message, .. } = &ev {
                                    failed = Some(message.clone());
                                }
                                let terminal = ev.is_terminal();
                                notify(channel, FeedNotice::Engine(ev));
                                if terminal {
                                    failed.get_or_insert_with(|| "session ended".into());
                                    break;
                                }
                            }
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => {
                                failed = Some("session event stream closed".into());
                                break;
                            }
                        }
                    }
                    if let Some(reason) = failed {
                        session = None;
                        down_since = Some(Instant::now());
                        last_try = Instant::now();
                        notify(channel, FeedNotice::Down { reason });
                    }
                }
                if session.is_none() && last_try.elapsed() >= retry_every {
                    last_try = Instant::now();
                    try_open(
                        &mut open,
                        &mut session,
                        &mut report,
                        &mut ever_up,
                        &mut down_since,
                    );
                }
            }
            if let Some(mut s) = session.take() {
                let (_keep, dummy) = mpsc::channel();
                let events = std::mem::replace(s.events(), dummy);
                let closer = std::thread::spawn(move || s.close());
                let deadline = Instant::now() + CLOSE_TIMEOUT;
                while Instant::now() < deadline {
                    match events.recv_timeout(Duration::from_millis(100)) {
                        Ok(ev) => {
                            let terminal = ev.is_terminal();
                            notify(channel, FeedNotice::Engine(ev));
                            if terminal {
                                break;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                let _ = closer.join();
            }
            report
        })
        .expect("spawn feed worker");
    FeedWorker {
        stop,
        handle: Some(handle),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(i: u64) -> Frame {
        Frame {
            channel: Channel::Mic,
            start_sample: i * 1_600,
            pts_us: i * 100_000,
            pcm: vec![0; 1_600],
            level: Default::default(),
        }
    }

    #[test]
    fn degrades_when_full_and_recovers_below_low_water() {
        let (mut tx, rx) = feed(5, 2);
        for i in 0..5 {
            assert_eq!(tx.offer(frame(i)), None);
        }
        assert_eq!(tx.offer(frame(5)), Some(FeedTransition::Degraded));
        assert_eq!(tx.offer(frame(6)), None);
        assert!(tx.is_degraded());
        // Drain to 3: still degraded (above the low-water mark).
        for _ in 0..2 {
            rx.recv_timeout(Duration::from_millis(10)).unwrap();
        }
        assert_eq!(tx.offer(frame(7)), None);
        rx.recv_timeout(Duration::from_millis(10)).unwrap();
        assert_eq!(tx.offer(frame(8)), Some(FeedTransition::Recovered));
        assert_eq!(tx.dropped(), 3);
        assert_eq!(tx.queued(), 3);
        // Frame order is kept for what got through.
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(10)).unwrap().pts_us,
            300_000
        );
    }

    #[test]
    fn dither_only_touches_silent_frames() {
        let mut st = 1;
        let mut z = vec![0i16; 1_600];
        assert!(dither_silence(&mut z, &mut st));
        assert!(z.iter().all(|&s| s == 1 || s == -1));
        assert!(z.contains(&1) && z.contains(&-1));
        let mut sig = vec![0i16; 1_600];
        sig[10] = 3;
        assert!(!dither_silence(&mut sig, &mut st));
        assert_eq!(sig.iter().filter(|&&s| s != 0).count(), 1);
    }
}
