# WWT Browser Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Concentrate browser availability, relaunch suppression, login eligibility, and browser transition policy in one private session-layer module without changing WWT behavior or public interfaces.

**Architecture:** Add a private `BrowserLifecycle` owned by `Session`. `Session` classifies page requests and results, asks the lifecycle to gate them, and applies transition outcomes to tabs, status, mode, and existing effects. Chromium processes, generations, retry backoff, page maps, and visible-browser machinery remain in `Core`.

**Tech Stack:** Rust 2024 and existing WWT `Event`, `Effect`, `Job`, `Session`, and `State` vocabulary.

**Spec:** `docs/superpowers/specs/2026-08-28-wwt-lifecycle-modules-design.md`

## Global Constraints

- This phase changes code ownership, not WWT behavior. Parent specifications win.
- Preserve the public interfaces of `Event`, `Effect`, `Job`, `Session`, and `Core`.
- Preserve the running, saving-login, login, missing, and relaunching states and their current transitions.
- Work from an absent or old browser cannot mutate current tab state.
- A missing browser requests at most one relaunch until that attempt finishes.
- Login remains unavailable to private sessions and starts only after its exact snapshot reaches disk.
- A deliberate login handoff must not race an automatic relaunch.
- Keep browser generations, stale-result filtering, process ownership, retry count/backoff, and visible Chromium handoff in `Core`.
- Add no dependencies, configuration, snapshot fields, browser flags, retries, or site-specific behavior.
- Do not change Chromium launch policy, frame pacing, the 33 ms screencast acknowledgement interval, persistence behavior, page behavior, reader behavior, or pixel behavior.
- Run each new direct browser test before implementation and confirm it fails for the expected missing-interface reason.
- Keep existing `Session`, `Core`, and supervisor characterization tests green.
- Before committing, run `cargo fmt --all -- --check`, `cargo test --workspace`, and `cargo clippy --workspace --all-targets -- -D warnings`.
- Inspect the final diff for unrelated changes. Do not push phase 3 without a separate instruction.
- Stop after the phase 3 commit for manual verification. Do not plan or begin phase 4.

---

## File map

- Create: `crates/wwt/src/browser.rs` — private availability state, work gate, browser transitions, outcomes, and direct invariant tests.
- Modify: `crates/wwt/src/lib.rs` — declare the private module.
- Modify: `crates/wwt/src/session.rs` — replace direct browser-state checks and writes with the module interface.
- Modify: `CONTEXT.md` — record browser lifecycle ownership and the page-work gate.
- Create: `docs/superpowers/plans/2026-08-28-wwt-browser-lifecycle.md` — phase 3 execution record.

---

### Task 1: Lock the existing browser behavior

**Files:**
- Modify: `crates/wwt/src/session.rs`
- Test: `crates/wwt/src/session.rs`

**Interfaces:**
- Consumes: current `BrowserState`, login, relaunch, and tab detachment behavior.
- Produces: baseline evidence for the state machine before extraction.

- [ ] **Step 1: Run the existing supervisor characterizations**

Run:

```bash
cargo test -p wwt session::tests::login_preserves_the_session_and_requests_one_browser_handoff
cargo test -p wwt session::tests::a_failed_pre_login_save_keeps_the_running_browser_and_tabs
cargo test -p wwt session::tests::the_expected_browser_disconnect_during_login_does_not_race_a_relaunch
cargo test -p wwt session::tests::closing_the_login_window_requests_one_headless_relaunch
cargo test -p wwt session::tests::a_failed_login_handoff_reports_the_error_and_recovers_headless
cargo test -p wwt session::tests::a_dead_browser_leaves_every_tab_where_it_was_and_asks_for_another
cargo test -p wwt session::tests::a_browser_that_came_back_is_asked_for_one_page
cargo test -p wwt session::tests::a_held_key_asks_for_one_relaunch_and_not_thirty
```

Expected: every test passes against the phase 2 commit.

- [ ] **Step 2: Add the missing saving-login loss characterization**

Add beside the supervisor tests:

