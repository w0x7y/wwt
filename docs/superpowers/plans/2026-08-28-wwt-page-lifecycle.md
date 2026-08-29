# WWT Page Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the page-read, fallback, navigation, hint, and detachment rules behind a
private borrowed `PageLifecycle<'_>` without changing `Tab` or WWT behavior.

**Architecture:** `Tab` keeps its public fields and methods. A private `page_view` module
borrows one `Tab` and applies complete lifecycle transitions. `Session` supplies global
context, converts `ReadRequest` values into existing `Effect`s, and applies global
outcomes such as saving or stopping a screencast.

**Tech Stack:** Rust 2024, Cargo workspace tests, Clippy, rustfmt.

**Spec:** `docs/superpowers/specs/2026-08-28-wwt-lifecycle-modules-design.md`

## Global Constraints

- Preserve all user-visible behavior and the public `Tab`, `Session`, `Event`, `Effect`,
  `Job`, and `Core` interfaces.
- Add no dependencies, configuration, snapshot fields, browser flags, codecs, retries,
  protocols, or site-specific behavior.
- Keep `Session` free of browser, terminal, filesystem, and task I/O.
- Keep Chromium begin-frame pacing and the 33 ms screencast acknowledgement interval.
- Never blank the visible frame on a failure.
- Keep the main checkout's uncommitted `TODO.md` change untouched.
- Make one local phase commit. Do not push it.
- Stop after the phase commit for the user's manual verification.

## File map

- Create `crates/wwt/src/page_view.rs`: borrowed `PageLifecycle`, read request and result
  types, transition outcomes, and direct lifecycle tests.
- Modify `crates/wwt/src/lib.rs`: register the private `page_view` module.
- Modify `crates/wwt/src/tab.rs`: keep the public shape and delegate existing lifecycle
  methods to `PageLifecycle`.
- Modify `crates/wwt/src/session.rs`: replace direct lifecycle coordination with the
  private module and translate outcomes into existing effects.
- Modify `CONTEXT.md`: name `PageLifecycle` as the private owner of a tab's transition
  rules without changing the meaning of **Tab** or **Session**.

---

### Task 1: Lock the existing Session behavior

**Files:**
- Test: `crates/wwt/src/session.rs`
- Test: `crates/wwt/src/tab.rs`

**Interfaces:**
- Consumes: the current `Session::on(Event) -> Vec<Effect>` seam.
- Produces: a recorded green baseline for the lifecycle cases that the refactor moves.

- [ ] **Step 1: Run the shared-read characterizations**

Run:

```bash
cargo test -p wwt session::tests::a_dirty_signal_during_an_extraction_re_runs_it_once_not_twice
cargo test -p wwt session::tests::one_reader_request_uses_the_shared_read_slot
cargo test -p wwt session::tests::an_ordinary_answer_hands_the_shared_slot_to_a_pending_reader
```

Expected: all three commands pass.

- [ ] **Step 2: Run the fallback characterizations**

Run:

```bash
cargo test -p wwt session::tests::a_read_that_timed_out_stalls_the_tab_and_does_not_degrade_it
cargo test -p wwt session::tests::a_script_that_threw_still_reaches_for_the_snapshot
cargo test -p wwt session::tests::a_status_read_that_throws_degrades_the_tab_like_an_extraction_does
cargo test -p wwt session::tests::a_snapshot_that_also_fails_is_the_end_of_the_line
```

Expected: all four commands pass.

- [ ] **Step 3: Run the document, hint, and detachment characterizations**

Run:

```bash
cargo test -p wwt session::tests::every_navigation_entry_leaves_reader_and_forgets_its_old_document
cargo test -p wwt session::tests::a_query_that_failed_leaves_f_working
cargo test -p wwt tab::tests::a_detached_tab_keeps_what_it_looked_like_and_loses_what_it_was_waiting_for
```

Expected: all three commands pass.

---

### Task 2: Introduce the borrowed read lifecycle test-first

**Files:**
- Create: `crates/wwt/src/page_view.rs`
- Modify: `crates/wwt/src/lib.rs`

**Interfaces:**
- Consumes: `Tab`, `Source`, `Failure`, `Extraction`, `ReaderExtraction`, and `Status`.
- Produces:

```rust
pub(crate) struct PageLifecycle<'a>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadDemand {
    pub focused: bool,
    pub pixel: bool,
    pub columns: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadRequest {
    Extract(Source),
    Reader,
    Status,
}

pub(crate) enum ReadResult {
    Extracted(Source, Result<wwt_page::Extraction, Failure>),
    Reader(Result<wwt_page::ReaderExtraction, Failure>),
    Status(Result<Status, Failure>),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PageOutcome {
    pub next: Option<ReadRequest>,
    pub save: bool,
    pub reader_became_active: bool,
}
```

