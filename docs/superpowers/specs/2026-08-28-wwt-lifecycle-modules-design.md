# WWT lifecycle modules

**Date:** 2026-08-28
**Status:** Approved design
**Parent specs:** `2026-08-19-wwt-design.md`, `2026-08-21-wwt-m4-design.md`,
`2026-08-22-wwt-m5-design.md`, `2026-08-23-wwt-m7-design.md`, and
`2026-08-24-wwt-m8-design.md` govern behavior.

This document changes code ownership, not WWT behavior. The parent specs win if this
document implies a different user-visible result.

## 1. Why these modules change

`Session` is still the correct owner of browser decisions. It takes an `Event`, changes
in-memory state, emits `Effect`s, and composes a `Frame` without browser or terminal I/O.
`Core` is still the correct adapter. It turns external input into events and performs
effects without choosing policy.

The implementation inside those owners has grown around four lifecycles:

- A tab's live and reader views share one read slot, fallback state, freshness, and
  completion rules. `Session` currently coordinates those rules through direct writes
  to many `Tab` fields.
- Snapshot persistence has one ordered writer, a debounce deadline, and two durability
  barriers. `Core` currently holds each part separately.
- Browser availability has five states and controls retry, login, and page access.
  Checks and transitions currently appear at each action and job entry point.
- Pixel presentation has one global preference, one focused screencast, one picture,
  and mandatory frame acknowledgements. Its rules currently span mode changes, focus,
  reader view, resize, frame completion, composition, and quit.

The refactor gives each lifecycle one private module. A module qualifies only when its
interface hides more state and sequencing than it exposes. File splitting without such
an interface is out of scope.

## 2. Invariants

The refactor preserves these rules:

- `Session` remains the only owner of application policy and mutable browsing state.
- `Core` remains an adapter and owns external resources such as `Page`, `Chromium`, the
  terminal renderer, and spawned tasks.
- `Event`, `Effect`, `Job`, `Session`, and `Core` keep their public interfaces.
- A failed operation never blanks the visible frame.
- A tab has at most one page read in flight. A dirty signal during that read causes at
  most one follow-up read.
- A script failure selects snapshot extraction. A timeout marks the page stalled and
  does not select snapshot extraction.
- Work from a closed tab or an old browser generation cannot change current state.
- Debounced saves cannot overtake login or shutdown saves.
- Login starts only after the login snapshot reaches disk.
- Shutdown waits for the final snapshot to reach disk.
- Only the focused live page screencasts. Reader view suppresses its screencast without
  changing the global pixel preference.
- Every screencast frame is acknowledged, including a stale, hidden, or undecodable
  frame.
- Chromium keeps normal begin-frame pacing. Screencast acknowledgements retain the
  existing 33 ms minimum interval.

## 3. Module map

All four modules are private to the `wwt` crate:

```text
Session -> page_view
Session -> browser
Session -> pixel
Core    -> persistence

Session -> Event and Effect
Core    -> Session and external adapters
```

The dependency direction does not change. The lifecycle modules know WWT domain types.
They do not know Chromium clients, pages, terminal streams, Tokio task handles, or
renderer state, except that `persistence` owns its writer task because that task is the
mechanism the module hides.

## 4. The page-view lifecycle owns read coordination

A private `page_view` module owns the state that describes what a tab can show and what
page answer it awaits. `Tab` retains its identity, target presence, focus recency, and a
`PageView`. `Session` stops mutating `PageView` fields directly.

The module represents the single read slot with an enum instead of the current shared
`reading` Boolean. The variants distinguish an idle slot from the kind of read in
flight. Live-page freshness and reader freshness remain separate because one page
change invalidates both representations, while a reader result refreshes only reader
content.

The conceptual interface is:

```rust
page.changed();
page.begin_read(ReadDemand) -> Option<ReadRequest>;
page.complete(ReadResult) -> PageOutcome;
page.begin_navigation();
page.detach();
page.render_state() -> RenderState;
page.snapshot_state() -> SnapshotState;
```

`ReadDemand` says which representation the focused tab needs. `ReadRequest` says which
existing `Effect` the session must emit. `ReadResult` wraps the existing job answer.
`PageOutcome` reports state changes and any follow-up demand without exposing the read
slot or freshness flags.

`RenderState` and `SnapshotState` are borrowed projections. They expose the data needed
for composition and persistence without adding a getter for every field.

`Session` continues to decide when focus, mode, pixel preference, or reader intent makes
a representation visible. The page-view module decides whether a read may start, how an
answer changes the cached representation, and whether one follow-up read is due.

## 5. Persistence owns write ordering

A private `persistence` module owns the save worker, the session-file path, the pending
snapshot, the debounce deadline, and the shutdown completion receiver. `Core` owns one
`Persistence` value instead of those fields.

The conceptual interface is:

```rust
persistence.request(SaveIntent, snapshot, now);
persistence.deadline() -> Option<Instant>;
persistence.flush_due();
persistence.finish().await;
```

`SaveIntent` has three variants:

- `Debounced` replaces the pending snapshot and moves the deadline.
- `LoginBarrier` cancels a pending debounced save and queues the exact login snapshot.
  Completion returns through the existing `Job::LoginSaved` path.
- `ShutdownBarrier` cancels a pending debounced save, queues the exact final snapshot,
  and records the completion receiver that `finish` awaits.

The worker stays single and ordered. The module reports an ordinary write failure
through the existing `Job::Unsaved` path. `Core` only supplies time to the debounce and
selects on the deadline.

## 6. Browser lifecycle owns availability policy

