# WWT pixel presentation lifecycle

**Date:** 2026-08-29
**Status:** Approved design
**Parent specs:** `2026-08-28-wwt-lifecycle-modules-design.md`,
`2026-08-22-wwt-m5-design.md`, `2026-08-23-wwt-m6-design.md`, and
`2026-08-24-wwt-m8-design.md` govern behavior.

This document completes phase 4 of the lifecycle-module refactor. It changes code
ownership and does not change WWT behavior. The parent specs win if this document
implies a different user-visible result.

## 1. Why pixel presentation becomes a module

Pixel presentation is one lifecycle with state and ordering rules spread through
`Session`:

- The global pixel preference selects live-page presentation unless reader view is
  active.
- Exactly one focused, attached, live page may receive screencast frames.
- Focus, attachment, reader entry, reader exit, resize, target loss, and quit change
  which screencast WWT requests.
- A picture survives a tab switch and reader entry until another picture replaces it
  or pixel mode ends.
- Every frame from an existing tab is acknowledged before WWT decides whether to use
  it.
- The terminal output form decides whether WWT keeps the base64 PNG or decodes it into
  half-block samples.
- Graphics images need a new generation and placement area after each accepted frame
  and resize.

`Session` currently coordinates these rules through `graphics`, `pixel`, `picture`,
and `generations`, plus methods for focus following, new-tab departure, frame sizing,
frame acceptance, and painting. The rules repeat the same question in several forms:
which focused page, if any, should screencast now?

The private `pixel` module owns that question. `Session` still owns tabs, focus,
reader state, input modes, and effect ordering outside pixel presentation.

## 2. Invariants

The refactor preserves these rules:

- Pixel preference is global and is not saved.
- Reader view suppresses pixel presentation without changing pixel preference.
- Only the focused, attached page outside reader view receives a requested
  screencast.
- Text mode requests no screencast.
- Opening or reattaching a focused tab stops the previous screencast. The new
  screencast starts only after the target becomes attached.
- Closing a target or losing Chromium forgets its screencast without asking the gone
  target to stop.
- A resize restarts the requested screencast at the new size.
- A tab switch keeps the previous picture until the new tab produces a frame.
- Reader entry keeps the previous picture while the reader request is in flight and
  after reader view becomes active. Reader exit may show that picture until a new
  frame arrives.
- Leaving pixel mode clears the picture, marks every tab's live runs dirty, and asks
  the focused live page for runs again.
- Quit requests a screencast stop before `Effect::Quit`.
- Every frame from an existing tab is acknowledged. A frame from a closed tab emits
  nothing.
- A stale or hidden frame is acknowledged and discarded.
- A half-block decode failure keeps the previous picture and reports
  `that picture could not be read`.
- Graphics frames keep their base64 payload in an `Arc<String>` and are not decoded.
- `Frame` remains the only output type for text, reader, graphics, and half-block
  presentation.

## 3. Module map

The module is private to the `wwt` crate:

```text
Session -> pixel
pixel   -> Effect vocabulary, TabId, wwt-frame, wwt-page, and wwt-png

Session owns tabs, focus, reader state, and input modes.
pixel owns pixel preference, screencast reconciliation, picture data,
terminal output form, and graphics generations.
Core continues to perform screencast effects and pace acknowledgements.
```

The module never receives a `Tab`, a tab slice, `Mode`, `Core`, `Page`, renderer, or
terminal stream.

## 4. State model

`PixelPresentation` owns four facts:

```rust
pub(crate) struct PixelPresentation {
    output: PixelOutput,
    mode: PixelMode,
    requested: Option<RequestedScreencast>,
    generation: u64,
}

enum PixelOutput {
    Graphics,
    HalfBlocks,
}

enum PixelMode {
    Text,
    Pixel { picture: Option<Picture> },
}

struct RequestedScreencast {
    tab: TabId,
    size: FrameSize,
}

enum Picture {
    Graphics(Image),
    Blocks(Samples),
}
```

`PixelMode` prevents text mode from retaining a picture. Pixel mode may have no
picture because a first frame has not arrived. Pixel mode may keep a picture while
reader view is active because reader view suppresses presentation rather than changing
the global preference.

`requested` records the screencast that policy asked `Core` to run. It is not a claim
that Chromium successfully started the screencast. The current system has no start
confirmation and this refactor does not add one.