- [ ] **Step 1: Write the failing read-slot tests**

Add `mod page_view;` to `crates/wwt/src/lib.rs`. Create
`crates/wwt/src/page_view.rs` with imports and these tests before defining the types:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tab::{Presence, Tab, TabId};

    fn attached() -> Tab {
        let mut tab = Tab::new(TabId(0), "https://example.com".to_string());
        tab.presence = Presence::Attached;
        tab
    }

    fn text() -> ReadDemand {
        ReadDemand { focused: true, pixel: false, columns: 80, rows: 22 }
    }

    #[test]
    fn one_read_slot_refuses_a_second_request() {
        let mut tab = attached();
        assert_eq!(
            PageLifecycle::new(&mut tab).begin_read(text()),
            Some(ReadRequest::Extract(Source::Script))
        );
        tab.reader.wanted = true;
        tab.reader.dirty = true;
        assert_eq!(PageLifecycle::new(&mut tab).begin_read(text()), None);
    }

    #[test]
    fn a_dirty_signal_during_a_read_is_spent_after_completion() {
        let mut tab = attached();
        assert!(PageLifecycle::new(&mut tab).begin_read(text()).is_some());
        PageLifecycle::new(&mut tab).changed();
        assert!(tab.reading);
        assert!(tab.dirty);
    }

    #[test]
    fn a_clean_active_reader_does_not_fall_through_to_a_live_read() {
        let mut tab = attached();
        tab.reader.active = true;
        tab.reader.dirty = false;
        tab.dirty = true;
        assert_eq!(PageLifecycle::new(&mut tab).begin_read(text()), None);
    }
}
```

- [ ] **Step 2: Run the new tests to verify RED**

Run:

```bash
cargo test -p wwt page_view::tests
```

Expected: compilation fails because `PageLifecycle`, `ReadDemand`, and `ReadRequest` do
not exist.

- [ ] **Step 3: Implement read selection**

Define the types above and this interface:

```rust
pub(crate) struct PageLifecycle<'a> {
    tab: &'a mut Tab,
}

impl<'a> PageLifecycle<'a> {
    pub(crate) fn new(tab: &'a mut Tab) -> Self {
        Self { tab }
    }

    pub(crate) fn changed(&mut self) {
        self.tab.dirty = true;
        self.tab.reader.dirty = true;
        self.tab.hints = None;
    }

    pub(crate) fn begin_read(&mut self, demand: ReadDemand) -> Option<ReadRequest> {
        if !self.tab.attached() || self.tab.reading {
            return None;
        }
        if self.tab.reader.wanted || self.tab.reader.active {
            if !demand.focused || !self.tab.reader.dirty {
                return None;
            }
            self.tab.reading = true;
            self.tab.reader.dirty = false;
            return Some(ReadRequest::Reader);
        }
        if !self.tab.dirty || (!demand.focused && self.tab.read) {
            return None;
        }
        self.tab.reading = true;
        self.tab.dirty = false;
        if demand.focused && demand.pixel && self.tab.read && !self.tab.degraded {
            Some(ReadRequest::Status)
        } else {
            let source = if self.tab.degraded { Source::Snapshot } else { Source::Script };
            Some(ReadRequest::Extract(source))
        }
    }
}
```

- [ ] **Step 4: Run the direct tests to verify GREEN**

Run:

```bash
cargo test -p wwt page_view::tests
```

Expected: all three tests pass.

---

### Task 3: Move read completion behind the lifecycle

**Files:**
- Modify: `crates/wwt/src/page_view.rs`
- Modify: `crates/wwt/src/session.rs`

**Interfaces:**
- Consumes: `PageLifecycle::begin_read`, `ReadDemand`, and each existing read job.
- Produces: `PageLifecycle::complete(ReadResult, ReadDemand) -> PageOutcome`.

- [ ] **Step 1: Write the failing completion tests**

Add these tests to `page_view.rs`:

```rust
#[test]
fn a_script_failure_selects_snapshot_once() {
    let mut tab = attached();
    assert!(PageLifecycle::new(&mut tab).begin_read(text()).is_some());
    let outcome = PageLifecycle::new(&mut tab).complete(
        ReadResult::Extracted(
            Source::Script,
            Err(Failure::Failed("script broke".to_string())),
        ),
        text(),
    );
    assert!(tab.degraded);
    assert_eq!(outcome.next, Some(ReadRequest::Extract(Source::Snapshot)));
}