```rust
#[test]
fn browser_loss_while_the_login_snapshot_is_saving_restarts_and_ignores_the_late_save() {
    let mut session = four_ready_tabs();
    typed(&mut session, ":login");
    assert!(matches!(
        session.on(code(KeyCode::Enter)).as_slice(),
        [Effect::SaveForLogin(_)]
    ));

    assert_eq!(session.on(Event::BrowserLost), vec![Effect::Relaunch]);
    assert!(session.tabs.iter().all(|tab| tab.presence == Presence::Detached));
    assert_eq!(session.on(Event::Done(Job::LoginSaved(Ok(())))), vec![]);
}
```

- [ ] **Step 3: Run the new characterization against the old implementation**

Run:

```bash
cargo test -p wwt session::tests::browser_loss_while_the_login_snapshot_is_saving_restarts_and_ignores_the_late_save
```

Expected: PASS. This records existing behavior before code moves.

---

### Task 2: Introduce the browser lifecycle test-first

**Files:**
- Create: `crates/wwt/src/browser.rs`
- Modify: `crates/wwt/src/lib.rs`
- Test: `crates/wwt/src/browser.rs`

**Interfaces:**
- Consumes: page-work classifications and browser signals from `Session`.
- Produces:

```rust
pub(crate) struct BrowserLifecycle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageWork {
    Request,
    Result,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkDecision {
    Proceed,
    Relaunch,
    Blocked,
}

pub(crate) enum BrowserSignal {
    Lost,
    Back,
    LoginRequested,
    LoginSaved(Result<(), String>),
    LoginFinished(Result<(), String>),
    RelaunchFinished(Result<(), String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserRequest {
    Relaunch,
    SaveForLogin,
    Login,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TabDirective {
    #[default]
    Keep,
    DetachAll,
    ReopenFocused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BrowserStatus {
    Notice(String),
    Error(String),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct BrowserOutcome {
    pub request: Option<BrowserRequest>,
    pub tabs: TabDirective,
    pub status: Option<BrowserStatus>,
}

impl BrowserLifecycle {
    pub(crate) fn new(login_available: bool) -> Self;
    pub(crate) fn set_login_available(&mut self, available: bool);
    pub(crate) fn gate(&mut self, work: PageWork) -> WorkDecision;
    pub(crate) fn transition(&mut self, signal: BrowserSignal) -> BrowserOutcome;
    pub(crate) fn running(&self) -> bool;
    pub(crate) fn allows_tab_change(&self) -> bool;
    pub(crate) fn allows_command(&self, login: bool) -> bool;
}
```

- [ ] **Step 1: Declare the module and add direct tests before implementation**

