# WWT Pixel Presentation Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move pixel preference, screencast reconciliation, frame acceptance, picture storage, terminal output selection, and graphics generations out of `Session` into one private module without changing WWT behavior.

**Architecture:** Add a private `PixelPresentation` owned by `Session`. `Session` projects its focused tab into `FocusedPage`, asks the module to reconcile the desired screencast, and translates private presentation requests into existing effects. The module owns only pixel lifecycle state and picture rendering; `Session` keeps tabs, focus, readers, modes, effect ordering, and browser policy.

**Tech Stack:** Rust 2024 and the existing `wwt-frame`, `wwt-page`, `wwt-png`, `Effect`, `Session`, and `TabId` vocabulary.

**Spec:** `docs/superpowers/specs/2026-08-29-wwt-pixel-lifecycle-design.md`

## Global Constraints

- This phase changes code ownership, not WWT behavior. The approved pixel design and its parent specifications win.
- Preserve the public interfaces of `Event`, `Effect`, `Job`, `Session`, and `Core`.
- Preserve effect order, especially stop before start on focus and resize, acknowledgement before any frame consequence, and screencast stop before quit.
- Pixel preference remains global and unsaved. Reader view suppresses presentation without changing that preference.
- Only the focused, attached, live page may have a requested screencast.
- A closed target or lost browser is forgotten without a stop request. A live target that ceases to be desired receives a stop request.
- Keep the previous picture across focus changes, reader entry, and decode failures. Clear it only when leaving pixel mode.
- A frame from every existing tab is acknowledged. Only a frame from the focused live pixel page is accepted.
- Keep graphics payloads base64 encoded in `Arc<String>`. Decode only half-block output.
- Add no dependency, feature, setting, snapshot field, key binding, public type, or browser behavior.
- For every new interface, run its test first and record the expected missing-interface failure before adding production code.
- Keep existing Session characterization tests green after each integration cluster.
- Use `scripts/check-pixel-lifecycle.sh` as the durable ownership check.
- Before committing, run formatting, all workspace tests, clippy with warnings denied, the ownership script, and whitespace validation.
- Inspect the final diff for unrelated changes. Do not push or merge without a separate instruction.

---

## File map

- Create: `crates/wwt/src/pixel.rs` - private pixel state, screencast reconciliation, frame acceptance, painting, and direct invariant tests.
- Modify: `crates/wwt/src/lib.rs` - declare the private module.
- Modify: `crates/wwt/src/session.rs` - replace pixel fields and coordination helpers with the module interface.
- Create: `scripts/check-pixel-lifecycle.sh` - reject pixel lifecycle state or coordination returning to production `session.rs` and run focused tests.
- Modify: `CONTEXT.md` - record pixel presentation ownership and the focused-page projection.
- Create: `docs/superpowers/plans/2026-08-29-wwt-pixel-lifecycle.md` - phase 4 execution record.

---

### Task 1: Lock the existing external behavior

**Files:**
- Test: `crates/wwt/src/session.rs`

**Interfaces:**
- Consumes: existing pixel, reader, focus, resize, close, and frame behavior.
- Produces: baseline evidence before ownership moves.

- [x] **Step 1: Run the focused pixel characterizations**

Run:

```bash
cargo test -p wwt session::tests::p_turns_pixel_mode_on_and_asks_for_pictures
cargo test -p wwt session::tests::p_again_turns_it_off_and_stops_them
cargo test -p wwt session::tests::cached_reader_entry_stops_pixels_and_exit_starts_them_again
cargo test -p wwt session::tests::switching_tabs_moves_the_screencast_with_the_focus
cargo test -p wwt session::tests::the_previous_picture_stays_up_until_the_new_tabs_first_frame
cargo test -p wwt session::tests::a_resize_restarts_the_screencast_at_the_new_size
cargo test -p wwt session::tests::quitting_from_pixel_mode_stops_the_screencast_first
cargo test -p wwt session::tests::closing_the_focused_tab_moves_the_screencast_to_the_next_one
```