#[test]
fn a_timeout_stalls_without_selecting_snapshot() {
    let mut tab = attached();
    assert!(PageLifecycle::new(&mut tab).begin_read(text()).is_some());
    let outcome = PageLifecycle::new(&mut tab).complete(
        ReadResult::Extracted(Source::Script, Err(Failure::TimedOut)),
        text(),
    );
    assert_eq!(tab.state, State::Stalled);
    assert!(!tab.degraded);
    assert_eq!(outcome.next, None);
}
```

- [ ] **Step 2: Run the completion tests to verify RED**

Run:

```bash
cargo test -p wwt page_view::tests
```

Expected: compilation fails because `ReadResult` and `PageLifecycle::complete` do not
exist.

- [ ] **Step 3: Implement complete read transitions**

Implement `ReadResult`, `PageOutcome`, and:

```rust
pub(crate) fn complete(
    &mut self,
    result: ReadResult,
    demand: ReadDemand,
) -> PageOutcome
```

Use the existing transition table exactly:

| Result | State transition | Follow-up |
|---|---|---|
| Script extraction succeeds | clear `reading`; set `read`, runs, caret, and status | spend current demand |
| Script extraction fails | clear `reading`; set `degraded` and `dirty` | snapshot extraction |
| Any extraction times out | clear `reading`; set `State::Stalled` | none |
| Snapshot extraction fails | clear `reading`; set `State::Error` | none |
| Status succeeds | clear `reading`; apply status without setting `read` | spend current demand |
| Status script fails | clear `reading`; set `degraded` and `dirty` | snapshot extraction |
| Status times out | clear `reading`; set `State::Stalled` | none |
| Reader succeeds with blocks | clear `reading`; cache and lay out the document; activate if wanted | spend current demand |
| Reader succeeds without blocks | clear `reading`; report `nothing to read`; cancel first entry | spend current demand |
| Reader fails | clear `reading`; keep an active layout; cancel and re-dirty first entry | spend current demand |

Move the `chrome-error://` status handling and the comparison of URL, title, and
`scroll_y` into a private `apply_status`. Set `PageOutcome::save` only when that tuple
changes. Set `reader_became_active` only on the inactive-to-active transition.

- [ ] **Step 4: Replace Session read coordination**

Add these translators to `Session`:

```rust
fn read_demand(&self, id: TabId) -> ReadDemand
fn request_read(&mut self, id: TabId, effects: &mut Vec<Effect>)
fn apply_page_outcome(&mut self, id: TabId, outcome: PageOutcome, effects: &mut Vec<Effect>)
```

`request_read` maps requests without changing their meaning:

```rust
match request {
    ReadRequest::Extract(source) => effects.push(Effect::Extract(id, source)),
    ReadRequest::Reader => effects.push(Effect::ReadReader(id)),
    ReadRequest::Status => effects.push(Effect::ReadStatus(id)),
}
```

`apply_page_outcome` emits `Effect::Save(self.snapshot())` when `save` is true, emits
`Effect::StopScreencast(id)` when reader view became active on the focused pixel tab,
and maps `next` through the same request translator.

Replace `start_extract`, `start_reader`, `start_current_read`, and `apply_status` with
these three methods. Route `Job::Extracted`, `Job::Reader`, and `Job::Status` through
`PageLifecycle::complete`.

- [ ] **Step 5: Run the direct and Session tests to verify GREEN**

Run:

```bash
cargo test -p wwt page_view::tests
cargo test -p wwt session::tests
```

Expected: both commands pass.

---

### Task 4: Move document, navigation, hint, and detachment transitions

**Files:**
- Modify: `crates/wwt/src/page_view.rs`
- Modify: `crates/wwt/src/tab.rs`
- Modify: `crates/wwt/src/session.rs`

**Interfaces:**
- Consumes: the public `Tab::mark_dirty`, `Tab::replace_document`, and `Tab::detach`
  methods plus existing navigation and hint jobs.
- Produces: lifecycle methods that preserve those public methods and remove direct
  coordination from `Session`.

```rust
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum HintRequest {
    Cached(Vec<HintTarget>),
    Query(Source),
}
```

- [ ] **Step 1: Write failing direct transition tests**

Add tests that call the desired methods:

