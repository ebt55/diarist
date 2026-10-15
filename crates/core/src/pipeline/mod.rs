//! Orchestration: file import (Phase 1), live post-pass and carry-over (Phase
//! 3), and the [`JobQueue`] that keeps GPU work to one job at a time.

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use crate::audio::Decoder;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::store::Store;
use crate::types::{JobId, MeetingId};
use crate::views::JobEvent;

/// What the user asked to import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRequest {
    pub job_id: JobId,
    /// The original file; kept as the playback source.
    pub path: PathBuf,
    /// Defaults to the file stem.
    pub title: Option<String>,
    /// BCP-47 tag or `auto`.
    pub language: String,
    pub diarize: bool,
}

/// Imports one audio or video file as a new meeting. Phase 1 implements it;
/// until then it returns [`CoreError::NotImplemented`].
///
/// Steps, each reported through `on_event` (`JobEvent::Progress` with the
/// matching stage, then exactly one `Done` or `Error`):
/// 1. `store.create_meeting` (source `file`, status `processing`), register
///    the original file as the `playback` track (format `original`).
/// 2. Decode to 16 kHz mono WAV in the meeting folder with `decoder`.
/// 3. `store.create_run(kind = file)`, then `engine.transcribe_file` and
///    drain its events until `Done` or `Error`.
/// 4. Build speakers, segments (`transcript` module) and words;
///    `store.insert_transcript`, `finish_run(done)`, `set_current_run`,
///    `finish_meeting`, status `done`; delete the 16 kHz intermediate.
/// 5. On any failure: `finish_run(failed)`, status `failed`, `JobEvent::Error`.
///
/// Runs on a [`JobQueue`] worker; it must poll the job's [`CancelToken`]
/// (passed by the caller's closure) and call `engine.cancel` when set.
pub fn import_file(
    store: &dyn Store,
    engine: &dyn Engine,
    decoder: &dyn Decoder,
    req: ImportRequest,
    on_event: &dyn Fn(JobEvent),
) -> Result<MeetingId> {
    let _ = (store, engine, decoder, req, on_event);
    Err(CoreError::NotImplemented("pipeline::import_file"))
}

/// Cooperative cancellation flag handed to each queued job.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Result of [`JobQueue::cancel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    /// The job was still queued and has been dropped without running.
    Dequeued,
    /// The job is running; its [`CancelToken`] is now set.
    Signalled,
    /// No queued or running job has this id.
    NotFound,
}

type JobFn = Box<dyn FnOnce(&CancelToken) + Send + 'static>;