Add `mod browser;` to `lib.rs`. Create `browser.rs` with a test module containing these tests before defining the interface:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_browser_accepts_requests_and_results() {
        let mut browser = BrowserLifecycle::new(true);
        assert_eq!(browser.gate(PageWork::Request), WorkDecision::Proceed);
        assert_eq!(browser.gate(PageWork::Result), WorkDecision::Proceed);
    }

    #[test]
    fn missing_page_work_requests_exactly_one_relaunch() {
        let mut browser = BrowserLifecycle::new(true);
        browser.transition(BrowserSignal::Lost);
        browser.transition(BrowserSignal::RelaunchFinished(Err("gone".to_string())));

        assert_eq!(browser.gate(PageWork::Request), WorkDecision::Relaunch);
        assert_eq!(browser.gate(PageWork::Request), WorkDecision::Blocked);
    }

    #[test]
    fn deliberate_login_blocks_page_work_without_requesting_relaunch() {
        let mut browser = BrowserLifecycle::new(true);
        browser.transition(BrowserSignal::LoginRequested);
        browser.transition(BrowserSignal::LoginSaved(Ok(())));

        assert_eq!(browser.gate(PageWork::Request), WorkDecision::Blocked);
        assert_eq!(browser.gate(PageWork::Result), WorkDecision::Blocked);
    }

    #[test]
    fn browser_loss_detaches_tabs_and_requests_relaunch() {
        let mut browser = BrowserLifecycle::new(true);
        let outcome = browser.transition(BrowserSignal::Lost);

        assert_eq!(outcome.request, Some(BrowserRequest::Relaunch));
        assert_eq!(outcome.tabs, TabDirective::DetachAll);
        assert_eq!(
            outcome.status,
            Some(BrowserStatus::Notice("browser gone, restarting".to_string()))
        );
        assert!(!browser.running());
    }

    #[test]
    fn browser_back_reopens_only_the_focused_tab() {
        let mut browser = BrowserLifecycle::new(true);
        browser.transition(BrowserSignal::Lost);
        let outcome = browser.transition(BrowserSignal::Back);

        assert_eq!(outcome.tabs, TabDirective::ReopenFocused);
        assert!(browser.running());
    }

    #[test]
    fn private_session_refuses_login_without_leaving_running() {
        let mut browser = BrowserLifecycle::new(false);
        let outcome = browser.transition(BrowserSignal::LoginRequested);

        assert_eq!(
            outcome.status,
            Some(BrowserStatus::Error(
                "login needs WWT's persistent profile".to_string()
            ))
        );
        assert_eq!(outcome.request, None);
        assert!(browser.running());
    }

    #[test]
    fn login_save_success_detaches_tabs_and_hands_off_the_browser() {
        let mut browser = BrowserLifecycle::new(true);
        let requested = browser.transition(BrowserSignal::LoginRequested);
        assert_eq!(requested.request, Some(BrowserRequest::SaveForLogin));
        assert!(!browser.allows_tab_change());
        assert!(!browser.allows_command(false));
        assert!(browser.allows_command(true));

        let saved = browser.transition(BrowserSignal::LoginSaved(Ok(())));
        assert_eq!(saved.request, Some(BrowserRequest::Login));
        assert_eq!(saved.tabs, TabDirective::DetachAll);
        assert_eq!(
            saved.status,
            Some(BrowserStatus::Notice(
                "finish login in Chromium, then close it".to_string()
            ))
        );
    }

    #[test]
    fn failed_login_save_returns_to_the_running_browser() {
        let mut browser = BrowserLifecycle::new(true);
        browser.transition(BrowserSignal::LoginRequested);
        let outcome = browser.transition(BrowserSignal::LoginSaved(Err("disk full".to_string())));

        assert!(browser.running());
        assert_eq!(
            outcome.status,
            Some(BrowserStatus::Error(
                "login failed: save session: disk full".to_string()
            ))
        );
    }

    #[test]
    fn closing_or_failing_visible_login_requests_one_relaunch() {
        for result in [Ok(()), Err("could not launch".to_string())] {
            let mut browser = BrowserLifecycle::new(true);
            browser.transition(BrowserSignal::LoginRequested);
            browser.transition(BrowserSignal::LoginSaved(Ok(())));

            let outcome = browser.transition(BrowserSignal::LoginFinished(result));
            assert_eq!(outcome.request, Some(BrowserRequest::Relaunch));
            assert_eq!(
                browser.transition(BrowserSignal::LoginFinished(Ok(()))),
                BrowserOutcome::default()
            );
        }
    }
}
```

- [ ] **Step 2: Run the direct browser tests to verify RED**

Run:

```bash
cargo test -p wwt browser::tests
```

Expected: compilation fails because the lifecycle types and methods do not exist. Confirm this exact reason before implementation.

- [ ] **Step 3: Implement the five-state lifecycle**

Use a private state enum inside `browser.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserState {
    Running,
    SavingLogin,
    Login,
    Missing,
    Relaunching,
}