A private `browser` module inside the session layer owns the existing running,
saving-login, login, missing, and relaunching states. It also owns the fixed fact that a
private session cannot perform a login handoff.

The conceptual interface is:

```rust
browser.gate(PageWork) -> WorkDecision;
browser.transition(BrowserSignal) -> BrowserOutcome;
```

`Session` classifies an action or command as local work or page work. `gate` decides
whether page work proceeds, requests one relaunch, or stays blocked during a deliberate
login handoff. `transition` applies browser loss, replacement, login request, login-save
completion, and visible-login completion.

`BrowserOutcome` carries the existing effect and status change that the transition
requires. It does not contain tab state. `Session` applies the outcome to the focused
tab and detaches or reopens tabs as required.

`Core` retains process ownership, browser generations, stale-result filtering, relaunch
backoff, and the visible Chromium handoff. Moving those mechanisms into the state
machine would make the domain depend on an adapter.

## 7. Pixel presentation owns screencast policy

A private `pixel` module owns the global enabled state, the terminal output form, the
latest picture, and the picture generation. It also owns the rules that start or stop a
screencast when focus, reader view, size, or pixel preference changes.

The conceptual interface is:

```rust
pixel.transition(PresentationChange) -> PresentationOutcome;
pixel.accept_frame(FocusedPage, ScreencastFrame) -> FrameOutcome;
pixel.paint(&mut Frame);
```

`FocusedPage` is a temporary projection. It contains only the tab ID, whether a target
is attached, and whether reader view is active. The pixel module cannot inspect or
mutate a `Tab`.

`PresentationChange` covers preference changes, focus changes, reader entry or exit,
resize, and quit. `PresentationOutcome` contains the existing start, stop, and image
cleanup actions. `accept_frame` always returns the acknowledgement effect. It replaces
the picture only when the frame belongs to the focused live page and decoding succeeds.

The module must pass the depth test during implementation. If the caller must expose
more tab state or coordinate the same sequence outside the module, the seam is shallow.
In that case the phase records the rejected design and leaves pixel behavior in
`Session`.

## 8. Main flows

The external flow stays the same:

```text
terminal or Chromium input
-> Core creates Event
-> Session applies a lifecycle transition
-> Session returns Effects
-> Core performs Effects
```

### A page becomes dirty during a read

1. `Session` passes the change to `PageView`.
2. `PageView` marks both representations stale and records one pending follow-up.
3. The current result completes the read slot.
4. `PageView` returns the result update and at most one next `ReadRequest`.
5. `Session` emits the corresponding existing effect.

### Chromium disappears

1. `Core` sends `Event::BrowserLost` for the current browser generation.
2. The browser lifecycle moves from running to missing and requests relaunch.
3. `Session` detaches every tab without deleting cached content.
4. `Core` advances the browser generation before it drops the old process.
5. A replacement moves the lifecycle to running and reopens only the focused tab.

### Login creates a durability barrier

1. The browser lifecycle accepts a login request only from a running persistent
   session.
2. `Session` emits `Effect::SaveForLogin` with the exact current snapshot.
3. `Persistence` cancels the debounced snapshot and queues the barrier snapshot.
4. `Job::LoginSaved(Ok(()))` lets the browser lifecycle emit `Effect::Login`.
5. `Core` advances the browser generation and hands the profile to visible Chromium.

### A screencast frame arrives after focus moves

1. `Session` gives the frame and `FocusedPage` projection to pixel presentation.
2. Pixel presentation returns `Effect::AckFrame` before it decides whether to paint.
3. The frame ID differs from the focused ID, so the old picture stays visible until the
   new focused page produces a frame.

## 9. Delivery and verification

Implementation uses four ordered phases:

1. Deepen the page-view lifecycle.
2. Deepen snapshot persistence.
3. Concentrate browser lifecycle policy.
4. Deepen pixel presentation if its final interface passes the depth test.

Each phase starts with missing characterization tests. The tests pass against the old
implementation before code moves. Existing `Session` and `Core` tests remain at the
external seam. New direct tests cover invariants hidden by a lifecycle module.

Before each phase is committed, run:

```text
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Run any relevant browser fixture or performance measurement for the phase. Inspect the
diff for unrelated changes and debug output. Each phase receives a separate local
commit. Do not push the commits without a separate instruction.

After each commit, stop for manual verification. Report what changed, which behavior
could be affected, the automated results, the commit hash, and exact manual steps. Do
not start the next phase until the manual result is approved.

## 10. Rejected designs

### Split files around the current methods

Moving extension implementations into smaller files would shorten `session.rs` and
`core.rs`, but it would preserve direct field mutation and lifecycle sequencing in the
caller. Deleting such a file would return the same scattered implementation without
recovering a coherent rule. This design fails the depth test.

### Extract stateless transition helpers

Pure helpers would make individual branches easier to test, but each caller would still
hold and synchronize the state. Their arguments would reproduce the current fields and
their call order would remain part of the interface. This design also fails the depth
test.

### Move browser policy into `Core`

`Core` owns the Chromium process, but `Session` owns whether an action may touch a page
and what the statusline says. Moving availability policy into `Core` would split one
decision across both sides of the event/effect seam. The process mechanism stays in
`Core`; the lifecycle policy stays in `Session`.

### Preserve old field access beside the new modules

A compatibility layer would create two ways to change the same state. Each phase moves
all callers in one change and removes the replaced field access before commit.

## 11. Non-goals

This work does not add features, dependencies, public configuration, snapshot fields,
browser flags, codecs, protocols, retries, or site-specific behavior. It does not change
scroll latency, screencast pacing, session-file format, reader behavior, or Chromium
launch policy.