Expected: every test passes.

- [x] **Step 2: Run the frame and output characterizations**

Run:

```bash
cargo test -p wwt session::tests::a_frame_without_graphics_composes_to_half_block_cells
cargo test -p wwt session::tests::a_frame_with_graphics_still_composes_to_an_image
cargo test -p wwt session::tests::a_picture_that_cannot_be_decoded_leaves_the_last_one_up_and_is_still_acked
cargo test -p wwt session::tests::a_frame_for_a_tab_that_is_not_focused_is_acked_and_dropped
cargo test -p wwt session::tests::a_frame_is_acked_even_when_pixel_mode_has_already_been_left
cargo test -p wwt session::tests::a_frame_for_a_tab_that_is_gone_is_dropped_without_a_word
cargo test -p wwt session::tests::each_frame_composes_a_new_generation
cargo test -p wwt session::tests::a_resize_moves_the_picture_to_the_area_it_now_covers
```

Expected: every test passes. Do not change source in this task.

---

### Task 2: Drive the state and reconciliation interface from direct tests

**Files:**
- Create: `crates/wwt/src/pixel.rs`
- Modify: `crates/wwt/src/lib.rs`
- Test: `crates/wwt/src/pixel.rs`

**Interfaces:**
- Produces:

```rust
pub(crate) struct PixelPresentation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PixelOutput {
    Graphics,
    HalfBlocks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FocusedPage {
    pub(crate) id: TabId,
    pub(crate) attached: bool,
    pub(crate) reader_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PresentationRequest {
    Start(TabId, FrameSize),
    Stop(TabId),
    Ack(TabId, i64),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PixelOutcome {
    pub(crate) changed: bool,
    pub(crate) refresh_live_runs: bool,
}
```

- [x] **Step 1: Declare the module and add state tests before definitions**