#[derive(Default)]
struct QueueState {
    queue: VecDeque<(JobId, CancelToken, JobFn)>,
    running: Option<(JobId, CancelToken)>,
    paused: bool,
    shutdown: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<QueueState>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, QueueState> {
        // A job panicking cannot poison this lock (jobs run outside it), so
        // recovering the guard is safe.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Runs GPU jobs one at a time, in submission order, on a dedicated worker
/// thread. Jobs are closures that receive a [`CancelToken`].
///
/// [`JobQueue::set_paused`] stops new jobs from starting (Phase 3: file jobs
/// wait while a live recording holds the GPU). Dropping the queue waits for
/// the running job and drops the queued ones.
pub struct JobQueue {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl JobQueue {
    pub fn new() -> Self {
        let shared = Arc::new(Shared::default());
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("meet-job-queue".into())
            .spawn(move || worker_loop(&worker_shared))
            .expect("spawn job queue worker");
        Self {
            shared,
            worker: Some(worker),
        }
    }

    /// Enqueues a job.
    pub fn submit(&self, job_id: JobId, job: impl FnOnce(&CancelToken) + Send + 'static) {
        let mut st = self.shared.lock();
        st.queue
            .push_back((job_id, CancelToken::default(), Box::new(job)));
        self.shared.changed.notify_all();
    }

    /// Cancels a queued job (it never runs) or signals a running one.
    pub fn cancel(&self, job_id: &JobId) -> CancelOutcome {
        let mut st = self.shared.lock();
        if let Some((id, token)) = &st.running {
            if id == job_id {
                token.cancel();
                return CancelOutcome::Signalled;
            }
        }
        if let Some(pos) = st.queue.iter().position(|(id, _, _)| id == job_id) {
            st.queue.remove(pos);
            self.shared.changed.notify_all();
            return CancelOutcome::Dequeued;
        }
        CancelOutcome::NotFound
    }

    /// The running job, if any.
    pub fn running(&self) -> Option<JobId> {
        self.shared
            .lock()
            .running
            .as_ref()
            .map(|(id, _)| id.clone())
    }

    /// Queued (not yet running) jobs in order.
    pub fn queued(&self) -> Vec<JobId> {
        self.shared
            .lock()
            .queue
            .iter()
            .map(|(id, _, _)| id.clone())
            .collect()
    }

    /// While paused, the running job finishes but no new job starts.
    pub fn set_paused(&self, paused: bool) {
        self.shared.lock().paused = paused;
        self.shared.changed.notify_all();
    }

    /// Blocks until nothing is running and, unless paused, nothing is queued.
    pub fn wait_idle(&self) {
        let mut st = self.shared.lock();
        while st.running.is_some() || (!st.paused && !st.queue.is_empty()) {
            st = self
                .shared
                .changed
                .wait(st)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl Default for JobQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for JobQueue {
    fn drop(&mut self) {
        {
            let mut st = self.shared.lock();
            st.shutdown = true;
            st.queue.clear();
            self.shared.changed.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn worker_loop(shared: &Shared) {
    loop {
        let (token, job) = {
            let mut st = shared.lock();
            loop {
                if st.shutdown {
                    return;
                }
                if !st.paused {
                    if let Some((job_id, token, job)) = st.queue.pop_front() {
                        // Mark running in the same critical section as the pop,
                        // so observers never see the job in neither place.
                        st.running = Some((job_id, token.clone()));
                        break (token, job);
                    }
                }
                st = shared.changed.wait(st).unwrap_or_else(|e| e.into_inner());
            }
        };
        shared.changed.notify_all();
        // A panicking job must not kill the worker; the job is responsible for
        // reporting its own failure through its event callback.
        let _ = catch_unwind(AssertUnwindSafe(|| job(&token)));
        shared.lock().running = None;
        shared.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::time::Duration;

    fn id(s: &str) -> JobId {
        JobId(s.to_owned())
    }

    #[test]
    fn runs_jobs_one_at_a_time_in_order() {
        let q = JobQueue::new();
        let active = Arc::new(AtomicUsize::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));
        for n in 0..5 {
            let active = Arc::clone(&active);
            let order = Arc::clone(&order);
            q.submit(id(&n.to_string()), move |_| {
                assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0, "overlapping jobs");
                std::thread::sleep(Duration::from_millis(5));
                order.lock().unwrap().push(n);
                active.fetch_sub(1, Ordering::SeqCst);
            });
        }
        q.wait_idle();
        assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn cancel_queued_and_running() {
        let q = JobQueue::new();
        let (started_tx, started_rx) = mpsc::channel();
        let (saw_cancel_tx, saw_cancel_rx) = mpsc::channel();
        q.submit(id("a"), move |token| {
            started_tx.send(()).unwrap();
            while !token.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            saw_cancel_tx.send(()).unwrap();
        });
        let ran_b = Arc::new(AtomicBool::new(false));
        let ran_b2 = Arc::clone(&ran_b);
        q.submit(id("b"), move |_| ran_b2.store(true, Ordering::SeqCst));

        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(q.running(), Some(id("a")));
        assert_eq!(q.queued(), vec![id("b")]);
        assert_eq!(q.cancel(&id("b")), CancelOutcome::Dequeued);
        assert_eq!(q.cancel(&id("a")), CancelOutcome::Signalled);
        saw_cancel_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        q.wait_idle();
        assert!(!ran_b.load(Ordering::SeqCst));
        assert_eq!(q.cancel(&id("zzz")), CancelOutcome::NotFound);
    }

    #[test]
    fn paused_queue_holds_jobs_and_panics_do_not_kill_the_worker() {
        let q = JobQueue::new();
        q.set_paused(true);
        let ran = Arc::new(AtomicUsize::new(0));
        q.submit(id("boom"), |_| panic!("job failure"));
        let r = Arc::clone(&ran);
        q.submit(id("after"), move |_| {
            r.fetch_add(1, Ordering::SeqCst);
        });
        q.wait_idle();
        assert_eq!(ran.load(Ordering::SeqCst), 0);
        assert_eq!(q.queued().len(), 2);
        q.set_paused(false);
        q.wait_idle();
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn import_file_is_a_documented_stub() {
        struct NoEngine;
        impl Engine for NoEngine {
            fn transcribe_file(
                &self,
                _: crate::engine::FileJob,
            ) -> Result<mpsc::Receiver<crate::engine::EngineEvent>> {
                unreachable!()
            }
            fn cancel(&self, _: JobId) {}
            fn info(&self) -> crate::engine::EngineInfo {
                crate::engine::EngineInfo::default()
            }
        }
        struct NoDecoder;
        impl Decoder for NoDecoder {
            fn decode_to_wav16k(
                &self,
                _: &std::path::Path,
                _: &std::path::Path,
            ) -> Result<crate::audio::DecodedInfo> {
                unreachable!()
            }
        }
        let store = crate::store::MemoryStore::new();
        let req = ImportRequest {
            job_id: JobId::new(),
            path: "a.m4a".into(),
            title: None,
            language: "en".into(),
            diarize: true,
        };
        let err = import_file(&store, &NoEngine, &NoDecoder, req, &|_| {}).unwrap_err();
        assert!(matches!(err, CoreError::NotImplemented(_)));
    }
}