`PixelOutput` starts as `HalfBlocks`. `Session::set_graphics` selects `Graphics` when
the startup probe reports support. The output form does not change during a run.

## 5. Focus projection

`Session` gives the module only the state needed to derive the desired screencast:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FocusedPage {
    pub(crate) id: TabId,
    pub(crate) attached: bool,
    pub(crate) reader_active: bool,
}
```

The desired screencast is `Some(id, size)` only when all three conditions hold:

- Pixel mode is enabled.
- The focused page is attached.
- Reader view is inactive.

Every other state has no desired screencast.

## 6. Interface

The caller uses this interface:

```rust
pixel.set_output(PixelOutput);
pixel.set_enabled(on) -> PixelOutcome;
pixel.reconcile(FocusedPage, Viewport) -> Vec<PresentationRequest>;
pixel.restart(FocusedPage, Viewport) -> Vec<PresentationRequest>;
pixel.forget(TabId);
pixel.stop() -> Option<PresentationRequest>;
pixel.accept_frame(
    source: TabId,
    source_exists: bool,
    focused: FocusedPage,
    frame: ScreencastFrame,
    viewport: Viewport,
) -> FrameOutcome;
pixel.paint(&mut Frame, Viewport);
pixel.enabled() -> bool;
```

The request vocabulary is private:

```rust
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

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FrameOutcome {
    pub(crate) request: Option<PresentationRequest>,
    pub(crate) notice: Option<String>,
}
```

`Session` translates `PresentationRequest` to the existing `Effect` variants. The
module does not add or change a public event, effect, job, snapshot field, setting, or
key binding.

## 7. Reconciliation

`reconcile` derives the desired screencast from `PixelMode`, `FocusedPage`, and the
viewport. It compares that value with `requested`:

- Equal values emit nothing.
- `Some(old)` to `None` emits `Stop(old.tab)` and clears `requested`.
- `None` to `Some(new)` emits `Start(new.tab, new.size)` and stores `new`.
- `Some(old)` to a different `Some(new)` emits `Stop(old.tab)` followed by
  `Start(new.tab, new.size)`, then stores `new`.

This one operation replaces `shows_pixel`, `follow_focus`, and
`leave_for_a_new_tab`.

`restart` applies only after a real viewport change. If a screencast is desired, it
emits `Stop` followed by `Start` at the new size and updates `requested`. If no
screencast is desired, it delegates to ordinary reconciliation.

`forget(id)` clears `requested` only when it names `id`. It emits no request because
the target is already gone or is about to be closed. Browser loss, login handoff, and
focused-tab close use this path.

`stop` emits `Stop` for the requested screencast and clears `requested`. Quit uses this
path before it emits `Effect::Quit`.

## 8. Enabling and disabling

`set_enabled(true)` changes `PixelMode::Text` to `Pixel { picture: None }` and returns
`changed: true`. A repeated enable returns the default `PixelOutcome`.

`set_enabled(false)` changes pixel mode to text mode, drops the picture, clears no
screencast by itself, and returns `changed: true` and `refresh_live_runs: true`.
`Session` then clears hints, calls `reconcile`, marks every tab's live view dirty, and
requests the focused read. A repeated disable returns the default outcome, clears no
hints, and requests no read.

Hint clearing and reader cancellation remain in `Session` because they belong to input
mode and reader policy.

## 9. Frames and painting

`accept_frame` receives the source tab separately from the focused-page projection.
It first checks `source_exists`:

- A frame from a closed tab returns the default `FrameOutcome`.
- Every other frame returns `PresentationRequest::Ack(source, frame.ack)` before any
  visibility or decode decision.

The module accepts the picture only when pixel mode is enabled, reader view is
inactive, and `source` names the focused tab.

For `PixelOutput::Graphics`, the module increments `generation` and stores an `Image`
whose payload is the frame's base64 string and whose area is the page viewport.

For `PixelOutput::HalfBlocks`, the module decodes the PNG and resamples it to
`grid.cols` by `grid.rows * 2`. A successful decode replaces the picture. A failed
decode preserves the previous picture and returns the existing notice text.

`paint` does nothing in text mode or when pixel mode has no picture. It attaches a
graphics image or paints half-block samples into the page viewport. `Session::compose`
keeps the reader-first choice, paints hints after the page representation, paints
chrome last, and remains the only place that selects the cursor.

On resize, the module updates a retained graphics image's area and generation before
`Session` composes the next frame. Half-block samples need no placement update.

## 10. Session integration

`Session` owns one field:

```rust
pixel: PixelPresentation,
```

The integration follows these rules:

- `set_graphics` delegates to `pixel.set_output`.
- Pixel toggle and `:set pixel` delegate to `set_enabled`, reconcile the focused page,
  and apply `PixelOutcome`.
- Focus changes call `reconcile` after focus, attachment, and reader state reach their
  new values.
- New-tab creation and reattachment set focused presence to `Opening`, then call
  `reconcile`. The old screencast stops and no new one starts.
- `Job::Opened` sets presence to `Attached`, then calls `reconcile`.
- Reader activation and exit call `reconcile` after reader state changes.
- Resize updates the viewport, asks the module to update picture placement, and calls
  `restart`.
- Browser loss, login handoff, and focused-tab close call `forget` for the gone target.
- Quit calls `stop` before `Effect::Quit`.
- `Event::Frame` delegates to `accept_frame` and applies the returned request and
  notice.
- `compose`, read demand, statusline tags, and cursor selection query `enabled`.

The order of non-pixel effects stays unchanged. In particular, a focused-tab close
still emits `CloseTab` before it starts a screencast for the new focused tab.

## 11. Tests

Direct `pixel` tests cover the rules hidden by the module:

- Text mode reconciles to no screencast.
- Enabling pixels starts only an attached live focused page.
- Reader view and an opening or detached page suppress the start.
- Reconciliation moves a screencast with focus and preserves the picture.
- A target that disappears is forgotten without a stop request.
- Resize restarts the desired screencast with the new size.
- Disable clears the picture and requests live-run refresh once.
- Quit stops the requested screencast.
- Every frame from an existing tab is acknowledged.
- Stale, hidden, and post-disable frames are acknowledged and discarded.
- Closed-tab frames emit nothing.
- Graphics frames keep base64 and advance generations.
- Half-block frames decode once, paint samples, and preserve the old picture on decode
  failure.
- Graphics resize changes image area and generation.

Existing `Session` tests remain at the external seam. They continue to cover input,
reader interaction, focus, opening, closing, browser loss, resize, effect ordering,
chrome tags, cursor behavior, and composition.

The implementation adds `scripts/check-pixel-lifecycle.sh`. The script examines only
production `session.rs`, stops at `#[cfg(test)]`, and fails if the removed fields or
helpers remain. It also runs the direct pixel tests and the focused pixel-related
`Session` tests. The script is deterministic and safe to rerun.

