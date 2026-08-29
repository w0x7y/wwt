//! Ordered, coalesced session-file writes.
//!
//! `Core` says why a snapshot needs saving and supplies the current time.
//! This module owns which snapshot is pending, when it becomes due, and the
//! single writer that puts durability barriers behind every earlier write.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, Instant};

use crate::event::Job;
use crate::store::Snapshot;

/// A held scroll key can change the saved offset every frame. Only the last
/// snapshot in that burst is useful to a later launch.
const SAVE_DEBOUNCE: Duration = Duration::from_secs(1);

type Reporter = dyn Fn(Job) + Send + Sync;
type Save = dyn Fn(&Path, &Snapshot) -> Result<(), String> + Send + Sync;

/// Why this snapshot is being offered to persistence.
pub(crate) enum SaveIntent {
    /// Replace the pending browsing update and restart its one-second wait.
    Debounced,
    /// Reach disk before the browser profile is handed to visible Chromium.
    LoginBarrier,
    /// Reach disk before the process is allowed to finish.
    ShutdownBarrier,
}

enum SaveCompletion {
    ReportFailure,
    Login,
    Shutdown(oneshot::Sender<Result<(), String>>),
}

struct SaveRequest {
    path: PathBuf,
    snapshot: Snapshot,
    completion: SaveCompletion,
}

/// Everything `Core` would otherwise have to coordinate for session writes.
pub(crate) struct Persistence {
    path: Option<PathBuf>,
    requests: mpsc::UnboundedSender<SaveRequest>,
    report: Arc<Reporter>,
    pending: Option<Snapshot>,
    deadline: Option<Instant>,
    shutdown: Option<oneshot::Receiver<Result<(), String>>>,
}

impl Persistence {
    /// One path and one writer for the lifetime of a `Core`.
    pub(crate) fn new(path: Option<PathBuf>, report: impl Fn(Job) + Send + Sync + 'static) -> Self {
        Self::with_writer(path, report, crate::store::save)
    }

    #[cfg(test)]
    fn with_save(
        path: Option<PathBuf>,
        report: impl Fn(Job) + Send + Sync + 'static,
        save: impl Fn(&Path, &Snapshot) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Self::with_writer(path, report, save)
    }

    fn with_writer(
        path: Option<PathBuf>,
        report: impl Fn(Job) + Send + Sync + 'static,
        save: impl Fn(&Path, &Snapshot) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        let report: Arc<Reporter> = Arc::new(report);
        let requests = spawn_save_worker(Arc::clone(&report), Arc::new(save));
        Self {
            path,
            requests,
            report,
            pending: None,
            deadline: None,
            shutdown: None,
        }
    }

    /// Coalesce a browsing update or queue an exact durability barrier.
    pub(crate) fn request(&mut self, intent: SaveIntent, snapshot: Snapshot, now: Instant) {
        match intent {
            SaveIntent::Debounced => {
                if self.path.is_none() {
                    return;
                }
                self.pending = Some(snapshot);
                self.deadline = Some(now + SAVE_DEBOUNCE);
            }
            SaveIntent::LoginBarrier => {
                self.pending = None;
                self.deadline = None;
                let Some(path) = self.path.clone() else {
                    (self.report)(Job::LoginSaved(Err(
                        "WWT does not own a session file".to_string()
                    )));
                    return;
                };
                let request = SaveRequest {
                    path,
                    snapshot,
                    completion: SaveCompletion::Login,
                };
                if self.requests.send(request).is_err() {
                    (self.report)(Job::LoginSaved(Err(
                        "session save worker stopped".to_string()
                    )));
                }
            }
            SaveIntent::ShutdownBarrier => {
                self.pending = None;
                self.deadline = None;
                self.shutdown = None;
                let Some(path) = self.path.clone() else {
                    return;
                };
                let (done, finished) = oneshot::channel();
                self.shutdown = Some(finished);
                let request = SaveRequest {
                    path,
                    snapshot,
                    completion: SaveCompletion::Shutdown(done),
                };
                let _ = self.requests.send(request);
            }
        }
    }

