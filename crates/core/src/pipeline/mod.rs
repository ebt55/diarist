//! Orchestration: file import ([`import_file`]), live post-pass and
//! carry-over (Phase 3), and the [`JobQueue`] that keeps GPU work to one job
//! at a time.
//!
//! A job is a closure submitted to the queue with a [`JobId`]; it receives a
//! [`CancelToken`] and reports through [`crate::views::JobEvent`]s, ending
//! with exactly one of `Done`, `Error` or `Cancelled`. A job dequeued before
//! it ran never reports; whoever cancelled it emits `Cancelled`.

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use crate::types::JobId;

mod import;

pub use import::{import_file, ImportOptions, MIN_FREE_BYTES};

/// What the user asked to import. Language, retention of intermediates and
/// the data root come from [`ImportOptions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRequest {
    pub job_id: JobId,
    /// The original file; kept as the playback source.
    pub path: PathBuf,
    /// Defaults to the file stem.
    pub title: Option<String>,
    pub diarize: bool,
}

/// The log of the most recent engine job under `audio_root`
/// (`audio/<uid>/engine.log`, newest by modification time) and its last
/// `max_bytes` (cut at a line start). `None` when no job has run.
pub fn last_engine_log(
    audio_root: &std::path::Path,
    max_bytes: usize,
) -> Option<(PathBuf, String)> {
    let newest = std::fs::read_dir(audio_root)
        .ok()?
        .flatten()
        .map(|e| e.path().join("engine.log"))
        .filter_map(|p| {
            let modified = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((modified, p))
        })
        .max_by_key(|(m, _)| *m)?
        .1;
    let bytes = std::fs::read(&newest).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut start = text.len().saturating_sub(max_bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let tail = &text[start..];
    let tail = match (start > 0, tail.find('\n')) {
        (true, Some(i)) => &tail[i + 1..],
        _ => tail,
    };
    Some((newest, tail.trim().to_owned()))
}

/// Cooperative cancellation flag handed to each queued job.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// Sets the flag ([`JobQueue::cancel`] does this for queued jobs).
    pub fn cancel(&self) {
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

/// A queued job.
struct Queued {
    id: JobId,
    token: CancelToken,
    job: JobFn,
    /// Submitted with [`JobQueue::submit_front`].
    front: bool,
}

#[derive(Default)]
struct QueueState {
    queue: VecDeque<Queued>,
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
/// wait while a live recording holds the GPU). [`JobQueue::submit_front`]
/// queues a job ahead of everything [`JobQueue::submit`] queued (the
/// post-pass of a recording goes before imports queued while it recorded).
/// Dropping the queue waits for the running job and drops the queued ones.
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

    /// Enqueues a job at the end.
    pub fn submit(&self, job_id: JobId, job: impl FnOnce(&CancelToken) + Send + 'static) {
        let mut st = self.shared.lock();
        st.queue.push_back(Queued {
            id: job_id,
            token: CancelToken::default(),
            job: Box::new(job),
            front: false,
        });
        self.shared.changed.notify_all();
    }

    /// Enqueues a job ahead of every job queued with [`Self::submit`], after
    /// the ones queued with `submit_front` before it (those keep their
    /// order). A running job is never interrupted.
    pub fn submit_front(&self, job_id: JobId, job: impl FnOnce(&CancelToken) + Send + 'static) {
        let mut st = self.shared.lock();
        let at = st.queue.iter().take_while(|q| q.front).count();
        st.queue.insert(
            at,
            Queued {
                id: job_id,
                token: CancelToken::default(),
                job: Box::new(job),
                front: true,
            },
        );
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
        if let Some(pos) = st.queue.iter().position(|q| &q.id == job_id) {
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
            .map(|q| q.id.clone())
            .collect()
    }

    /// While paused, the running job finishes but no new job starts.
    pub fn set_paused(&self, paused: bool) {
        self.shared.lock().paused = paused;
        self.shared.changed.notify_all();
    }

    /// New jobs wait (a recording holds the GPU).
    pub fn is_paused(&self) -> bool {
        self.shared.lock().paused
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
                    if let Some(q) = st.queue.pop_front() {
                        // Mark running in the same critical section as the pop,
                        // so observers never see the job in neither place.
                        st.running = Some((q.id, q.token.clone()));
                        break (q.token, q.job);
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
    fn last_engine_log_is_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        assert!(last_engine_log(dir.path(), 100).is_none());
        for (uid, text) in [("A", "old run\n"), ("B", "first line\nsecond line\n")] {
            std::fs::create_dir_all(dir.path().join(uid)).unwrap();
            std::fs::write(dir.path().join(uid).join("engine.log"), text).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        let (path, tail) = last_engine_log(dir.path(), 100).unwrap();
        assert!(path.ends_with("B/engine.log") || path.ends_with("B\\engine.log"));
        assert_eq!(tail, "first line\nsecond line");
        assert_eq!(last_engine_log(dir.path(), 14).unwrap().1, "second line");
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
    fn submit_front_goes_ahead_of_submitted_jobs_in_order() {
        let q = JobQueue::new();
        q.set_paused(true);
        assert!(q.is_paused());
        let order = Arc::new(Mutex::new(Vec::new()));
        let job = |name: &'static str| {
            let order = Arc::clone(&order);
            move |_: &CancelToken| order.lock().unwrap().push(name)
        };
        q.submit(id("import-1"), job("import-1"));
        q.submit(id("import-2"), job("import-2"));
        q.submit_front(id("post-pass-1"), job("post-pass-1"));
        q.submit_front(id("post-pass-2"), job("post-pass-2"));
        q.submit(id("import-3"), job("import-3"));
        assert_eq!(
            q.queued(),
            [
                "post-pass-1",
                "post-pass-2",
                "import-1",
                "import-2",
                "import-3"
            ]
            .map(id)
        );
        assert_eq!(q.cancel(&id("post-pass-2")), CancelOutcome::Dequeued);
        q.set_paused(false);
        assert!(!q.is_paused());
        q.wait_idle();
        assert_eq!(
            *order.lock().unwrap(),
            ["post-pass-1", "import-1", "import-2", "import-3"]
        );
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
}