## 12. Delivery

Implementation follows four ordered tasks:

1. Add direct failing tests and the private state model.
2. Implement reconciliation, frame acceptance, and painting.
3. Replace `Session` coordination and preserve all external characterizations.
4. Add the structural verification script, update `CONTEXT.md`, and run the full gate.

Before the implementation commit, run:

```text
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts/check-pixel-lifecycle.sh
```

Commit phase 4 separately as `refactor(wwt): centralize pixel presentation`. Do not
push without separate instruction.

## 13. Rejected designs

### One event variant for every call site

A `PresentationChange` reducer would model pixel toggle, focus, reader, resize, target
open, target loss, and quit as separate variants. `Session` would still need to select
the correct lifecycle transition at every call site. The event list would repeat the
current method list rather than hide it.

Reconciliation asks one stable question instead: which screencast should be requested
for the focused page now?

### Extract only decoding and painting

A picture module could own `Picture`, PNG decoding, and `paint` while `Session` kept
screencast start and stop rules. That split would shorten `session.rs` but leave the
lifecycle spread across focus, reader, resize, frame, and quit paths. Deleting the
module would recover little complexity.

### Move screencast state into each tab

Only one focused tab screencasts. Per-tab state would represent combinations the
product rejects, require coordination across tabs on every focus change, and invite a
saved per-tab preference. Pixel preference and picture data remain global.

## 14. Non-goals

This work adds no feature, dependency, configuration key, snapshot field, browser
flag, codec, protocol, retry, frame buffer, per-tab preference, or site-specific
behavior. It does not change frame pacing, acknowledgement timing, image IDs, reader
behavior, or the public `Session` interface.