    /// When `Core` should wake the event loop to flush the pending snapshot.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Queue the snapshot whose debounce deadline the event loop observed.
    pub(crate) fn flush_due(&mut self) {
        self.deadline = None;
        let (Some(path), Some(snapshot)) = (self.path.clone(), self.pending.take()) else {
            return;
        };
        let request = SaveRequest {
            path,
            snapshot,
            completion: SaveCompletion::ReportFailure,
        };
        let _ = self.requests.send(request);
    }

    /// Wait for the shutdown barrier and every FIFO write ahead of it.
    pub(crate) async fn finish(&mut self) -> Result<(), String> {
        let Some(finished) = self.shutdown.take() else {
            return Ok(());
        };
        finished
            .await
            .map_err(|_| "session save worker stopped before shutdown".to_string())?
    }
}

fn spawn_save_worker(report: Arc<Reporter>, save: Arc<Save>) -> mpsc::UnboundedSender<SaveRequest> {
    let (tx, mut rx) = mpsc::unbounded_channel::<SaveRequest>();
    tokio::spawn(async move {
        while let Some(request) = rx.recv().await {
            let SaveRequest {
                path,
                snapshot,
                completion,
            } = request;
            let save = Arc::clone(&save);
            let result = tokio::task::spawn_blocking(move || save(&path, &snapshot))
                .await
                .map_err(|error| format!("session save task failed: {error}"))
                .and_then(|result| result);
            match completion {
                SaveCompletion::Login => (report)(Job::LoginSaved(result)),
                SaveCompletion::ReportFailure => {
                    if let Err(error) = result {
                        (report)(Job::Unsaved(error));
                    }
                }
                SaveCompletion::Shutdown(done) => {
                    let _ = done.send(result);
                }
            }
        }
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SavedTab;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;

    fn snapshot(url: &str) -> Snapshot {
        Snapshot {
            version: crate::store::VERSION,
            focus: 0,
            tabs: vec![SavedTab {
                url: url.to_string(),
                title: String::new(),
                scroll_y: 0.0,
            }],
        }
    }

    fn url(snapshot: &Snapshot) -> String {
        snapshot.tabs[0].url.clone()
    }

    #[tokio::test]
    async fn debounced_requests_replace_pending_and_move_the_deadline() {
        let directory = tempfile::tempdir().expect("session directory");
        let path = directory.path().join("session.json");
        let writes = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&writes);
        let (jobs_tx, mut jobs_rx) = mpsc::unbounded_channel();
        let mut persistence = Persistence::with_save(
            Some(path),
            move |job| {
                let _ = jobs_tx.send(job);
            },
            move |_, snapshot| {
                recorded.lock().expect("write log").push(url(snapshot));
                Ok(())
            },
        );
        let now = Instant::now();

        persistence.request(SaveIntent::Debounced, snapshot("https://old.test"), now);
        persistence.request(
            SaveIntent::Debounced,
            snapshot("https://latest.test"),
            now + Duration::from_millis(250),
        );
        assert_eq!(
            persistence.deadline(),
            Some(now + Duration::from_millis(250) + SAVE_DEBOUNCE)
        );

        persistence.flush_due();
        persistence.request(
            SaveIntent::LoginBarrier,
            snapshot("https://barrier.test"),
            now,
        );
        assert!(matches!(
            jobs_rx.recv().await.expect("login result"),
            Job::LoginSaved(Ok(()))
        ));
        assert_eq!(
            *writes.lock().expect("write log"),
            vec!["https://latest.test", "https://barrier.test"]
        );
    }

    #[tokio::test]
    async fn login_barrier_cancels_pending_and_queues_its_exact_snapshot() {
        let directory = tempfile::tempdir().expect("session directory");
        let path = directory.path().join("session.json");
        let writes = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&writes);
        let (jobs_tx, mut jobs_rx) = mpsc::unbounded_channel();
        let mut persistence = Persistence::with_save(
            Some(path),
            move |job| {
                let _ = jobs_tx.send(job);
            },
            move |_, snapshot| {
                recorded.lock().expect("write log").push(url(snapshot));
                Ok(())
            },
        );
        let now = Instant::now();

        persistence.request(SaveIntent::Debounced, snapshot("https://pending.test"), now);
        persistence.request(
            SaveIntent::LoginBarrier,
            snapshot("https://login.test"),
            now,
        );

        assert_eq!(persistence.deadline(), None);
        assert!(matches!(
            jobs_rx.recv().await.expect("login result"),
            Job::LoginSaved(Ok(()))
        ));
        assert_eq!(
            *writes.lock().expect("write log"),
            vec!["https://login.test"]
        );
    }

    #[tokio::test]
    async fn login_barrier_waits_behind_a_flushed_ordinary_write() {
        let directory = tempfile::tempdir().expect("session directory");
        let path = directory.path().join("session.json");
        let (jobs_tx, mut jobs_rx) = mpsc::unbounded_channel();
        let mut persistence = Persistence::with_save(
            Some(path.clone()),
            move |job| {
                let _ = jobs_tx.send(job);
            },
            move |path, snapshot| {
                if snapshot.tabs[0].url == "https://older.test" {
                    std::thread::sleep(Duration::from_millis(100));
                }
                crate::store::save(path, snapshot)
            },
        );
        let now = Instant::now();

        persistence.request(SaveIntent::Debounced, snapshot("https://older.test"), now);
        persistence.flush_due();
        let login = snapshot("https://login.test");
        persistence.request(SaveIntent::LoginBarrier, login.clone(), now);

        assert!(matches!(
            jobs_rx.recv().await.expect("login result"),
            Job::LoginSaved(Ok(()))
        ));
        assert_eq!(crate::store::load(&path), Ok(Some(login)));
    }

    #[tokio::test]
    async fn shutdown_cancels_pending_and_finish_waits_for_the_exact_final_snapshot() {
        let directory = tempfile::tempdir().expect("session directory");
        let path = directory.path().join("session.json");
        let writes = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&writes);
        let mut persistence = Persistence::with_save(
            Some(path),
            |_| {},
            move |_, snapshot| {
                if snapshot.tabs[0].url == "https://older.test" {
                    std::thread::sleep(Duration::from_millis(100));
                }
                recorded.lock().expect("write log").push(url(snapshot));
                Ok(())
            },
        );
        let now = Instant::now();

        persistence.request(SaveIntent::Debounced, snapshot("https://older.test"), now);
        persistence.flush_due();
        persistence.request(SaveIntent::Debounced, snapshot("https://pending.test"), now);
        persistence.request(
            SaveIntent::ShutdownBarrier,
            snapshot("https://final.test"),
            now,
        );

        persistence.finish().await.expect("final save succeeds");
        assert_eq!(
            *writes.lock().expect("write log"),
            vec!["https://older.test", "https://final.test"]
        );
    }

    #[tokio::test]
    async fn ordinary_failure_reports_unsaved() {
        let directory = tempfile::tempdir().expect("session directory");
        let path = directory.path().join("session.json");
        let (jobs_tx, mut jobs_rx) = mpsc::unbounded_channel();
        let mut persistence = Persistence::with_save(
            Some(path),
            move |job| {
                let _ = jobs_tx.send(job);
            },
            |_, _| Err("disk full".to_string()),
        );

        persistence.request(
            SaveIntent::Debounced,
            snapshot("https://page.test"),
            Instant::now(),
        );
        persistence.flush_due();

        let Job::Unsaved(message) = jobs_rx.recv().await.expect("save result") else {
            panic!("ordinary failure must be reported as unsaved");
        };
        assert_eq!(message, "disk full");
    }

    #[tokio::test]
    async fn a_session_without_a_file_rejects_login_and_finishes_without_writing() {
        let (jobs_tx, mut jobs_rx) = mpsc::unbounded_channel();
        let mut persistence = Persistence::with_save(
            None,
            move |job| {
                let _ = jobs_tx.send(job);
            },
            |_, _| panic!("a private session must not write"),
        );
        let now = Instant::now();

        persistence.request(SaveIntent::Debounced, snapshot("https://pending.test"), now);
        assert_eq!(persistence.deadline(), None);
        persistence.request(
            SaveIntent::LoginBarrier,
            snapshot("https://login.test"),
            now,
        );
        assert!(matches!(
            jobs_rx.recv().await.expect("login result"),
            Job::LoginSaved(Err(message)) if message == "WWT does not own a session file"
        ));
        persistence.request(
            SaveIntent::ShutdownBarrier,
            snapshot("https://final.test"),
            now,
        );
        persistence.finish().await.expect("nothing to write");
    }
}