pub(crate) struct BrowserLifecycle {
    state: BrowserState,
    login_available: bool,
}
```

Implement the page gate exactly:

| State | `PageWork::Request` | `PageWork::Result` |
|---|---|---|
| Running | `Proceed` | `Proceed` |
| SavingLogin | `Blocked` | `Proceed` |
| Login | `Blocked` | `Blocked` |
| Missing | move to Relaunching, `Relaunch` | `Blocked` |
| Relaunching | `Blocked` | `Blocked` |

`allows_tab_change` is false only while saving the login snapshot. `allows_command(false)` is also false only while saving; `allows_command(true)` remains true so a repeated `:login` receives the existing handoff-in-progress notice.

Implement transitions with these exact rules:

- `Lost` from Running or SavingLogin moves to Relaunching and returns relaunch, detach-all, and `browser gone, restarting`. It is ignored from Login, Missing, or Relaunching.
- `Back` moves to Running and returns reopen-focused.
- `LoginRequested` reports the persistent-profile error when unavailable; from Running it moves to SavingLogin and returns save-for-login plus `saving session for login`; from any other state it returns `a browser handoff is already in progress`.
- `LoginSaved` acts only from SavingLogin. Success moves to Login and returns login, detach-all, and the visible-login instruction. Failure moves to Running and returns the existing save-session error.
- `LoginFinished` acts only from Login. It moves to Relaunching, requests relaunch, and returns the existing success or failure status.
- `RelaunchFinished` acts only from Relaunching. It moves to Missing and returns `no browser: {message}. any key retries` on failure.

- [ ] **Step 4: Run the direct browser tests to verify GREEN**

Run:

```bash
cargo test -p wwt browser::tests
```

Expected: all nine direct browser tests pass.

---

### Task 3: Route `Session` through the browser lifecycle

**Files:**
- Modify: `crates/wwt/src/session.rs`
- Test: `crates/wwt/src/session.rs`

**Interfaces:**
- Consumes: `BrowserLifecycle`, work gating, signals, and outcomes.
- Produces: a `Session` with no browser-state enum or login-availability field of its own.

- [ ] **Step 1: Replace fields and add outcome translation**

Remove `BrowserState` and `login_available` from `session.rs`. Replace `browser: BrowserState` with `browser: BrowserLifecycle`, initialized as `BrowserLifecycle::new(true)`. Keep `Session::set_login_available` public and delegate it.

Add:

```rust
fn gate_page_work(&mut self, work: PageWork, effects: &mut Vec<Effect>) -> bool {
    match self.browser.gate(work) {
        WorkDecision::Proceed => true,
        WorkDecision::Relaunch => {
            effects.push(Effect::Relaunch);
            false
        }
        WorkDecision::Blocked => false,
    }
}

fn apply_browser_outcome(&mut self, outcome: BrowserOutcome, effects: &mut Vec<Effect>) {
    match outcome.tabs {
        TabDirective::Keep => {}
        TabDirective::DetachAll => {
            for tab in &mut self.tabs {
                tab.detach();
            }
        }
        TabDirective::ReopenFocused => {
            let id = self.focused_id();
            self.reattach(id, effects);
        }
    }
    if let Some(status) = outcome.status {
        self.focused_mut().state = match status {
            BrowserStatus::Notice(message) => State::Notice(message),
            BrowserStatus::Error(message) => State::Error(message),
        };
    }
    if let Some(request) = outcome.request {
        match request {
            BrowserRequest::Relaunch => effects.push(Effect::Relaunch),
            BrowserRequest::SaveForLogin => {
                self.mode = Mode::Normal;
                effects.push(Effect::SaveForLogin(self.snapshot()));
            }
            BrowserRequest::Login => effects.push(Effect::Login),
        }
    }
}
```

- [ ] **Step 2: Route browser events and user work**

- Replace `on_browser_lost`, `ask_for_a_browser`, and `on_browser_back` with calls to `transition(Lost)` and `transition(Back)` plus `apply_browser_outcome`.
- In `run_action`, classify page-touching actions as `PageWork::Request`; use `allows_tab_change` for `TabAt` and `TabClose`.
- In `run_command`, use `allows_command(command == Command::Login)`, gate page-touching commands, and route `Command::Login` through `BrowserSignal::LoginRequested`.
- In `activate_reader`, gate `PageWork::Request`.
- Where tab close or focus needs to know whether browser effects can be emitted, use `browser.running()`.

- [ ] **Step 3: Route browser jobs and page results**

At the start of `on_job`:

- Always apply `Job::Unsaved` to the focused statusline.
- Route `Job::Relaunched`, `Job::Login`, and `Job::LoginSaved` through `RelaunchFinished`, `LoginFinished`, and `LoginSaved`, then return.
- Classify every remaining tab-specific job, including `Opened`, as `PageWork::Result`; return unless the gate says `Proceed`.

Keep the existing tab-ID lookup and all page lifecycle completion code unchanged after that gate.

- [ ] **Step 4: Update internal tests without exposing production state**

Replace the two tests that assign `BrowserState::Missing` directly by driving the lifecycle through `BrowserLost` followed by failed `Relaunched`, or add one `#[cfg(test)]` helper on `BrowserLifecycle` if driving the public event seam would change what the test is trying to isolate. Replace the one direct Running assertion with `session.browser.running()`.