```rust
#[test]
fn detachment_keeps_content_and_clears_in_flight_work() {
    let mut tab = attached();
    tab.title = "cached title".to_string();
    tab.reading = true;
    tab.navigating = true;
    tab.hinting = true;
    PageLifecycle::new(&mut tab).detach();
    assert_eq!(tab.presence, Presence::Detached);
    assert_eq!(tab.title, "cached title");
    assert!(!tab.reading);
    assert!(!tab.navigating);
    assert!(!tab.hinting);
    assert!(tab.dirty);
    assert!(tab.reader.dirty);
}

#[test]
fn navigation_replaces_document_state_without_blank_content() {
    let mut tab = attached();
    tab.degraded = true;
    tab.title = "cached title".to_string();
    assert!(PageLifecycle::new(&mut tab).begin_navigation());
    assert!(tab.navigating);
    assert!(!tab.degraded);
    assert_eq!(tab.state, State::Loading);
    assert_eq!(tab.title, "cached title");
    assert!(tab.reader.document.is_none());
}
```

- [ ] **Step 2: Run the transition tests to verify RED**

Run:

```bash
cargo test -p wwt page_view::tests
```

Expected: compilation fails because `detach` and `begin_navigation` do not exist on
`PageLifecycle`.

- [ ] **Step 3: Implement and route the remaining transitions**

Implement these methods:

```rust
pub(crate) fn detach(&mut self)
pub(crate) fn replace_document(&mut self)
pub(crate) fn begin_navigation(&mut self) -> bool
pub(crate) fn navigation_settled(&mut self)
pub(crate) fn operation_failed(&mut self, failure: Failure)
pub(crate) fn begin_hints(&mut self) -> Option<HintRequest>
pub(crate) fn complete_hints(
    &mut self,
    result: Result<Vec<HintTarget>, Failure>,
) -> Result<Vec<HintTarget>, Failure>
```

Keep `Tab::mark_dirty`, `Tab::replace_document`, and `Tab::detach` public with their
current signatures. Their bodies delegate to a short-lived `PageLifecycle` so existing
callers remain source-compatible.

For navigation, keep the current order: refuse a second navigation, call
`Session::leave_reader` so it emits the existing presentation effects, then call
`PageLifecycle::begin_navigation`. Route `Job::Hints`, `Job::Settled`, `Job::Resized`,
and `Job::Failed` through the lifecycle methods. `HintRequest::Cached` enters hint mode
without an effect. `HintRequest::Query` emits the existing hint effect. Keep mode,
focus, effect emission, tab presence after opening, and hint-mode entry in `Session`
because those rules span more than one tab or page view.

- [ ] **Step 4: Check that Session no longer coordinates lifecycle fields**

Run:

```bash
sed '/^#\[cfg(test)\]/,$d' crates/wwt/src/session.rs \
  | rg -n 'tab\.(reading|dirty|degraded|navigating|hinting|hints)\s*='
```

Expected: no match. Tests may still construct states directly through the preserved
public interface.

- [ ] **Step 5: Run all `wwt` tests**

Run:

```bash
cargo test -p wwt
```

Expected: all tests pass.

---

### Task 5: Document, verify, and commit phase 1

**Files:**
- Modify: `CONTEXT.md`
- Verify: all phase files

**Interfaces:**
- Consumes: the completed borrowed lifecycle seam.
- Produces: one reviewed local phase commit ready for manual testing.

- [ ] **Step 1: Record the private lifecycle owner in CONTEXT.md**

Extend the existing **Tab** entry with this paragraph:

```markdown
**Page lifecycle** — the private transition rules for one **tab**: shared reads, live
and reader freshness, script fallback, navigation, hints, and detachment.
`PageLifecycle<'_>` borrows a tab for one transition, so `Tab` keeps its public shape
while `Session` stops coordinating its fields individually. `wwt::page_view`.
```

- [ ] **Step 2: Format and inspect the phase diff**

Run:

```bash
cargo fmt --all
git diff --check
git diff --stat
git diff -- crates/wwt/src/page_view.rs crates/wwt/src/tab.rs crates/wwt/src/session.rs crates/wwt/src/lib.rs CONTEXT.md
```

Expected: no whitespace errors, unrelated files, behavior changes, or public `Tab`
field changes.

- [ ] **Step 3: Run the full verification gate**

Run:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: every command exits 0 with no warnings.

- [ ] **Step 4: Commit the completed phase locally**

Run:

```bash
git add CONTEXT.md crates/wwt/src/lib.rs crates/wwt/src/page_view.rs crates/wwt/src/session.rs crates/wwt/src/tab.rs
git commit -m "refactor(wwt): centralize page lifecycle"
```

Expected: one local commit and a clean working tree. Do not push.

- [ ] **Step 5: Stop for manual verification**

Report the commit hash, changed files, automated results, and possible effects. Ask the
user to test text and pixel rendering, reader entry and exit, navigation, hints, tab
switching, browser restart, and one YouTube watch page. Do not plan or begin phase 2
until the user approves this phase.
