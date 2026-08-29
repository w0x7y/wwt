# WWT Persistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Centralize snapshot coalescing, ordered writes, login durability, and shutdown durability in one private persistence module without changing WWT behavior or public interfaces.

**Architecture:** Add a private `Persistence` owner beside `Core`. It owns the session-file path, pending snapshot, debounce deadline, FIFO writer task, result reporting, and shutdown receiver. `Core` retains event-loop timing and effect execution, but translates the existing save effects into `SaveIntent` and no longer coordinates persistence fields individually.

**Tech Stack:** Rust 2024, Tokio channels/tasks/time, existing WWT `Snapshot`, `Effect`, and `Job` types.

**Spec:** `docs/superpowers/specs/2026-08-28-wwt-lifecycle-modules-design.md`

## Global Constraints

- This phase changes code ownership, not WWT behavior. Parent specifications win.
- Preserve the public interfaces of `Event`, `Effect`, `Job`, `Session`, and `Core`.
- Keep one FIFO writer. Debounced saves must not overtake login or shutdown saves.
- A login handoff starts only after its exact snapshot reaches disk.
- Shutdown waits until its exact final snapshot reaches disk.
- A private session continues to write no session file.
- Keep the existing one-second save debounce and existing error messages.
- Add no dependencies, configuration, snapshot fields, browser flags, retries, or site-specific behavior.
- Do not change Chromium launch policy, frame pacing, the 33 ms screencast acknowledgement interval, page behavior, or terminal behavior.
- Run each new direct persistence test before its implementation and confirm that it fails for the expected missing-interface reason.
- Keep existing `Core` and `Session` characterization tests green.
- Before committing, run `cargo fmt --all -- --check`, `cargo test --workspace`, and `cargo clippy --workspace --all-targets -- -D warnings`.
- Inspect the final diff for unrelated changes. Do not push phase 2 without a separate instruction.
- Stop after the phase 2 commit for manual verification. Do not plan or begin phase 3.

---

## File map

- Create: `crates/wwt/src/persistence.rs` — private persistence state, save intents, ordered writer, and direct invariant tests.
- Modify: `crates/wwt/src/lib.rs` — declare the private module.
- Modify: `crates/wwt/src/core.rs` — replace persistence fields and sequencing with one `Persistence` value.
- Modify: `CONTEXT.md` — record the persistence owner and durability-barrier terminology.
- Test: `crates/wwt/src/core.rs` — preserve external characterization around the existing worker before moving it.

---

### Task 1: Lock the existing persistence behavior

**Files:**
- Modify: `crates/wwt/src/core.rs`
- Test: `crates/wwt/src/core.rs`

**Interfaces:**
- Consumes: the existing `SaveRequest`, FIFO worker, `Finished::Job`, and `Job` paths.
- Produces: characterization evidence for ordered barriers and ordinary failure reporting before code moves.

- [ ] **Step 1: Run the existing barrier characterizations**

Run:

```bash
cargo test -p wwt core::tests::login_save_acknowledges_after_earlier_saves_and_leaves_its_snapshot_last
cargo test -p wwt core::tests::final_save_barrier_drains_older_work_before_acknowledging_quit
```

Expected: both commands pass against the phase 1 commit.

- [ ] **Step 2: Add the missing ordinary-failure characterization**

Add beside the existing worker tests:

```rust
#[tokio::test]
async fn ordinary_save_failure_is_reported() {
    let directory = tempfile::tempdir().expect("session directory");
    let path = directory.path().join("session.json");
    let snapshot = Snapshot {
        version: crate::store::VERSION,
        focus: 0,
        tabs: Vec::new(),
    };
    let (jobs_tx, mut jobs_rx) = mpsc::unbounded_channel();
    let saves = spawn_save_worker_with(jobs_tx, |_, _| Err("disk full".to_string()));

    saves
        .send(SaveRequest::ordinary(path, snapshot))
        .expect("queue ordinary save");

    let Finished::Job(Job::Unsaved(message)) =
        jobs_rx.recv().await.expect("save result")
    else {
        panic!("ordinary failure must be reported as unsaved");
    };
    assert_eq!(message, "disk full");
}
```

- [ ] **Step 3: Run the new characterization against the old implementation**

Run:

```bash
cargo test -p wwt core::tests::ordinary_save_failure_is_reported
```

Expected: PASS. This is a characterization test, so it proves current behavior before extraction rather than starting RED.

- [ ] **Step 4: Run all `Core` tests**

Run:

```bash
cargo test -p wwt core::tests
```

Expected: all `Core` tests pass.

---

### Task 2: Introduce persistence ownership test-first

**Files:**
- Create: `crates/wwt/src/persistence.rs`
- Modify: `crates/wwt/src/lib.rs`
- Test: `crates/wwt/src/persistence.rs`

**Interfaces:**
- Consumes: `crate::store::Snapshot`, `crate::event::Job`, a session-file path, an injected job reporter, and `tokio::time::Instant`.
- Produces:

```rust
pub(crate) enum SaveIntent {
    Debounced,
    LoginBarrier,
    ShutdownBarrier,
}

pub(crate) struct Persistence;

impl Persistence {
    pub(crate) fn new(
        path: Option<PathBuf>,
        report: impl Fn(Job) + Send + Sync + 'static,
    ) -> Self;

    pub(crate) fn request(
        &mut self,
        intent: SaveIntent,
        snapshot: Snapshot,
        now: Instant,
    );

    pub(crate) fn deadline(&self) -> Option<Instant>;
    pub(crate) fn flush_due(&mut self);
    pub(crate) async fn finish(&mut self) -> Result<(), String>;
}
```

- [ ] **Step 1: Declare the private module and add direct tests before the types**

Add `mod persistence;` to `crates/wwt/src/lib.rs`. Create `persistence.rs` with imports, the `SAVE_DEBOUNCE` constant, a test-only snapshot helper, and these six tests. The tests deliberately refer to `Persistence` and `SaveIntent` before either exists.

```rust
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

        persistence.request(SaveIntent::Debounced, snapshot("https://page.test"), Instant::now());
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
```

- [ ] **Step 2: Run the direct module tests to verify RED**

Run:

```bash
cargo test -p wwt persistence::tests
```

Expected: compilation fails because `Persistence`, `SaveIntent`, and `Persistence::with_save` do not exist. Confirm this exact reason before implementation.

- [ ] **Step 3: Implement the minimal deep persistence module**

Above the tests, implement:

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, Instant};

use crate::event::Job;
use crate::store::Snapshot;

const SAVE_DEBOUNCE: Duration = Duration::from_secs(1);

type Reporter = dyn Fn(Job) + Send + Sync;
type Save = dyn Fn(&Path, &Snapshot) -> Result<(), String> + Send + Sync;