Add `mod pixel;` to `crates/wwt/src/lib.rs`. Create `pixel.rs` with imports, viewport fixtures, and tests that refer to the not-yet-defined interface:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wwt_frame::{CellSize, GridSize};

    const TAB_0: TabId = TabId(0);
    const TAB_1: TabId = TabId(1);

    fn viewport() -> Viewport {
        Viewport::with_origin(
            GridSize { cols: 80, rows: 22 },
            CellSize { w: 9, h: 20 },
            1,
        )
    }

    fn page(id: TabId) -> FocusedPage {
        FocusedPage { id, attached: true, reader_active: false }
    }

    #[test]
    fn text_mode_requests_no_screencast() {
        let mut pixel = PixelPresentation::new();
        assert_eq!(pixel.reconcile(page(TAB_0), viewport()), vec![]);
    }

    #[test]
    fn enabling_starts_only_an_attached_live_page() {
        let mut pixel = PixelPresentation::new();
        assert_eq!(
            pixel.set_enabled(true),
            PixelOutcome { changed: true, refresh_live_runs: false }
        );
        assert!(matches!(
            pixel.reconcile(page(TAB_0), viewport()).as_slice(),
            [PresentationRequest::Start(TAB_0, _)]
        ));

        let mut reader = PixelPresentation::new();
        reader.set_enabled(true);
        assert_eq!(
            reader.reconcile(
                FocusedPage { id: TAB_0, attached: true, reader_active: true },
                viewport(),
            ),
            vec![]
        );

        let mut opening = PixelPresentation::new();
        opening.set_enabled(true);
        assert_eq!(
            opening.reconcile(
                FocusedPage { id: TAB_0, attached: false, reader_active: false },
                viewport(),
            ),
            vec![]
        );
    }

    #[test]
    fn focus_change_stops_before_it_starts() {
        let mut pixel = PixelPresentation::new();
        pixel.set_enabled(true);
        pixel.reconcile(page(TAB_0), viewport());
        assert!(matches!(
            pixel.reconcile(page(TAB_1), viewport()).as_slice(),
            [PresentationRequest::Stop(TAB_0), PresentationRequest::Start(TAB_1, _)]
        ));
    }

    #[test]
    fn forgetting_a_gone_target_emits_no_stop() {
        let mut pixel = PixelPresentation::new();
        pixel.set_enabled(true);
        pixel.reconcile(page(TAB_0), viewport());
        pixel.forget(TAB_0);
        assert_eq!(pixel.stop(), None);
    }

    #[test]
    fn resize_restarts_at_the_new_size() {
        let mut pixel = PixelPresentation::new();
        pixel.set_enabled(true);
        pixel.reconcile(page(TAB_0), viewport());
        let larger = Viewport::with_origin(
            GridSize { cols: 100, rows: 28 },
            CellSize { w: 9, h: 20 },
            1,
        );
        assert!(matches!(
            pixel.restart(page(TAB_0), larger).as_slice(),
            [PresentationRequest::Stop(TAB_0), PresentationRequest::Start(TAB_0, _)]
        ));
    }

    #[test]
    fn disabling_clears_once_and_refreshes_live_runs() {
        let mut pixel = PixelPresentation::new();
        assert_eq!(pixel.set_enabled(false), PixelOutcome::default());
        pixel.set_enabled(true);
        assert_eq!(
            pixel.set_enabled(false),
            PixelOutcome { changed: true, refresh_live_runs: true }
        );
        assert_eq!(pixel.set_enabled(false), PixelOutcome::default());
        assert!(!pixel.enabled());
    }

    #[test]
    fn stop_returns_the_requested_target_once() {
        let mut pixel = PixelPresentation::new();
        pixel.set_enabled(true);
        pixel.reconcile(page(TAB_0), viewport());
        assert_eq!(pixel.stop(), Some(PresentationRequest::Stop(TAB_0)));
        assert_eq!(pixel.stop(), None);
    }
}
```

- [x] **Step 2: Run the direct tests and confirm red**

Run:

```bash
cargo test -p wwt pixel::tests
```

Expected: compilation fails because `PixelPresentation`, `PixelOutcome`, `FocusedPage`, and `PresentationRequest` do not exist.

- [x] **Step 3: Implement the smallest state model and reconciliation**

Add the production state above the tests:

```rust
pub(crate) struct PixelPresentation {
    output: PixelOutput,
    mode: PixelMode,
    requested: Option<RequestedScreencast>,
    generation: u64,
}