- [ ] **Step 5: Run focused and full Session verification**

Run:

```bash
cargo test -p wwt browser::tests
cargo test -p wwt session::tests
cargo test -p wwt core::tests
cargo test -p wwt --test supervisor
cargo test -p wwt
```

Expected: every command passes. Login, private-session refusal, deliberate disconnect, relaunch retry, lazy focused-tab restore, late-page-result rejection, and cached frame preservation remain unchanged.

- [ ] **Step 6: Prove `Session` no longer owns browser state transitions**

Run:

```bash
sed '/^#\[cfg(test)\]/,$d' crates/wwt/src/session.rs \
  | rg -n 'BrowserState|login_available:|browser\s*=|browser\s*[!=]='
```

Expected: no match. The required public `set_login_available` method remains, but its
implementation only delegates to `BrowserLifecycle`; production `Session` interacts with browser
availability only through lifecycle methods and outcome application.

---

### Task 4: Document, verify, and commit phase 3

**Files:**
- Modify: `CONTEXT.md`
- Verify: all phase 3 files

**Interfaces:**
- Consumes: the completed browser lifecycle seam.
- Produces: one reviewed local phase 3 commit ready for manual browser-loss and login testing.

- [ ] **Step 1: Record browser lifecycle ownership in `CONTEXT.md`**

Add after the **Browser generation** entry:

```markdown
**Browser lifecycle** — the private five-state policy for running, saving a login
snapshot, visible login, missing Chromium, and relaunch in flight. It gates page work,
suppresses duplicate relaunches, and returns status, browser requests, and tab
directives; `Session` applies those outcomes without inspecting the state.
`wwt::browser::BrowserLifecycle`.
```

- [ ] **Step 2: Inspect the complete phase diff**

Run:

```bash
git diff --check
git diff --stat
git status --short
git diff -- CONTEXT.md crates/wwt/src/lib.rs crates/wwt/src/browser.rs crates/wwt/src/session.rs docs/superpowers/plans/2026-08-28-wwt-browser-lifecycle.md
```

Expected: only the five phase 3 paths appear. There are no public interface, dependency, persistence, browser-process, generation, retry, configuration, snapshot, pixel, or site-specific changes.

- [ ] **Step 3: Run the full verification gate**

Run:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: tests and Clippy exit 0. The previously authorized workspace rustfmt baseline mismatch may remain; record it without reformatting unrelated files.

- [ ] **Step 4: Commit phase 3 locally**

Run:

```bash
git add CONTEXT.md crates/wwt/src/lib.rs crates/wwt/src/browser.rs crates/wwt/src/session.rs docs/superpowers/plans/2026-08-28-wwt-browser-lifecycle.md
git commit -m "refactor(wwt): centralize browser lifecycle"
```

Expected: one local commit and a clean worktree. Do not push phase 3.

- [ ] **Step 5: Stop for manual verification**

Report the module interface, red-green evidence, changed files, verification results, commit hash, and behavior that could be affected. Ask the user to test browser loss and automatic recovery, failed relaunch and one-key retry, login entry and exit, login failure recovery, private-session login refusal, tab switching during visible login, and quitting during visible login. Do not plan or begin phase 4 until the user approves phase 3.