pub(crate) enum SaveIntent {
    Debounced,
    LoginBarrier,
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

pub(crate) struct Persistence {
    path: Option<PathBuf>,
    requests: mpsc::UnboundedSender<SaveRequest>,
    report: Arc<Reporter>,
    pending: Option<Snapshot>,
    deadline: Option<Instant>,
    shutdown: Option<oneshot::Receiver<Result<(), String>>>,
}
```

`Persistence::new` delegates to a private constructor with `crate::store::save`. The test-only `with_save` constructor accepts an injected writer. Both constructors create one unbounded FIFO channel and spawn one worker. The worker performs each write through `tokio::task::spawn_blocking` and preserves these exact completion rules:

```rust
match completion {
    SaveCompletion::Login => report(Job::LoginSaved(result)),
    SaveCompletion::ReportFailure => {
        if let Err(error) = result {
            report(Job::Unsaved(error));
        }
    }
    SaveCompletion::Shutdown(done) => {
        let _ = done.send(result);
    }
}
```

Implement requests as follows:

- `Debounced`: when a path exists, replace `pending` and set `deadline` to `now + SAVE_DEBOUNCE`; otherwise do nothing.
- `LoginBarrier`: clear `pending` and `deadline`, queue the exact snapshot as `SaveCompletion::Login`, or report `Job::LoginSaved(Err("WWT does not own a session file"))` when no path exists. Report `"session save worker stopped"` if the request channel is closed.
- `ShutdownBarrier`: clear `pending` and `deadline`; when a path exists, create a oneshot pair, store the receiver, and queue the exact snapshot as `SaveCompletion::Shutdown`. A closed request channel drops the sender so `finish` returns the existing shutdown-worker error.
- `flush_due`: clear `deadline`, take the pending snapshot, and queue it as `SaveCompletion::ReportFailure` when a path exists.
- `finish`: take and await the shutdown receiver. Preserve the exact outer error `"session save worker stopped before shutdown"`; return the writer's inner error unchanged. Return `Ok(())` when no shutdown write exists.

- [ ] **Step 4: Run the direct tests to verify GREEN**

Run:

```bash
cargo test -p wwt persistence::tests
```

Expected: all six direct persistence tests pass.

---

### Task 3: Route `Core` through `Persistence`

**Files:**
- Modify: `crates/wwt/src/core.rs`
- Test: `crates/wwt/src/core.rs`
- Test: `crates/wwt/src/session.rs`

**Interfaces:**
- Consumes: `Persistence::{new, request, deadline, flush_due, finish}` and `SaveIntent`.
- Produces: one `Core::persistence: Persistence` field with no direct persistence sequencing left in `Core`.

- [ ] **Step 1: Replace the old worker and fields**

In `core.rs`:

- Remove `SAVE_DEBOUNCE`, `SaveCompletion`, `SaveRequest`, `spawn_save_worker`, `spawn_save_worker_with`, and `Core::flush_save`.
- Remove `saves`, `final_save`, `session_file`, and `pending` from `Core`.
- Add `use crate::persistence::{Persistence, SaveIntent};`.
- Add `persistence: Persistence` beside `jobs_tx` and `jobs_rx`.
- Keep `Startup::session_file` unchanged.

Construct the module after the job channel:

```rust
let report = jobs_tx.clone();
let persistence = Persistence::new(startup.session_file, move |job| {
    let _ = report.send(Finished::Job(job));
});
```

- [ ] **Step 2: Move the debounce deadline behind the module**

Remove the local `save_at`. Before each `tokio::select!`, copy the deadline:

```rust
let save_at = self.persistence.deadline();
```

Keep the select arm free of `self`:

```rust
() = async { sleep_until(save_at.expect("guarded")).await },
    if save_at.is_some() =>
{
    due_to_save = true;
    None
}
```

After the select, replace `self.flush_save()` with:

```rust
self.persistence.flush_due();
```

- [ ] **Step 3: Translate existing save effects into intents**

Remove `save_at` from `Core::apply` and both callers. Preserve effect order while replacing the three branches:

```rust
Effect::Quit => {
    self.persistence.request(
        SaveIntent::ShutdownBarrier,
        self.session.snapshot(),
        Instant::now(),
    );
    return Ok(true);
}

Effect::Save(snapshot) => self.persistence.request(
    SaveIntent::Debounced,
    snapshot,
    Instant::now(),
),

Effect::SaveForLogin(snapshot) => self.persistence.request(
    SaveIntent::LoginBarrier,
    snapshot,
    Instant::now(),
),
```

Replace `finish_saves` with:

```rust
async fn finish_saves(&mut self) -> Result<()> {
    self.persistence.finish().await.map_err(anyhow::Error::msg)
}
```

- [ ] **Step 4: Remove migrated worker tests and run the focused suites**

Delete the three old worker-level tests from `core.rs`; their invariants now live in the direct persistence suite. Keep `old_browser_work_is_rejected_before_core_can_file_it` in `core.rs`.

Run:

```bash
cargo test -p wwt persistence::tests
cargo test -p wwt core::tests
cargo test -p wwt session::tests
cargo test -p wwt
```

Expected: every command passes. Existing `Effect`, `Job`, `Core`, `Session`, login, quit, and session-file behavior remains unchanged.

- [ ] **Step 5: Prove `Core` no longer coordinates persistence fields**

Run:

```bash
rg -n 'self\.(saves|final_save|session_file|pending)|let mut save_at|\*save_at|&mut save_at|SaveRequest|SaveCompletion|spawn_save_worker' \
  crates/wwt/src/core.rs
```

Expected: no match. The immutable local copied from `persistence.deadline()` is allowed;
`Startup::session_file` remains public, and `Core` may otherwise mention only
`persistence`, `SaveIntent`, and `Persistence` through the new module interface.

---

### Task 4: Document, verify, and commit phase 2

**Files:**
- Modify: `CONTEXT.md`
- Verify: all phase 2 files

**Interfaces:**
- Consumes: the completed private persistence seam.
- Produces: one reviewed local phase 2 commit ready for manual durability testing.

- [ ] **Step 1: Record the private persistence owner in `CONTEXT.md`**

Add after the **Core** entry:

```markdown
**Persistence** — the private owner of the session-file path, pending snapshot,
one-second debounce deadline, FIFO writer, and login and shutdown durability barriers.
`Core` supplies time and save intent; `Persistence` decides which exact snapshot is
queued and when completion returns as the existing job. `wwt::persistence::Persistence`.
```

- [ ] **Step 2: Inspect the complete phase diff**

Run:

```bash
git diff --check
git diff --stat
git diff -- CONTEXT.md crates/wwt/src/lib.rs crates/wwt/src/persistence.rs crates/wwt/src/core.rs docs/superpowers/plans/2026-08-28-wwt-persistence.md
git status --short
```

Expected: only the five phase 2 paths appear. There are no public interface, dependency, configuration, snapshot-format, browser, page, pixel, or site-specific changes.

- [ ] **Step 3: Run the full verification gate**

Run:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: tests and Clippy exit 0. The previously authorized workspace rustfmt baseline mismatch may remain: if `cargo fmt --all -- --check` still reports only pre-existing formatter differences, record that exception and do not reformat unrelated files.

- [ ] **Step 4: Commit the completed phase locally**

Run:

```bash
git add CONTEXT.md crates/wwt/src/lib.rs crates/wwt/src/persistence.rs crates/wwt/src/core.rs docs/superpowers/plans/2026-08-28-wwt-persistence.md
git commit -m "refactor(wwt): centralize persistence"
```

Expected: one local commit and a clean working tree. Do not push phase 2.

- [ ] **Step 5: Stop for manual verification**

Report the resulting module interface, red-green evidence, changed files, full verification results, local commit hash, and behavior that could be affected. Ask the user to verify ordinary session restoration, rapid-scroll debounce, login durability and failure recovery, clean shutdown restoration, and private-session no-write behavior. Do not plan or begin phase 3 until the user approves phase 2.