enum PixelMode {
    Text,
    Pixel { picture: Option<Picture> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RequestedScreencast {
    tab: TabId,
    size: FrameSize,
}

impl PixelPresentation {
    pub(crate) fn new() -> Self;
    pub(crate) fn set_output(&mut self, output: PixelOutput);
    pub(crate) fn set_enabled(&mut self, on: bool) -> PixelOutcome;
    pub(crate) fn reconcile(
        &mut self,
        focused: FocusedPage,
        viewport: Viewport,
    ) -> Vec<PresentationRequest>;
    pub(crate) fn restart(
        &mut self,
        focused: FocusedPage,
        viewport: Viewport,
    ) -> Vec<PresentationRequest>;
    pub(crate) fn forget(&mut self, id: TabId);
    pub(crate) fn stop(&mut self) -> Option<PresentationRequest>;
    pub(crate) fn enabled(&self) -> bool;
}
```

Implement one private `desired(focused, viewport)` calculation. `frame_size` belongs inside the module: graphics use CSS dimensions; half-blocks use `cols * 2` by `rows * 4`. Reconciliation compares `desired` with `requested` and emits only the four transitions specified in the design.

- [x] **Step 4: Run direct tests green**

Run:

```bash
cargo test -p wwt pixel::tests
```

Expected: all state and reconciliation tests pass.

---

### Task 3: Drive frame acceptance and painting from direct tests

**Files:**
- Modify: `crates/wwt/src/pixel.rs`
- Test: `crates/wwt/src/pixel.rs`

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FrameOutcome {
    pub(crate) request: Option<PresentationRequest>,
    pub(crate) notice: Option<String>,
}

impl PixelPresentation {
    pub(crate) fn accept_frame(
        &mut self,
        source: TabId,
        source_exists: bool,
        focused: FocusedPage,
        frame: ScreencastFrame,
        viewport: Viewport,
    ) -> FrameOutcome;

    pub(crate) fn paint(&self, frame: &mut Frame, viewport: Viewport);
}
```

- [x] **Step 1: Add frame tests before methods**

Add tests covering these exact cases:

```rust
#[test]
fn a_closed_source_is_neither_acked_nor_painted() {
    let mut pixel = graphics_pixel();
    let outcome = pixel.accept_frame(TAB_0, false, page(TAB_1), frame("GONE", 7), viewport());
    assert_eq!(outcome, FrameOutcome::default());
    assert!(painted(&pixel).image().is_none());
}

#[test]
fn an_existing_hidden_frame_is_acked_and_discarded() {
    let mut pixel = graphics_pixel();
    let outcome = pixel.accept_frame(TAB_1, true, page(TAB_0), frame("STALE", 7), viewport());
    assert_eq!(outcome.request, Some(PresentationRequest::Ack(TAB_1, 7)));
    assert!(painted(&pixel).image().is_none());
}

#[test]
fn an_existing_frame_after_disable_is_acked_and_discarded() {
    let mut pixel = graphics_pixel();
    pixel.set_enabled(false);
    let outcome = pixel.accept_frame(TAB_0, true, page(TAB_0), frame("LATE", 7), viewport());
    assert_eq!(outcome.request, Some(PresentationRequest::Ack(TAB_0, 7)));
    assert!(painted(&pixel).image().is_none());
}

#[test]
fn graphics_keep_the_payload_and_advance_generation() {
    let mut pixel = graphics_pixel();
    pixel.accept_frame(TAB_0, true, page(TAB_0), frame("SAME", 1), viewport());
    let first = painted(&pixel).image().expect("first image").clone();
    pixel.accept_frame(TAB_0, true, page(TAB_0), frame("SAME", 2), viewport());
    let second = painted(&pixel).image().expect("second image").clone();
    assert_eq!(second.payload.as_str(), "SAME");
    assert_ne!(first.generation, second.generation);
}

#[test]
fn half_blocks_decode_and_paint_the_fixture() {
    let mut pixel = half_block_pixel();
    pixel.accept_frame(TAB_0, true, page(TAB_0), fixture_frame(), viewport());
    let frame = painted(&pixel);
    assert_eq!(frame.cell(CellPos { col: 0, row: 1 }).expect("painted").ch, '\u{2580}');
    assert!(frame.image().is_none());
}

#[test]
fn a_bad_half_block_frame_keeps_the_previous_picture() {
    let mut pixel = half_block_pixel();
    pixel.accept_frame(TAB_0, true, page(TAB_0), fixture_frame(), viewport());
    let outcome = pixel.accept_frame(
        TAB_0,
        true,
        page(TAB_0),
        frame("not a picture", 7),
        viewport(),
    );
    assert_eq!(outcome.request, Some(PresentationRequest::Ack(TAB_0, 7)));
    assert_eq!(outcome.notice.as_deref(), Some("that picture could not be read"));
    assert_eq!(painted(&pixel).cell(CellPos { col: 0, row: 1 }).expect("old picture").ch, '\u{2580}');
}

#[test]
fn resize_moves_a_graphics_picture_and_advances_generation() {
    let mut pixel = graphics_pixel();
    pixel.accept_frame(TAB_0, true, page(TAB_0), frame("IMAGE", 1), viewport());
    let before = painted(&pixel).image().expect("image").generation;
    let larger = larger_viewport();
    pixel.restart(page(TAB_0), larger);
    let mut frame = Frame::new(GridSize { cols: 100, rows: 30 });
    pixel.paint(&mut frame, larger);
    let image = frame.image().expect("resized image");
    assert_eq!(image.area.cols, 100);
    assert_eq!(image.area.rows, 28);
    assert_ne!(image.generation, before);
}
```

Define test helpers `graphics_pixel`, `half_block_pixel`, `frame`, `fixture_frame`, `painted`, and `larger_viewport` with concrete WWT types. `fixture_frame` must read `../../wwt-png/tests/fixtures/screencast.txt`, as the existing Session fixture does.

- [x] **Step 2: Run the frame tests and confirm red**

Run:

```bash
cargo test -p wwt pixel::tests
```

Expected: compilation fails because `FrameOutcome`, `accept_frame`, and `paint` do not exist.

- [x] **Step 3: Implement frame storage and painting**

Add:

```rust
#[derive(Debug, Clone, PartialEq)]
enum Picture {
    Graphics(Image),
    Blocks(Samples),
}
```

`accept_frame` must return immediately for an absent source. For every existing source it first constructs `Ack(source, frame.ack)`, then discards unless pixel mode is enabled, the source equals the focused page, and that page is attached, live, and the requested target. Graphics frames increment `generation` and store an `Image` with an `Arc<String>` payload. Half-block frames decode and resample to `cols` by `rows * 2`; a failure returns the notice and retains the old picture.

`paint` matches `Picture`: clone an `Image` into `Frame::set_image`, or paint `Samples` into the page `CellRect`. Text mode and pixel mode without a picture paint nothing. `restart` also moves a retained graphics image to the new page area and advances its generation.

- [x] **Step 4: Run all direct module tests green**

Run:

```bash
cargo test -p wwt pixel::tests
```

Expected: all direct tests pass.

---

### Task 4: Integrate mode changes, composition, and frames

**Files:**
- Modify: `crates/wwt/src/session.rs`
- Test: `crates/wwt/src/session.rs`

**Interfaces:**
- Consumes: `PixelPresentation`, `PixelOutput`, `FocusedPage`, `PixelOutcome`, and `PresentationRequest`.
- Produces: one module field plus small translation helpers.

- [x] **Step 1: Replace Session's pixel storage**

Remove `Picture` and the fields `graphics`, `pixel`, `picture`, and `generations`. Add:

```rust
use crate::pixel::{
    FocusedPage, PixelOutcome, PixelOutput, PixelPresentation, PresentationRequest,
};

pub struct Session {
    // existing fields
    pixel: PixelPresentation,
    // existing fields
}
```

Initialize it with `PixelPresentation::new()`.

- [x] **Step 2: Add the only Session adapters**

Add these helpers:

```rust
fn focused_page(&self) -> FocusedPage {
    let tab = self.focused();
    FocusedPage {
        id: tab.id,
        attached: tab.attached(),
        reader_active: tab.reader.active,
    }
}

fn emit_presentation_requests(
    &self,
    requests: impl IntoIterator<Item = PresentationRequest>,
    effects: &mut Vec<Effect>,
) {
    effects.extend(requests.into_iter().map(|request| match request {
        PresentationRequest::Start(id, size) => Effect::StartScreencast(id, size),
        PresentationRequest::Stop(id) => Effect::StopScreencast(id),
        PresentationRequest::Ack(id, ack) => Effect::AckFrame(id, ack),
    }));
}

fn reconcile_pixels(&mut self, effects: &mut Vec<Effect>) {
    let focused = self.focused_page();
    let requests = self.pixel.reconcile(focused, self.vp);
    self.emit_presentation_requests(requests, effects);
}

fn apply_pixel_outcome(&mut self, outcome: PixelOutcome, effects: &mut Vec<Effect>) {
    if !outcome.changed {
        return;
    }
    self.clear_hints();
    self.reconcile_pixels(effects);
    if outcome.refresh_live_runs {
        for tab in &mut self.tabs {
            PageLifecycle::new(tab).live_changed();
        }
        let id = self.focused_id();
        if !self.focused().reader.active {
            self.request_read(id, effects);
        }
    }
}
```

If borrowing makes `emit_presentation_requests(&self, ...)` awkward, make it an associated function with no receiver. Do not give it policy.

- [x] **Step 3: Migrate startup, toggles, compose, and frames**

- `set_graphics(true)` selects `PixelOutput::Graphics`; false selects `HalfBlocks`.
- `set_pixel(on)` calls `set_enabled` and `apply_pixel_outcome`.
- `compose` calls `pixel.paint` when reader is inactive and pixels are enabled.
- Chrome state, cursor decisions, and `ReadDemand.pixel` use `pixel.enabled()`.
- `Event::Frame` passes the source id, source existence, and `FocusedPage` to `accept_frame`, translates its request, and sends its notice through `Session::notice`.
- Remove `shows_pixel`, `frame_size`, and `on_frame` after their last callers move.
- Update private test assertions from `session.pixel` to `session.pixel.enabled()` and size expectations to the emitted effect.

- [x] **Step 4: Run the mode and frame characterization cluster**

Run:

```bash
cargo test -p wwt session::tests::pixel_mode_without_graphics_is_offered_rather_than_refused
cargo test -p wwt session::tests::p_turns_pixel_mode_on_and_asks_for_pictures
cargo test -p wwt session::tests::p_again_turns_it_off_and_stops_them
cargo test -p wwt session::tests::a_frame_without_graphics_composes_to_half_block_cells
cargo test -p wwt session::tests::a_frame_with_graphics_still_composes_to_an_image
cargo test -p wwt session::tests::a_picture_that_cannot_be_decoded_leaves_the_last_one_up_and_is_still_acked
cargo test -p wwt session::tests::a_frame_for_a_tab_that_is_not_focused_is_acked_and_dropped
cargo test -p wwt session::tests::a_frame_is_acked_even_when_pixel_mode_has_already_been_left
cargo test -p wwt session::tests::a_frame_for_a_tab_that_is_gone_is_dropped_without_a_word
cargo test -p wwt session::tests::each_frame_composes_a_new_generation
```

Expected: every test passes.

---

### Task 5: Integrate every page lifecycle transition

**Files:**
- Modify: `crates/wwt/src/session.rs`
- Test: `crates/wwt/src/session.rs`

**Interfaces:**
- Consumes: the same `reconcile`, `restart`, `forget`, and `stop` operations.
- Produces: one screencast policy path for all Session transitions.

- [x] **Step 1: Migrate reader transitions**

After `set_reader` and `leave_reader` finish mutating reader state, call `reconcile_pixels`. Remove their direct `StartScreencast` and `StopScreencast` branches.

Run:

```bash
cargo test -p wwt session::tests::asking_for_reader_keeps_the_picture_until_the_document_arrives
cargo test -p wwt session::tests::cached_reader_entry_stops_pixels_and_exit_starts_them_again
cargo test -p wwt session::tests::p_from_reader_starts_pixels_and_quit_stops_them
cargo test -p wwt session::tests::p_selects_pixels_even_when_reader_was_entered_from_pixel_mode
```

Expected: every test passes.

- [x] **Step 2: Migrate focus, open, adopt, and attachment**

- In `open_tab` and `adopt_tab`, create and focus the new opening tab, then reconcile. The old requested target stops; the unattached target does not start.
- In `focus_tab`, finish `look_at` and `reattach`, then reconcile. An attached destination stops the old target and starts the new one; an opening destination only stops the old one.
- In successful `Job::Opened`, set `Presence::Attached`, then reconcile if the opened tab is focused.
- Delete `follow_focus` and `leave_for_a_new_tab` after all callers move.

Run:

```bash
cargo test -p wwt session::tests::opening_a_tab_in_pixel_mode_moves_the_picture_to_it
cargo test -p wwt session::tests::adopting_a_tab_in_pixel_mode_moves_the_picture_to_it
cargo test -p wwt session::tests::opening_a_tab_in_text_mode_asks_for_no_screencast
cargo test -p wwt session::tests::switching_tabs_moves_the_screencast_with_the_focus
cargo test -p wwt session::tests::switching_tabs_in_text_mode_asks_for_no_screencast
cargo test -p wwt session::tests::the_previous_picture_stays_up_until_the_new_tabs_first_frame
cargo test -p wwt session::tests::pixel_screencast_follows_only_tabs_that_show_pixels
cargo test -p wwt session::tests::detached_reader_starts_no_picture_and_refreshes_when_opened
```

Expected: every test passes.

- [x] **Step 3: Migrate close and browser loss**

- Before removing a closed tab, call `pixel.forget(id)`. If focus moves to an attached successor, reconcile afterward. Do not stop the closed target.
- Before `TabDirective::DetachAll` detaches tabs after browser loss or login handoff, forget the currently requested focused target. Do not emit a stop to the lost browser.
- Preserve the existing browser outcome and effect order.

Run:

```bash
cargo test -p wwt session::tests::closing_the_focused_tab_moves_the_screencast_to_the_next_one
cargo test -p wwt session::tests::a_frame_for_a_tab_that_is_gone_is_dropped_without_a_word
cargo test -p wwt session::tests::a_dead_browser_leaves_every_tab_where_it_was_and_asks_for_another
cargo test -p wwt session::tests::login_preserves_the_session_and_requests_one_browser_handoff
cargo test -p wwt session::tests::the_expected_browser_disconnect_during_login_does_not_race_a_relaunch
```

Expected: every test passes.

- [x] **Step 4: Migrate resize and quit**

- In `on_resize`, update grid, cell, viewport, reader layouts, and `SetViewport` effects first. Then call `pixel.restart` and translate its requests.
- In quit handling, translate `pixel.stop()` before appending `Effect::Quit`.
- Remove the last direct screencast policy from `Session`.

Run:

```bash
cargo test -p wwt session::tests::a_resize_restarts_the_screencast_at_the_new_size
cargo test -p wwt session::tests::a_resize_moves_the_picture_to_the_area_it_now_covers
cargo test -p wwt session::tests::a_resize_to_the_same_size_costs_nothing
cargo test -p wwt session::tests::quitting_from_pixel_mode_stops_the_screencast_first
```

Expected: every test passes.

- [x] **Step 5: Run the complete crate test suite**

Run:

```bash
cargo test -p wwt --lib
```

Expected: all `wwt` library tests pass.

---

### Task 6: Encode the ownership rule and document the module

**Files:**
- Create: `scripts/check-pixel-lifecycle.sh`
- Modify: `CONTEXT.md`

**Interfaces:**
- Produces: an executable architectural regression check and a concise ownership note.

- [x] **Step 1: Add the failing ownership check before removing all old coordination**

Create this executable script:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
session="$repo_root/crates/wwt/src/session.rs"
production="$(mktemp)"
trap 'rm -f "$production"' EXIT

sed '/^#\[cfg(test)\]/,$d' "$session" > "$production"

for forbidden in \
    '^    graphics: bool,$' \
    '^    pixel: bool,$' \
    '^    picture: Option<Picture>,$' \
    '^    generations: u64,$' \
    '^    fn shows_pixel' \
    '^    fn follow_focus' \
    '^    fn leave_for_a_new_tab' \
    '^    fn frame_size' \
    '^    fn on_frame'
do
    if rg -n "$forbidden" "$production"; then
        echo "pixel lifecycle leaked into production session.rs: $forbidden" >&2
        exit 1
    fi
done

cargo test --quiet --manifest-path "$repo_root/Cargo.toml" -p wwt pixel::tests
cargo test --quiet --manifest-path "$repo_root/Cargo.toml" -p wwt session::tests::switching_tabs_moves_the_screencast_with_the_focus
cargo test --quiet --manifest-path "$repo_root/Cargo.toml" -p wwt session::tests::every_frame_is_acked_so_the_next_one_comes
cargo test --quiet --manifest-path "$repo_root/Cargo.toml" -p wwt session::tests::cached_reader_entry_stops_pixels_and_exit_starts_them_again
cargo test --quiet --manifest-path "$repo_root/Cargo.toml" -p wwt session::tests::a_resize_restarts_the_screencast_at_the_new_size
cargo test --quiet --manifest-path "$repo_root/Cargo.toml" -p wwt session::tests::closing_the_focused_tab_moves_the_screencast_to_the_next_one
```

Make it executable and run:

```bash
chmod +x scripts/check-pixel-lifecycle.sh
scripts/check-pixel-lifecycle.sh
```

Expected before final cleanup: failure naming any old field or helper still present. Remove the remaining old ownership and rerun until it passes.

- [x] **Step 2: Record the boundary in CONTEXT.md**

Add a module-map entry equivalent to:

```markdown
- **Pixel presentation** (`crates/wwt/src/pixel.rs`) owns the global pixel preference, requested screencast, output form, retained picture, frame acceptance, and graphics generations. `Session` supplies only a focused-page projection and translates presentation requests into existing effects.
```

Use the vocabulary already established by nearby lifecycle entries.

- [x] **Step 3: Run the ownership check green**

Run:

```bash
scripts/check-pixel-lifecycle.sh
```

Expected: no forbidden production Session ownership and all focused tests pass.

---

### Task 7: Verify, inspect, and commit phase 4

**Files:**
- Verify: all changed files.

- [x] **Step 1: Format and verify the real workspace**

Run:

```bash
cargo fmt --all
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts/check-pixel-lifecycle.sh
git diff --check
```

Expected: every command succeeds. The workspace test must include the Chromium-backed tests rather than substituting a library-only proxy.

Execution note, 2026-08-29: tests, clippy, the ownership check, and whitespace
validation succeeded. `cargo fmt --all -- --check` was run, but rustfmt 1.9 also
rejects untouched baseline files such as `core.rs`, `effect.rs`, and `keymap.rs`.
The new `pixel.rs` passes a direct rustfmt check. The repository-wide rewrite was
left out of this refactor to preserve scope.

- [x] **Step 2: Inspect scope and ownership**

Run:

```bash
git status --short
git diff --stat
git diff -- crates/wwt/src/pixel.rs crates/wwt/src/session.rs crates/wwt/src/lib.rs CONTEXT.md scripts/check-pixel-lifecycle.sh
rg -n 'StartScreencast|StopScreencast|AckFrame' crates/wwt/src/session.rs crates/wwt/src/pixel.rs
```

Expected: direct screencast effects in `session.rs` exist only in the request-to-effect adapter. Pixel policy and picture state live in `pixel.rs`. No public vocabulary or unrelated file changed.

- [x] **Step 3: Commit the completed phase**

Run:

```bash
git add CONTEXT.md crates/wwt/src/lib.rs crates/wwt/src/pixel.rs crates/wwt/src/session.rs docs/superpowers/plans/2026-08-29-wwt-pixel-lifecycle.md scripts/check-pixel-lifecycle.sh
git commit -m "refactor(wwt): centralize pixel presentation"
```

Expected: one implementation commit after the approved design commit.

- [x] **Step 4: Stop for integration direction**

Report the commits and fresh verification results. Do not push, merge, or remove the worktree until the user chooses an integration path.
