use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use crate::tui::app::event::{Event, JobDone, JobProgress};

/// What a tracked background job is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    /// Live connect + initial sync for the active (foreground) account.
    Connect,
    /// Background warm-up connect for a non-active account.
    Warmup,
    /// Per-folder STATUS (message/unread) count sweep.
    FolderCounts,
    /// All-folders envelope sync into the offline cache.
    FolderSync,
    /// Manual re-fetch of the current folder (the `R` key).
    Refresh,
    /// Server-side delete of messages.
    ServerDelete,
    /// Server-side move (archive) of messages.
    ServerArchive,
}

impl JobKind {
    pub fn label(self) -> &'static str {
        match self {
            JobKind::Connect => "Connect",
            JobKind::Warmup => "Warm up",
            JobKind::FolderCounts => "Folder counts",
            JobKind::FolderSync => "Sync folders",
            JobKind::Refresh => "Refresh",
            JobKind::ServerDelete => "Delete on server",
            JobKind::ServerArchive => "Archive on server",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    Done,
    Failed,
}

/// One tracked background job, as shown in the job-tracker window.
pub struct Job {
    pub id: u64,
    pub kind: JobKind,
    /// Account label the job runs for (may be empty for account-less work).
    pub account: String,
    pub state: JobState,
    /// `(done, total)` for jobs that report determinate progress.
    pub progress: Option<(usize, usize)>,
    /// Failure message when `state == Failed`.
    pub error: Option<String>,
    pub started: Instant,
    pub finished: Option<Instant>,
}

impl Job {
    /// Wall-clock duration so far (or total, once finished).
    pub fn elapsed(&self) -> Duration {
        self.finished.unwrap_or_else(Instant::now) - self.started
    }
}

/// The set of tracked jobs. Owned by `App`; mutated only on the event loop.
pub struct JobRegistry {
    jobs: Vec<Job>,
    next_id: u64,
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl JobRegistry {
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            next_id: 1,
        }
    }

    /// Register a new running job and return its id.
    pub fn begin(&mut self, kind: JobKind, account: String) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.jobs.push(Job {
            id,
            kind,
            account,
            state: JobState::Running,
            progress: None,
            error: None,
            started: Instant::now(),
            finished: None,
        });
        id
    }

    pub fn set_progress(&mut self, id: u64, done: usize, total: usize) {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
            j.progress = Some((done, total));
        }
    }

    /// Mark a job finished (failed when `error` is `Some`). No-op for an unknown
    /// or already-finished id.
    pub fn finish(&mut self, id: u64, error: Option<String>) {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id)
            && j.state == JobState::Running
        {
            j.state = if error.is_some() {
                JobState::Failed
            } else {
                JobState::Done
            };
            j.error = error;
            j.finished = Some(Instant::now());
        }
    }

    pub fn has_running(&self) -> bool {
        self.jobs.iter().any(|j| j.state == JobState::Running)
    }

    pub fn running_count(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.state == JobState::Running)
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// Jobs newest-first, for display.
    pub fn iter(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter().rev()
    }

    /// Drop finished jobs that completed more than `older_than` ago, so the
    /// window and the running indicator settle back to idle after activity.
    pub fn prune(&mut self, older_than: Duration) {
        let now = Instant::now();
        self.jobs.retain(|j| match j.finished {
            Some(f) => now.duration_since(f) < older_than,
            None => true,
        });
    }

    /// Drop every finished job now (the window's "clear" key).
    pub fn clear_finished(&mut self) {
        self.jobs.retain(|j| j.state == JobState::Running);
    }
}

/// RAII completion signal held by a spawned background task. Progress goes out
/// via [`JobGuard::progress`]; on `Drop` it reports the job done - as failed if
/// [`JobGuard::fail`] was called - so completion is signaled even on an early
/// `return` or a panic. A guard with no sender (no event loop wired yet, as in
/// some tests) is inert.
pub struct JobGuard {
    id: u64,
    tx: Option<UnboundedSender<Event>>,
    error: Option<String>,
}

impl JobGuard {
    pub fn new(id: u64, tx: Option<UnboundedSender<Event>>) -> Self {
        Self {
            id,
            tx,
            error: None,
        }
    }

    /// Report determinate progress (e.g. folders synced so far).
    pub fn progress(&self, done: usize, total: usize) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Event::JobProgress(JobProgress {
                id: self.id,
                done,
                total,
            }));
        }
    }

    /// Mark the job as failed; the message is shown in the tracker.
    pub fn fail(&mut self, msg: impl Into<String>) {
        self.error = Some(msg.into());
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Event::JobDone(JobDone {
                id: self.id,
                error: self.error.take(),
            }));
        }
    }
}
