# wwt M3 — Interaction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn M2's read-only browser into one you can use: keys reach the page, every link and button is reachable without a mouse, forms accept text, and a click lands where you pointed.

**Architecture:** The loop is unchanged. `Core` still owns all state and still spawns page operations rather than awaiting them. M3 adds a wider `Mode` enum, a `wwt-ui` crate holding everything that decides what input means and what the screen shows, and one long-lived input pump task so that keystrokes reach Chromium in the order you typed them.

**Tech Stack:** Rust 2024, tokio, tokio-tungstenite, crossterm (with `event-stream`), futures-util, serde/serde_json, anyhow. Chromium as an external process.

**Spec:** `docs/superpowers/specs/2026-08-19-wwt-m3-design.md` — read it in full before starting. Its parent, `docs/superpowers/specs/2026-08-19-wwt-design.md`, governs where the two disagree; sections 4, 6 and 8 of the parent are the relevant ones, and section 8 of this plan's spec lists the three places M3 amends them.

## Global Constraints

- Rust edition **2024**, toolchain **1.97+**.
- Dependency versions, exact, unchanged from M2: `tokio = "1.53"`, `tokio-tungstenite = "0.30"`, `futures-util = "0.3"`, `serde = "1.0"` (feature `derive`), `serde_json = "1.0"`, `crossterm = "0.29"` (feature `event-stream`), `rustix = "1.1"` (feature `termios`), `anyhow = "1.0"`, `thiserror = "2.0"`, `tempfile = "3"` (dev-dependency).
- **M3 adds no dependencies at all.** The new `wwt-ui` crate uses `wwt-frame` and nothing else. If a task tempts you to add a crate, stop and ask.
- `wwt-frame` has **no I/O and no dependencies**. Unchanged, non-negotiable.
- `wwt-ui` depends on `wwt-frame` only. It must never learn about pages, CDP, or the terminal: that is what keeps every mode transition and every painted overlay testable without a browser.
- Chromium is located via `WWT_CHROMIUM`, falling back to the first of `chromium`, `chromium-browser`, `google-chrome-stable` on `PATH`. Never download anything.
- `cargo clippy --workspace --all-targets -- -D warnings` must be clean at the end of **every** task, not only at the end of the plan.
- Tests that need a browser live in `tests/`, never in `src/`. Unit tests in `src/` must run without Chromium.
- Follow the existing comment style: explain *why*, in prose, where the reason is not obvious from the code. Do not add comments that restate the code.
- Commits are conventional with a crate scope: `feat(ui):`, `feat(page):`, `refactor(wwt):`.

## Baseline

Before Task 1, confirm the starting state:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 102 tests pass (22 `wwt-frame`, 18 `wwt-term`, 8 `wwt-cdp`, 22 `wwt-page`, 32 `wwt`), clippy clean.

If tests fail with paths naming a directory that no longer exists, the target directory holds stale artifacts from before the rename: `touch crates/*/tests/*.rs` and run again.

## File structure

| File | Responsibility |
|---|---|
| `crates/wwt-frame/src/target.rs` | **New.** `HintTarget`, `TargetKind`, the click point and the label cell. Geometry, like `TextRun` beside it. |
| `crates/wwt-ui/src/lib.rs` | **New crate.** Module list and re-exports. |
| `crates/wwt-ui/src/mode.rs` | **New.** The four-mode enum. Nothing else: the modes' behaviour lives with the things they operate. |
| `crates/wwt-ui/src/hint.rs` | **New.** Label alphabet, label assignment, prefix filtering, overlay painting. |
| `crates/wwt-ui/src/chrome.rs` | Moved from `wwt`. Statusline, command line, the mode indicator. |
| `crates/wwt-ui/src/command.rs` | Moved from `wwt`. `:` command parsing, plus `:set mouse`. |
| `crates/wwt-page/src/input.rs` | **New.** `KeyInput`, `MouseInput`: the wire shapes `Input.dispatch*Event` needs. |
| `crates/wwt-page/src/extract.rs` | Gains `eval`, `dispatch_key`, `dispatch_mouse`, `blur`, `hints`. |
| `crates/wwt-page/assets/bootstrap.js` | Gains `hints()` beside `extract()`. |
| `crates/wwt/src/keys.rs` | **New.** crossterm `KeyEvent` to `KeyInput`. Pure, US layout, unit-tested without a browser. |
| `crates/wwt/src/input.rs` | **New.** The input pump: one task, one channel, ordered delivery. |
| `crates/wwt/src/core.rs` | Mode routing, the hint cache, mouse dispatch, `Job::InputFailed` and `Job::Hints`. |
| `crates/wwt/src/keymap.rs` | Gains `Action::Insert` and `Action::Hints`. |
| `crates/wwt/src/main.rs` | Mouse capture beside the alternate screen. |

---

### Task 1: `HintTarget` in `wwt-frame`

Hint targets are geometry the page produces and the painter consumes, which is exactly what `TextRun` is, so they live in the same crate for the same reason. Putting them in `wwt-page` would force `wwt-ui` to depend on `wwt-page`.

**Files:**
- Create: `crates/wwt-frame/src/target.rs`
- Modify: `crates/wwt-frame/src/lib.rs`

**Interfaces:**
- Consumes: `CellPos`, `CssPoint`, `CssRect`, `Viewport` from `wwt_frame::geom`.
- Produces: `wwt_frame::{HintTarget, TargetKind}`; `HintTarget { rect: CssRect, kind: TargetKind }`; `TargetKind::{Clickable, Editable}`; `HintTarget::center(&self) -> CssPoint`; `HintTarget::label_cell(&self, vp: &Viewport) -> CellPos`.

- [ ] **Step 1: Write the failing tests**

Create `crates/wwt-frame/src/target.rs` containing only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{CellSize, GridSize};

    fn vp() -> Viewport {
        Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 })
    }

    fn target(x: f64, y: f64) -> HintTarget {
        HintTarget {
            rect: CssRect { x, y, w: 40.0, h: 20.0 },
            kind: TargetKind::Clickable,
        }
    }

    #[test]
    fn the_click_point_is_the_middle_of_the_box() {
        let t = target(100.0, 200.0);
        assert_eq!(t.center(), CssPoint { x: 120.0, y: 210.0 });
    }

    #[test]
    fn the_label_goes_at_the_top_left_cell_of_the_box() {
        // x 90 is column 10 at 9px cells; y 200 is row 10 at 20px cells.
        let t = target(90.0, 200.0);
        assert_eq!(t.label_cell(&vp()), CellPos { col: 10, row: 10 });
    }

    #[test]
    fn a_box_starting_off_the_left_or_top_still_gets_a_reachable_label() {
        let t = target(-500.0, -500.0);
        assert_eq!(t.label_cell(&vp()), CellPos { col: 0, row: 0 });
    }

    #[test]
    fn a_box_starting_past_the_grid_is_clamped_to_its_last_cell() {
        let t = target(100_000.0, 100_000.0);
        assert_eq!(t.label_cell(&vp()), CellPos { col: 79, row: 23 });
    }
}
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p wwt-frame`
Expected: compilation fails, `cannot find type HintTarget in this scope`.

- [ ] **Step 3: Write the implementation**

Put this above the test module in `crates/wwt-frame/src/target.rs`:

```rust
use crate::geom::{CellPos, CssPoint, CssRect, Viewport};

/// What activating a target leaves you in the middle of.
///
/// The distinction is the whole reason the page reports a kind: clicking a
/// link is finished when the click lands, and clicking a text field is the
/// start of typing into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Clickable,
    Editable,
}

/// One interactive element, as the page measured it.
#[derive(Debug, Clone, PartialEq)]
pub struct HintTarget {
    /// The element's client rect, in CSS pixels.
    pub rect: CssRect,
    pub kind: TargetKind,
}

impl HintTarget {
    /// The point a click on this target should land on.
    ///
    /// The centre, because that is the point the page's own occlusion check
    /// tested: clicking anywhere else could land on something that was
    /// covering an edge.
    pub fn center(&self) -> CssPoint {
        CssPoint {
            x: self.rect.x + self.rect.w / 2.0,
            y: self.rect.y + self.rect.h / 2.0,
        }
    }

    /// The cell this target's label is painted at.
    ///
    /// Clamped into the grid rather than dropped, so a box that starts above
    /// or left of the viewport but is still visible keeps a label you can
    /// reach.
    pub fn label_cell(&self, vp: &Viewport) -> CellPos {
        let grid = vp.grid();
        let last_col = i64::from(grid.cols.saturating_sub(1));
        let last_row = i64::from(grid.rows.saturating_sub(1));
        CellPos {
            col: vp.col_of(self.rect.x).clamp(0, last_col) as u16,
            row: vp.row_of(self.rect.y).clamp(0, last_row) as u16,
        }
    }
}
```

- [ ] **Step 4: Export it**

In `crates/wwt-frame/src/lib.rs`, add the module and the re-export beside the existing ones:

```rust
pub mod target;

pub use target::{HintTarget, TargetKind};
```

Keep the file's existing ordering: modules first, then `pub use` lines.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p wwt-frame`
Expected: PASS, 26 tests.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/wwt-frame
git commit -m "feat(frame): add HintTarget beside TextRun"
```

---

### Task 2: The `wwt-ui` crate

A pure move. No behaviour changes, no new tests, and the same 102 tests pass at the end. Doing it alone means the next four tasks are reviewable as behaviour rather than as a diff full of relocated lines.

**Files:**
- Create: `crates/wwt-ui/Cargo.toml`, `crates/wwt-ui/src/lib.rs`
- Move: `crates/wwt/src/chrome.rs` to `crates/wwt-ui/src/chrome.rs`
- Move: `crates/wwt/src/command.rs` to `crates/wwt-ui/src/command.rs`
- Modify: `Cargo.toml` (workspace members), `crates/wwt/Cargo.toml`, `crates/wwt/src/lib.rs`, `crates/wwt/src/core.rs`, `crates/wwt/src/main.rs`, `crates/wwt/tests/smoke.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `wwt_ui::chrome::{State, Mode, statusline, command_line, paint}` and `wwt_ui::command::{Command, parse, normalize_url}`, at the same paths they had under `wwt`.

- [ ] **Step 1: Create the crate**

`crates/wwt-ui/Cargo.toml`:

```toml
[package]
name = "wwt-ui"
edition.workspace = true
version.workspace = true

[dependencies]
wwt-frame = { path = "../wwt-frame" }
```

`crates/wwt-ui/src/lib.rs`:

```rust
//! Chrome and modes: what input means, and what the screen says about it.
//!
//! This crate knows about a `Frame` and nothing else. It cannot reach a
//! page, a socket, or the terminal, which is what keeps every mode
//! transition and every painted overlay testable with no browser in the
//! loop.

pub mod chrome;
pub mod command;
```

Add `"crates/wwt-ui"` to the `members` list in the workspace `Cargo.toml`, keeping the existing dependency order (`wwt-frame`, `wwt-term`, `wwt-cdp`, `wwt-page`, `wwt-ui`, `wwt`).

- [ ] **Step 2: Move the files**

```bash
git mv crates/wwt/src/chrome.rs crates/wwt-ui/src/chrome.rs
git mv crates/wwt/src/command.rs crates/wwt-ui/src/command.rs
```

- [ ] **Step 3: Rewire the binary**

In `crates/wwt/Cargo.toml`, add to `[dependencies]`, keeping the list alphabetical:

```toml
wwt-ui = { path = "../wwt-ui" }
```

In `crates/wwt/src/lib.rs`, delete the `pub mod chrome;` and `pub mod command;` lines.

In `crates/wwt/src/core.rs`, replace the two `use crate::chrome...` / `use crate::command...` lines with:

```rust
use wwt_ui::chrome::{self, Mode, State};
use wwt_ui::command::{self, Command};
```

In `crates/wwt/src/main.rs`, replace `use wwt::command::normalize_url;` with:

```rust
use wwt_ui::command::normalize_url;
```

In `crates/wwt/tests/smoke.rs`, replace `use wwt::chrome::Mode;` with `use wwt_ui::chrome::Mode;`, and inside `the_command_line_opens_fills_and_closes` replace `wwt::chrome::` with `wwt_ui::chrome::` and `wwt::command::` with `wwt_ui::command::` throughout.

- [ ] **Step 4: Run everything**

Run: `cargo test --workspace`
Expected: PASS, 106 tests, the same set as after Task 1 with 18 of them now
living elsewhere: 26 `wwt-frame`, 18 `wwt-term`, 8 `wwt-cdp`, 22 `wwt-page`,
18 `wwt-ui`, 14 `wwt`. A move that changes the total is a move that lost a
test.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(ui): move chrome and commands into wwt-ui

The crate M2 deferred. Its second consumer arrives with hint overlays
and the mode machine, and drawing the boundary now keeps that work out
of the binary."
```

---

### Task 3: Insert mode exists

The mode enum moves to its own module and grows a third variant, and the statusline learns to say which mode you are in. Nothing enters insert mode yet: Task 10 wires the key.

**Files:**
- Create: `crates/wwt-ui/src/mode.rs`
- Modify: `crates/wwt-ui/src/lib.rs`, `crates/wwt-ui/src/chrome.rs`
- Modify: `crates/wwt/src/core.rs` (the `on_key` match must stay exhaustive)

**Interfaces:**
- Consumes: nothing new.
- Produces: `wwt_ui::mode::Mode` with variants `Normal`, `Command(String)`, `Insert`, re-exported as `wwt_ui::Mode`; `chrome::statusline(mode: &Mode, state: &State, url: &str, title: &str, progress: f64, cols: u16) -> String`. `chrome::paint`'s signature is unchanged.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `crates/wwt-ui/src/chrome.rs`:

```rust
    #[test]
    fn the_statusline_says_when_you_are_in_insert_mode() {
        let line = statusline(&Mode::Insert, &State::Ready, "https://example.com", "", 0.0, 60);
        assert!(line.starts_with("-- INSERT --"), "line was {line:?}");
        assert!(line.contains("https://example.com"), "line was {line:?}");
    }

    #[test]
    fn normal_mode_adds_nothing_to_the_statusline() {
        let line = statusline(&Mode::Normal, &State::Ready, "https://example.com", "", 0.0, 60);
        assert!(line.starts_with("https://example.com"), "line was {line:?}");
    }
```

Update the five existing `statusline(...)` calls in that module to pass `&Mode::Normal` as the first argument.

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p wwt-ui`
Expected: compilation fails, `this function takes 5 arguments but 6 arguments were supplied`.

- [ ] **Step 3: Move `Mode` into its own module**

Create `crates/wwt-ui/src/mode.rs`:

```rust
//! What keys mean right now.

/// The mode the browser is in.
///
/// Mode changes only in response to a keystroke. A page cannot move you
/// between modes, which is the property that makes handing it the keyboard
/// safe: `Esc` always comes back.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    /// The `:` line is open, holding what has been typed so far.
    Command(String),
    /// Every key goes to the page. Entered with `i` or by hinting a text
    /// field, left with `Esc`, which is never forwarded.
    Insert,
}
```

Delete the `Mode` enum from `crates/wwt-ui/src/chrome.rs` and add `use crate::mode::Mode;` at the top of it instead.

In `crates/wwt-ui/src/lib.rs`, add the module and the re-export:

```rust
pub mod mode;

pub use mode::Mode;
```

`Mode` now has exactly one home. Update its two consumers to import it from
there: in `crates/wwt/src/core.rs` change `use wwt_ui::chrome::{self, Mode, State};`
to

```rust
use wwt_ui::Mode;
use wwt_ui::chrome::{self, State};
```

and in `crates/wwt/tests/smoke.rs` change `use wwt_ui::chrome::Mode;` to
`use wwt_ui::Mode;`.

- [ ] **Step 4: Teach the statusline about modes**

In `crates/wwt-ui/src/chrome.rs`, add above `statusline`:

```rust
/// What the statusline says about the mode, if anything.
///
/// Normal mode says nothing: the absence of a tag is what normal looks
/// like, and a browser that shouts its default state at you all day is
/// noise.
fn mode_tag(mode: &Mode) -> String {
    match mode {
        Mode::Normal | Mode::Command(_) => String::new(),
        Mode::Insert => "-- INSERT -- ".to_string(),
    }
}
```

Change `statusline`'s signature and its `left` binding:

```rust
pub fn statusline(
    mode: &Mode,
    state: &State,
    url: &str,
    title: &str,
    progress: f64,
    cols: u16,
) -> String {
    let tag = match state {
        State::Ready => String::new(),
        State::Loading => "[loading] ".to_string(),
        State::Stalled => "[stalled] ".to_string(),
        State::Error(message) => format!("[error] {message} — "),
    };

    let left = if title.is_empty() {
        format!("{}{tag}{url}", mode_tag(mode))
    } else {
        format!("{}{tag}{url} — {title}", mode_tag(mode))
    };
```

The rest of the function is unchanged. In `paint`, pass the mode through:

```rust
    let text = match mode {
        Mode::Command(buffer) => command_line(buffer, cols),
        _ => statusline(mode, state, url, title, progress, cols),
    };
```

- [ ] **Step 5: Keep `Core` exhaustive**

`Core::on_key` matches on `&self.mode`. Add an arm so it compiles, doing nothing until Task 10:

```rust
            // Wired in Task 10. Until then a mode nothing can enter.
            Mode::Insert => false,
```

- [ ] **Step 6: Run the tests**

Run: `cargo test --workspace`
Expected: PASS, 108 tests (20 `wwt-ui`).

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(ui): give the mode its own module and a statusline tag"
```

---

### Task 4: Hint labels

All of hint mode's thinking, with no browser anywhere near it: assign labels, filter them as you type, paint what still matches.

**Files:**
- Create: `crates/wwt-ui/src/hint.rs`
- Modify: `crates/wwt-ui/src/lib.rs`, `crates/wwt-ui/src/mode.rs`, `crates/wwt-ui/src/chrome.rs`
- Modify: `crates/wwt/src/core.rs` (exhaustive match again)

**Interfaces:**
- Consumes: `wwt_frame::{Frame, HintTarget, TargetKind, Viewport}` from Task 1.
- Produces: `wwt_ui::hint::{ALPHABET, labels, HintSession, Filtered}`; `labels(count: usize) -> Vec<String>`; `HintSession::new(targets: Vec<HintTarget>) -> HintSession`; `HintSession::{is_empty, typed, push, pop, paint, tag}`; `Filtered::{Waiting(usize), Activate(HintTarget), None}`; `Mode::Hint(HintSession)`.

- [ ] **Step 1: Write the failing tests**

Create `crates/wwt-ui/src/hint.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wwt_frame::{CellSize, CssRect, GridSize};

    fn vp() -> Viewport {
        Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 })
    }

    /// `count` targets stacked one cell apart, so each label lands on its
    /// own row and the painted result is readable.
    fn targets(count: usize) -> Vec<HintTarget> {
        (0..count)
            .map(|i| HintTarget {
                rect: CssRect { x: 0.0, y: (i as f64) * 20.0, w: 40.0, h: 20.0 },
                kind: TargetKind::Clickable,
            })
            .collect()
    }

    #[test]
    fn labels_are_one_character_while_the_alphabet_covers_the_targets() {
        let labels = labels(ALPHABET.len());
        assert_eq!(labels.len(), ALPHABET.len());
        assert!(labels.iter().all(|l| l.chars().count() == 1), "{labels:?}");
    }

    #[test]
    fn labels_grow_to_two_characters_one_past_the_alphabet() {
        let labels = labels(ALPHABET.len() + 1);
        assert!(labels.iter().all(|l| l.chars().count() == 2), "{labels:?}");
    }

    #[test]
    fn every_label_is_distinct() {
        let labels = labels(200);
        let mut sorted = labels.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len());
    }

    #[test]
    fn no_label_is_a_prefix_of_another() {
        // This is what uniform length buys, and it is why activation needs
        // no timeout: a full match cannot also be a partial one.
        let labels = labels(200);
        for a in &labels {
            for b in &labels {
                if a != b {
                    assert!(!b.starts_with(a.as_str()), "{a} is a prefix of {b}");
                }
            }
        }
    }

    #[test]
    fn typing_narrows_the_matching_set() {
        let mut session = HintSession::new(targets(100));
        let first = ALPHABET[0] as char;
        match session.push(first) {
            Filtered::Waiting(n) => assert!(n > 0 && n < 100, "narrowed to {n} of 100"),
            other => panic!("expected to still be narrowing, got {other:?}"),
        }
        assert_eq!(session.typed(), first.to_string());
    }

    #[test]
    fn a_unique_prefix_activates_its_target() {
        let mut session = HintSession::new(targets(3));
        // Three targets get one-character labels, so the first character
        // identifies one.
        match session.push(ALPHABET[1] as char) {
            Filtered::Activate(target) => assert_eq!(target.rect.y, 20.0),
            other => panic!("expected an activation, got {other:?}"),
        }
    }

    #[test]
    fn a_prefix_that_matches_nothing_says_so() {
        let mut session = HintSession::new(targets(3));
        assert!(matches!(session.push('z'), Filtered::None));
    }

    #[test]
    fn backspace_widens_the_set_again() {
        let mut session = HintSession::new(targets(100));
        session.push(ALPHABET[0] as char);
        match session.pop() {
            Filtered::Waiting(n) => assert_eq!(n, 100),
            other => panic!("expected the whole set back, got {other:?}"),
        }
        assert_eq!(session.typed(), "");
    }

    #[test]
    fn labels_paint_at_each_targets_top_left_cell() {
        let session = HintSession::new(targets(3));
        let mut frame = Frame::new(GridSize { cols: 80, rows: 24 });
        session.paint(&mut frame, &vp());
        assert_eq!(frame.row_text(0), ALPHABET[0..1].iter().map(|c| *c as char).collect::<String>());
        assert_eq!(frame.row_text(1), ALPHABET[1..2].iter().map(|c| *c as char).collect::<String>());
        assert_eq!(frame.row_text(2), ALPHABET[2..3].iter().map(|c| *c as char).collect::<String>());
    }

    #[test]
    fn a_filtered_out_label_stops_being_painted() {
        let mut session = HintSession::new(targets(100));
        session.push(ALPHABET[0] as char);
        let mut frame = Frame::new(GridSize { cols: 80, rows: 24 });
        session.paint(&mut frame, &vp());
        // With 100 targets the labels are two characters wide, so typing the
        // alphabet's first character keeps the first fourteen and drops the
        // rest. Row 14 held one of the dropped ones.
        assert_eq!(frame.row_text(14), "", "a filtered label was still painted");
        assert_ne!(frame.row_text(0), "", "a matching label stopped being painted");
    }
}
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p wwt-ui`
Expected: compilation fails, `cannot find function labels in this scope`.

- [ ] **Step 3: Write the implementation**

Put this above the test module in `crates/wwt-ui/src/hint.rs`:

```rust
//! Hint labels: assignment, filtering, and painting.

use wwt_frame::{CellPos, Frame, HintTarget, Rgb, Style, Viewport};

/// The home row and the keys nearest it. Fourteen characters label 14
/// targets with one keystroke and 196 with two, which covers all but the
/// densest pages.
pub const ALPHABET: &[u8] = b"sadfjklewcmpgh";

/// Labels must be findable at a glance and must never be mistaken for the
/// page underneath. Reverse video does both, and still reads on a terminal
/// that ignores the colour.
const LABEL_STYLE: Style = Style {
    fg: Rgb { r: 0xff, g: 0xd7, b: 0x00 },
    bold: true,
    reverse: true,
};

/// Labels for `count` targets, all of the same length.
///
/// Uniform length is what makes the set prefix-free: no label can be a
/// prefix of another, so the moment what you have typed matches a label it
/// cannot also be the beginning of a different one. That removes both the
/// timeout and the tie-break rule a variable-length scheme needs.
pub fn labels(count: usize) -> Vec<String> {
    let base = ALPHABET.len();
    let mut width = 1usize;
    let mut capacity = base;
    while capacity < count {
        capacity = capacity.saturating_mul(base);
        width += 1;
    }

    (0..count)
        .map(|index| {
            let mut rest = index;
            let mut label = vec![0u8; width];
            for slot in (0..width).rev() {
                label[slot] = ALPHABET[rest % base];
                rest /= base;
            }
            String::from_utf8(label).expect("the alphabet is ASCII")
        })
        .collect()
}

/// What typing one more character did to the set.
#[derive(Debug, Clone, PartialEq)]
pub enum Filtered {
    /// Still narrowing, with this many targets left.
    Waiting(usize),
    /// One target left. Click it.
    Activate(HintTarget),
    /// Nothing matches what was typed. The caller leaves hint mode.
    None,
}

/// One pass through hint mode: the targets the page reported, their labels,
/// and what has been typed so far.
#[derive(Debug, Clone, PartialEq)]
pub struct HintSession {
    targets: Vec<HintTarget>,
    labels: Vec<String>,
    typed: String,
}

impl HintSession {
    pub fn new(targets: Vec<HintTarget>) -> Self {
        let labels = labels(targets.len());
        Self { targets, labels, typed: String::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    pub fn typed(&self) -> &str {
        &self.typed
    }

    pub fn push(&mut self, c: char) -> Filtered {
        // Labels are lowercase, so a stray shift does not lose your place.
        self.typed.push(c.to_ascii_lowercase());
        self.resolve()
    }

    pub fn pop(&mut self) -> Filtered {
        self.typed.pop();
        self.resolve()
    }

    /// Paint the label of every target that still matches.
    ///
    /// Labels are painted after the page, so they cover the text underneath.
    /// That is what makes them readable, and it is undone the moment hint
    /// mode ends.
    pub fn paint(&self, frame: &mut Frame, vp: &Viewport) {
        for index in self.matching() {
            let cell: CellPos = self.targets[index].label_cell(vp);
            frame.paint_text(cell, &self.labels[index], LABEL_STYLE);
        }
    }

    /// What the statusline says while the labels are up.
    pub fn tag(&self) -> String {
        format!("-- HINT {} ({}) -- ", self.typed, self.matching().len())
    }

    fn matching(&self) -> Vec<usize> {
        self.labels
            .iter()
            .enumerate()
            .filter(|(_, label)| label.starts_with(&self.typed))
            .map(|(index, _)| index)
            .collect()
    }

    fn resolve(&self) -> Filtered {
        let matching = self.matching();
        match matching.len() {
            0 => Filtered::None,
            1 => Filtered::Activate(self.targets[matching[0]].clone()),
            n => Filtered::Waiting(n),
        }
    }
}
```

- [ ] **Step 4: Add the mode variant**

In `crates/wwt-ui/src/mode.rs`, add the import and the variant:

```rust
use crate::hint::HintSession;
```

```rust
    /// Labels are on screen and the next keys filter them.
    Hint(HintSession),
```

In `crates/wwt-ui/src/lib.rs`, add `pub mod hint;` beside the other modules.

In `crates/wwt-ui/src/chrome.rs`, extend `mode_tag`:

```rust
        Mode::Hint(session) => session.tag(),
```

In `crates/wwt/src/core.rs`, extend the `on_key` match so it stays exhaustive:

```rust
            // Wired in Task 11.
            Mode::Hint(_) => false,
```

- [ ] **Step 5: Test the statusline tag**

Add to the `mod tests` block in `crates/wwt-ui/src/chrome.rs`:

```rust
    #[test]
    fn the_statusline_shows_what_has_been_typed_at_the_hints() {
        use crate::hint::HintSession;
        use wwt_frame::{CssRect, HintTarget, TargetKind};

        let targets = vec![HintTarget {
            rect: CssRect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 },
            kind: TargetKind::Clickable,
        }];
        let line = statusline(
            &Mode::Hint(HintSession::new(targets)),
            &State::Ready,
            "https://example.com",
            "",
            0.0,
            60,
        );
        assert!(line.starts_with("-- HINT  (1) --"), "line was {line:?}");
    }
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p wwt-ui`
Expected: PASS, 31 tests.

Run: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 119 tests, clippy clean.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(ui): assign, filter, and paint hint labels

Labels are of uniform length, which makes the set prefix-free: a full
match cannot also be a partial one, so activation needs neither a
timeout nor a tie-break rule."
```

---

### Task 5: Keys reach the page

`Input.dispatchKeyEvent`, the type that describes one key to it, and the two helpers the tests need to see what happened.

**Files:**
- Create: `crates/wwt-page/src/input.rs`
- Create: `crates/wwt-page/tests/fixtures/form.html`, `crates/wwt-page/tests/fixtures/submitted.html`
- Create: `crates/wwt-page/tests/interaction.rs`
- Modify: `crates/wwt-page/src/lib.rs`, `crates/wwt-page/src/extract.rs`

**Interfaces:**
- Consumes: `Page`, `Client::call_on`.
- Produces: `wwt_page::KeyInput { key: String, code: String, windows_virtual_key_code: u32, text: String, modifiers: u32 }`; `Page::eval(&self, expression: &str) -> Result<serde_json::Value>`; `Page::dispatch_key(&self, key: &KeyInput) -> Result<()>`; `Page::blur(&self) -> Result<()>`.

- [ ] **Step 1: Write the fixtures**

`crates/wwt-page/tests/fixtures/form.html`:

```html
<!doctype html>
<meta charset="utf-8">
<title>Form</title>
<body style="margin:0;font:16px monospace">
  <form action="submitted.html" method="get">
    <input id="name" name="name" style="width:200px">
  </form>
  <a id="link" href="simple.html">A link</a>
  <div id="scroller" style="position:absolute;left:0;top:200px;width:300px;height:100px;overflow:auto">
    <div style="height:2000px">inner content</div>
  </div>
  <div style="height:3000px"></div>
</body>
```

`crates/wwt-page/tests/fixtures/submitted.html`:

```html
<!doctype html>
<meta charset="utf-8">
<title>Submitted</title>
<body style="margin:0;font:16px monospace">The form was submitted.</body>
```

- [ ] **Step 2: Write the failing tests**

Create `crates/wwt-page/tests/interaction.rs`:

```rust
use std::sync::Arc;
use std::time::{Duration, Instant};

use wwt_cdp::{Chromium, Client};
use wwt_frame::{CellSize, GridSize, Viewport};
use wwt_page::{KeyInput, Page};

fn fixture_url(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    format!("file://{}", path.display())
}

fn viewport() -> Viewport {
    Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 })
}

struct Harness {
    _browser: Chromium,
    client: Arc<Client>,
}

async fn harness() -> Harness {
    let browser = Chromium::launch().await.expect("launch chromium");
    let client = Client::connect(browser.ws_url()).await.expect("connect");
    Harness { _browser: browser, client: Arc::new(client) }
}

async fn open(h: &Harness, fixture: &str) -> Page {
    Page::open(Arc::clone(&h.client), &fixture_url(fixture), viewport())
        .await
        .expect("open the fixture")
}

/// Poll an expression until it equals `expected`, then return what it last
/// said. Dispatching a key does not wait for what the key sets in motion, so
/// a test that asserts immediately is a test that flakes.
async fn eventually(page: &Page, expression: &str, expected: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let value = page.eval(expression).await.expect("eval");
        let value = value.as_str().unwrap_or_default().to_string();
        if value == expected || Instant::now() > deadline {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A printable key, described the way `keys::describe` will describe it.
fn letter(c: char) -> KeyInput {
    KeyInput {
        key: c.to_string(),
        code: format!("Key{}", c.to_ascii_uppercase()),
        windows_virtual_key_code: c.to_ascii_uppercase() as u32,
        text: c.to_string(),
        modifiers: 0,
    }
}

#[tokio::test]
async fn typed_keys_land_in_the_focused_field() {
    let h = harness().await;
    let page = open(&h, "form.html").await;
    page.eval("document.querySelector('#name').focus()").await.expect("focus");

    for c in "hi".chars() {
        page.dispatch_key(&letter(c)).await.expect("dispatch a key");
    }

    let value = eventually(&page, "document.querySelector('#name').value", "hi").await;
    assert_eq!(value, "hi");
}

#[tokio::test]
async fn enter_submits_the_form_it_is_typed_into() {
    let h = harness().await;
    let page = open(&h, "form.html").await;
    page.eval("document.querySelector('#name').focus()").await.expect("focus");

    let enter = KeyInput {
        key: "Enter".to_string(),
        code: "Enter".to_string(),
        windows_virtual_key_code: 13,
        text: "\r".to_string(),
        modifiers: 0,
    };
    page.dispatch_key(&enter).await.expect("dispatch enter");

    assert_eq!(eventually(&page, "document.title", "Submitted").await, "Submitted");
}

#[tokio::test]
async fn blurring_takes_the_focus_off_the_field() {
    let h = harness().await;
    let page = open(&h, "form.html").await;
    page.eval("document.querySelector('#name').focus()").await.expect("focus");
    assert_eq!(
        eventually(&page, "document.activeElement.id", "name").await,
        "name"
    );

    page.blur().await.expect("blur");

    let active = eventually(&page, "document.activeElement.tagName", "BODY").await;
    assert_eq!(active, "BODY", "focus should have gone back to the document");
}
```

- [ ] **Step 3: Run the tests and watch them fail**

Run: `cargo test -p wwt-page --test interaction`
Expected: compilation fails, `no method named eval found for struct Page`.

- [ ] **Step 4: Write `KeyInput`**

Create `crates/wwt-page/src/input.rs`:

```rust
//! The shapes `Input.dispatchKeyEvent` and `Input.dispatchMouseEvent` want.
//!
//! Building them correctly is the caller's problem: this module only names
//! the fields. Where they come from is a keyboard-layout question, and the
//! binary owns it.

/// Alt. The modifier bits are a CDP bitmask, not crossterm's.
pub const ALT: u32 = 1;
pub const CTRL: u32 = 2;
pub const META: u32 = 4;
pub const SHIFT: u32 = 8;

/// One key press, described four ways.
///
/// `key`, `code`, `windows_virtual_key_code` and `text` must agree with each
/// other or web applications misbehave: anything reading `e.code`, and every
/// application-level keyboard shortcut.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyInput {
    /// The value `e.key` reports: `a`, `A`, `Enter`, `ArrowLeft`.
    pub key: String,
    /// The physical key `e.code` reports: `KeyA`, `Digit1`, `Enter`.
    pub code: String,
    pub windows_virtual_key_code: u32,
    /// What the key inserts. Empty for a key that inserts nothing.
    pub text: String,
    pub modifiers: u32,
}
```

- [ ] **Step 5: Write the page operations**

In `crates/wwt-page/src/extract.rs`, add `use crate::input::KeyInput;` to the imports and these methods to `impl Page`, after `extract`:

```rust
    /// Evaluate an expression in the page and return its value.
    ///
    /// This is the escape hatch the tests use to see what a keystroke did.
    /// It is deliberately not how anything in the browser reads the page:
    /// that is `extract`, once, in one round trip.
    pub async fn eval(&self, expression: &str) -> Result<serde_json::Value> {
        let result = self
            .client
            .call_on(
                &self.session_id,
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true }),
            )
            .await
            .with_context(|| format!("evaluate {expression}"))?;

        if let Some(details) = result.get("exceptionDetails") {
            bail!("{expression} threw: {details}");
        }
        Ok(result["result"]["value"].clone())
    }

    /// Send one key to the page.
    ///
    /// A key that inserts text dispatches `keyDown`, which Chromium turns
    /// into a character insertion. A key that inserts nothing dispatches
    /// `rawKeyDown`, which stays a bare key event. Sending the wrong one
    /// either loses your typing or types your shortcuts.
    pub async fn dispatch_key(&self, key: &KeyInput) -> Result<()> {
        let mut down = json!({
            "type": if key.text.is_empty() { "rawKeyDown" } else { "keyDown" },
            "key": key.key,
            "code": key.code,
            "windowsVirtualKeyCode": key.windows_virtual_key_code,
            "nativeVirtualKeyCode": key.windows_virtual_key_code,
            "modifiers": key.modifiers,
        });
        if !key.text.is_empty() {
            down["text"] = json!(key.text);
            down["unmodifiedText"] = json!(key.text);
        }

        self.client
            .call_on(&self.session_id, "Input.dispatchKeyEvent", down)
            .await
            .context("dispatch a key down")?;
        self.client
            .call_on(
                &self.session_id,
                "Input.dispatchKeyEvent",
                json!({
                    "type": "keyUp",
                    "key": key.key,
                    "code": key.code,
                    "windowsVirtualKeyCode": key.windows_virtual_key_code,
                    "nativeVirtualKeyCode": key.windows_virtual_key_code,
                    "modifiers": key.modifiers,
                }),
            )
            .await
            .context("dispatch a key up")?;
        Ok(())
    }

    /// Take focus off whatever has it.
    ///
    /// Leaving insert mode has to be local: if this fails, the mode changes
    /// anyway. Taking the keyboard back must not depend on the page.
    pub async fn blur(&self) -> Result<()> {
        self.eval("document.activeElement && document.activeElement.blur()")
            .await?;
        Ok(())
    }
```

In `crates/wwt-page/src/lib.rs`, add `pub mod input;` and extend the re-export line with `pub use input::KeyInput;`.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p wwt-page --test interaction`
Expected: PASS, 3 tests.

Run: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 122 tests, clippy clean.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(page): dispatch keys into the page

A key carrying text dispatches keyDown so Chromium inserts a character;
a key carrying none dispatches rawKeyDown so it stays a bare event."
```

---

### Task 6: The key table

The tedious, bounded, load-bearing part: a crossterm `KeyEvent` becomes the quad CDP needs. It lives in the binary because its output type belongs to `wwt-page` and its input type belongs to crossterm, and putting it in either of those crates would point a dependency edge backwards.

**Files:**
- Create: `crates/wwt/src/keys.rs`
- Modify: `crates/wwt/src/lib.rs`

**Interfaces:**
- Consumes: `wwt_page::KeyInput` and the modifier constants from Task 5.
- Produces: `wwt::keys::describe(event: KeyEvent) -> Option<KeyInput>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/wwt/src/keys.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyInput {
        describe(KeyEvent::new(code, modifiers)).expect("a bound key")
    }

    #[test]
    fn a_letter_reports_its_physical_key() {
        let k = key(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(k.key, "a");
        assert_eq!(k.code, "KeyA");
        assert_eq!(k.windows_virtual_key_code, 65);
        assert_eq!(k.text, "a");
        assert_eq!(k.modifiers, 0);
    }

    #[test]
    fn a_capital_is_the_same_physical_key_with_shift() {
        let k = key(KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert_eq!(k.code, "KeyA", "shift does not change which key was pressed");
        assert_eq!(k.windows_virtual_key_code, 65);
        assert_eq!(k.key, "A");
        assert_eq!(k.text, "A");
        assert_eq!(k.modifiers, wwt_page::input::SHIFT);
    }

    #[test]
    fn shifted_punctuation_reports_the_unshifted_code() {
        // `!` is produced by pressing Digit1, and a page reading e.code
        // needs to hear that rather than a key named after the glyph.
        let k = key(KeyCode::Char('!'), KeyModifiers::SHIFT);
        assert_eq!(k.code, "Digit1");
        assert_eq!(k.windows_virtual_key_code, 49);
        assert_eq!(k.key, "!");
        assert_eq!(k.text, "!");
    }

    #[test]
    fn control_suppresses_the_text_but_keeps_the_key() {
        // Ctrl-s must reach a page's save handler without leaving an `s` in
        // whatever box has focus.
        let k = key(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(k.code, "KeyS");
        assert_eq!(k.key, "s");
        assert_eq!(k.text, "", "a modified key inserts nothing");
        assert_eq!(k.modifiers, wwt_page::input::CTRL);
    }

    #[test]
    fn enter_and_tab_carry_their_control_characters() {
        let enter = key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(enter.text, "\r");
        assert_eq!(enter.windows_virtual_key_code, 13);
        let tab = key(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(tab.text, "\t");
        assert_eq!(tab.windows_virtual_key_code, 9);
    }

    #[test]
    fn a_named_key_inserts_nothing() {
        let esc = key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(esc.key, "Escape");
        assert_eq!(esc.code, "Escape");
        assert_eq!(esc.windows_virtual_key_code, 27);
        assert_eq!(esc.text, "");

        let left = key(KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(left.key, "ArrowLeft");
        assert_eq!(left.windows_virtual_key_code, 37);
    }

    #[test]
    fn function_keys_number_themselves() {
        let f5 = key(KeyCode::F(5), KeyModifiers::NONE);
        assert_eq!(f5.key, "F5");
        assert_eq!(f5.code, "F5");
        assert_eq!(f5.windows_virtual_key_code, 116);
    }

    #[test]
    fn an_unmapped_key_is_dropped_rather_than_guessed_at() {
        assert!(describe(KeyEvent::new(KeyCode::Menu, KeyModifiers::NONE)).is_none());
        assert!(describe(KeyEvent::new(KeyCode::F(25), KeyModifiers::NONE)).is_none());
    }

    #[test]
    fn the_space_bar_types_a_space() {
        let k = key(KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(k.code, "Space");
        assert_eq!(k.windows_virtual_key_code, 32);
        assert_eq!(k.text, " ");
    }
}
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p wwt keys`
Expected: compilation fails, `cannot find function describe in this scope`.

- [ ] **Step 3: Write the implementation**

Put this above the test module in `crates/wwt/src/keys.rs`:

```rust
//! Crossterm key events, described the way `Input.dispatchKeyEvent` needs.
//!
//! The mapping is a US layout. On other layouts the character you typed is
//! still correct, because crossterm reports the character the terminal
//! produced, but `e.code` names the physical key a US keyboard would have
//! used. The terminal does not report the layout, so there is nothing better
//! available.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use wwt_page::KeyInput;
use wwt_page::input::{ALT, CTRL, META, SHIFT};

/// US-layout punctuation: the character, the physical key that produces it,
/// and that key's virtual key code. Shifted and unshifted characters share a
/// physical key, which is the entire point of the table: `!` is `Digit1`.
const PUNCTUATION: &[(char, &str, u32)] = &[
    (' ', "Space", 32),
    ('`', "Backquote", 192),
    ('~', "Backquote", 192),
    ('-', "Minus", 189),
    ('_', "Minus", 189),
    ('=', "Equal", 187),
    ('+', "Equal", 187),
    ('[', "BracketLeft", 219),
    ('{', "BracketLeft", 219),
    (']', "BracketRight", 221),
    ('}', "BracketRight", 221),
    ('\\', "Backslash", 220),
    ('|', "Backslash", 220),
    (';', "Semicolon", 186),
    (':', "Semicolon", 186),
    ('\'', "Quote", 222),
    ('"', "Quote", 222),
    (',', "Comma", 188),
    ('<', "Comma", 188),
    ('.', "Period", 190),
    ('>', "Period", 190),
    ('/', "Slash", 191),
    ('?', "Slash", 191),
    ('!', "Digit1", 49),
    ('@', "Digit2", 50),
    ('#', "Digit3", 51),
    ('$', "Digit4", 52),
    ('%', "Digit5", 53),
    ('^', "Digit6", 54),
    ('&', "Digit7", 55),
    ('*', "Digit8", 56),
    ('(', "Digit9", 57),
    (')', "Digit0", 48),
];

/// The physical key a character comes from, as (`code`, virtual key code).
fn physical(c: char) -> Option<(String, u32)> {
    if c.is_ascii_alphabetic() {
        let upper = c.to_ascii_uppercase();
        // ASCII uppercase and the virtual key codes agree for letters.
        return Some((format!("Key{upper}"), upper as u32));
    }
    if c.is_ascii_digit() {
        return Some((format!("Digit{c}"), c as u32));
    }
    PUNCTUATION
        .iter()
        .find(|(character, _, _)| *character == c)
        .map(|(_, code, vk)| ((*code).to_string(), *vk))
}

fn modifiers_of(modifiers: KeyModifiers) -> u32 {
    let mut mask = 0;
    if modifiers.contains(KeyModifiers::ALT) {
        mask |= ALT;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        mask |= CTRL;
    }
    if modifiers.contains(KeyModifiers::SUPER) {
        mask |= META;
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        mask |= SHIFT;
    }
    mask
}

/// Describe one key, or `None` if it is not one we know how to send.
///
/// Unknown keys are dropped rather than approximated: a wrong `code` is
/// worse than a missing keystroke, because the page acts on it.
pub fn describe(event: KeyEvent) -> Option<KeyInput> {
    let modifiers = modifiers_of(event.modifiers);

    let (key, code, vk, text) = match event.code {
        KeyCode::Char(c) => {
            let (code, vk) = physical(c)?;
            (c.to_string(), code, vk, c.to_string())
        }
        KeyCode::Enter => ("Enter".into(), "Enter".into(), 13, "\r".to_string()),
        KeyCode::Tab => ("Tab".into(), "Tab".into(), 9, "\t".to_string()),
        KeyCode::Backspace => ("Backspace".into(), "Backspace".into(), 8, String::new()),
        KeyCode::Delete => ("Delete".into(), "Delete".into(), 46, String::new()),
        KeyCode::Esc => ("Escape".into(), "Escape".into(), 27, String::new()),
        KeyCode::Left => ("ArrowLeft".into(), "ArrowLeft".into(), 37, String::new()),
        KeyCode::Up => ("ArrowUp".into(), "ArrowUp".into(), 38, String::new()),
        KeyCode::Right => ("ArrowRight".into(), "ArrowRight".into(), 39, String::new()),
        KeyCode::Down => ("ArrowDown".into(), "ArrowDown".into(), 40, String::new()),
        KeyCode::Home => ("Home".into(), "Home".into(), 36, String::new()),
        KeyCode::End => ("End".into(), "End".into(), 35, String::new()),
        KeyCode::PageUp => ("PageUp".into(), "PageUp".into(), 33, String::new()),
        KeyCode::PageDown => ("PageDown".into(), "PageDown".into(), 34, String::new()),
        KeyCode::Insert => ("Insert".into(), "Insert".into(), 45, String::new()),
        KeyCode::F(n @ 1..=12) => {
            let name = format!("F{n}");
            (name.clone(), name, 111 + u32::from(n), String::new())
        }
        _ => return None,
    };

    Some(KeyInput {
        key,
        code,
        windows_virtual_key_code: vk,
        // Ctrl and Meta turn a key into a command rather than a character.
        // Shift does not: crossterm has already applied it.
        text: if modifiers & (CTRL | META) != 0 { String::new() } else { text },
        modifiers,
    })
}
```

Add `pub mod keys;` to `crates/wwt/src/lib.rs`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p wwt keys`
Expected: PASS, 9 tests.

Run: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 131 tests, clippy clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(wwt): map terminal keys onto CDP key events

Ctrl and Meta suppress the inserted text so a page's own shortcuts fire
without typing a character, and shifted punctuation reports the
unshifted physical key that produced it."
```

---

### Task 7: Clicks and wheels

**Files:**
- Modify: `crates/wwt-page/src/input.rs`, `crates/wwt-page/src/extract.rs`, `crates/wwt-page/src/lib.rs`
- Modify: `crates/wwt-page/tests/interaction.rs`

**Interfaces:**
- Consumes: `wwt_frame::CssPoint`, the `form.html` fixture from Task 5.
- Produces: `wwt_page::{MouseInput, MouseAction}`; `MouseInput::{press, release, wheel}`; `Page::dispatch_mouse(&self, mouse: &MouseInput) -> Result<()>`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/wwt-page/tests/interaction.rs`, and extend its `use wwt_page::{KeyInput, Page};` line to `use wwt_page::{KeyInput, MouseInput, Page};`, plus `use wwt_frame::CssPoint;`:

```rust
/// Where an element is, right now, in CSS pixels.
async fn center_of(page: &Page, selector: &str) -> CssPoint {
    let value = page
        .eval(&format!(
            "(() => {{ const r = document.querySelector('{selector}').getBoundingClientRect(); \
              return {{ x: r.left + r.width / 2, y: r.top + r.height / 2 }}; }})()"
        ))
        .await
        .expect("read a rect");
    CssPoint {
        x: value["x"].as_f64().expect("an x"),
        y: value["y"].as_f64().expect("a y"),
    }
}

#[tokio::test]
async fn clicking_a_link_follows_it() {
    let h = harness().await;
    let page = open(&h, "form.html").await;
    let at = center_of(&page, "#link").await;

    page.dispatch_mouse(&MouseInput::press(at)).await.expect("press");
    page.dispatch_mouse(&MouseInput::release(at)).await.expect("release");

    assert_eq!(
        eventually(&page, "document.title", "Fixture Page").await,
        "Fixture Page"
    );
}

#[tokio::test]
async fn a_wheel_scrolls_what_is_under_the_pointer() {
    let h = harness().await;
    let page = open(&h, "form.html").await;
    let at = center_of(&page, "#scroller").await;

    page.dispatch_mouse(&MouseInput::wheel(at, 200.0)).await.expect("wheel");

    assert_eq!(
        eventually(
            &page,
            "String(document.querySelector('#scroller').scrollTop > 0)",
            "true"
        )
        .await,
        "true",
        "the scroller under the pointer should have moved"
    );
    let document_scroll = page.eval("window.scrollY").await.expect("scrollY");
    assert_eq!(
        document_scroll.as_f64(),
        Some(0.0),
        "the document must not scroll when a nested scroller was under the pointer"
    );
}
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p wwt-page --test interaction`
Expected: compilation fails, `cannot find MouseInput in wwt_page`.

- [ ] **Step 3: Write `MouseInput`**

Add to `crates/wwt-page/src/input.rs`:

```rust
use wwt_frame::CssPoint;

/// What a mouse event does at a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MouseAction {
    Press,
    Release,
    /// A wheel turn in CSS pixels, positive being downward.
    Wheel(f64),
}

/// One mouse event, at a point in the page's own coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseInput {
    pub at: CssPoint,
    pub action: MouseAction,
}

impl MouseInput {
    pub fn press(at: CssPoint) -> Self {
        Self { at, action: MouseAction::Press }
    }

    pub fn release(at: CssPoint) -> Self {
        Self { at, action: MouseAction::Release }
    }

    pub fn wheel(at: CssPoint, dy: f64) -> Self {
        Self { at, action: MouseAction::Wheel(dy) }
    }
}
```

Extend the re-export in `crates/wwt-page/src/lib.rs` to `pub use input::{KeyInput, MouseAction, MouseInput};`.

- [ ] **Step 4: Write the page operation**

In `crates/wwt-page/src/extract.rs`, add `MouseAction, MouseInput` to the `use crate::input::...` line and this method to `impl Page`:

```rust
    /// Send one mouse event to the page.
    ///
    /// The point is the page's, not the terminal's: the caller converts
    /// through `Viewport`, which is the only place a cell becomes a pixel.
    pub async fn dispatch_mouse(&self, mouse: &MouseInput) -> Result<()> {
        let params = match mouse.action {
            MouseAction::Press => json!({
                "type": "mousePressed",
                "x": mouse.at.x,
                "y": mouse.at.y,
                "button": "left",
                "buttons": 1,
                "clickCount": 1,
                "modifiers": 0,
            }),
            MouseAction::Release => json!({
                "type": "mouseReleased",
                "x": mouse.at.x,
                "y": mouse.at.y,
                "button": "left",
                "buttons": 0,
                "clickCount": 1,
                "modifiers": 0,
            }),
            MouseAction::Wheel(dy) => json!({
                "type": "mouseWheel",
                "x": mouse.at.x,
                "y": mouse.at.y,
                "deltaX": 0.0,
                "deltaY": dy,
                "button": "none",
                "clickCount": 0,
                "modifiers": 0,
            }),
        };

        self.client
            .call_on(&self.session_id, "Input.dispatchMouseEvent", params)
            .await
            .context("dispatch a mouse event")?;
        Ok(())
    }
```

- [ ] **Step 5: Fold `scroll_by` into it**

`scroll_by` builds its own wheel event. Now that there is one wheel implementation, make it use it. Replace the body of `scroll_by` with:

```rust
    pub async fn scroll_by(&self, dy: f64, vp: Viewport) -> Result<()> {
        // Aimed at the middle of the viewport, because a keyboard scroll has
        // no pointer to aim with.
        let at = CssPoint {
            x: f64::from(vp.css_width()) / 2.0,
            y: f64::from(vp.css_height()) / 2.0,
        };
        self.dispatch_mouse(&MouseInput::wheel(at, dy)).await
    }
```

Keep the existing doc comment on `scroll_by`. Add `CssPoint` to the `use wwt_frame::{...}` line.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p wwt-page`
Expected: PASS, 27 tests. The M2 scroll tests must still pass: they are what proves the refactor kept the wheel behaviour.

Run: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 133 tests, clippy clean.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(page): dispatch clicks and wheels at a point

Wheels now aim at the pointer, so a nested scroller under the cursor
scrolls instead of the document. Keyboard scrolling aims at the middle
of the viewport and goes through the same path."
```

---

### Task 8: The hint query

The page-side half of hints: find what can be interacted with, discard what is hidden, off screen, or covered, and report the rest with its geometry.

**Files:**
- Modify: `crates/wwt-page/assets/bootstrap.js`
- Modify: `crates/wwt-page/src/extract.rs`
- Create: `crates/wwt-page/tests/fixtures/interactive.html`, `crates/wwt-page/tests/fixtures/links.html`
- Modify: `crates/wwt-page/tests/interaction.rs`

**Interfaces:**
- Consumes: `wwt_frame::{CssRect, HintTarget, TargetKind}` from Task 1.
- Produces: `Page::hints(&self) -> Result<Vec<HintTarget>>`, in document order.

- [ ] **Step 1: Write the fixtures**

`crates/wwt-page/tests/fixtures/interactive.html`:

```html
<!doctype html>
<meta charset="utf-8">
<title>Interactive</title>
<body style="margin:0;font:16px monospace">
  <a href="#one" id="one">One</a>
  <button id="two">Two</button>
  <input id="three" type="text">
  <a href="#hidden" id="hidden" style="display:none">Hidden</a>
  <a href="#offscreen" id="offscreen" style="position:absolute;left:0;top:3000px">Offscreen</a>
  <a href="#covered" id="covered"
     style="position:absolute;left:600px;top:0;width:100px;height:40px">Covered</a>
  <div style="position:absolute;left:600px;top:0;width:100px;height:40px;background:#fff"></div>
</body>
```

`crates/wwt-page/tests/fixtures/links.html`, for the measurement. Every paragraph carries a link, so the hit test runs on every one of them:

```html
<!doctype html>
<meta charset="utf-8">
<title>Links Fixture</title>
<style>body { margin: 0; font: 16px/20px sans-serif; width: 720px; }</style>
<script>
  let html = "";
  for (let i = 0; i < 1500; i++) {
    html += `<p><a href="#${i}">link number ${i}</a> and some trailing text</p>`;
  }
  document.write(html);
</script>
```

- [ ] **Step 2: Write the failing tests**

Add to `crates/wwt-page/tests/interaction.rs`, extending its imports with `use wwt_frame::TargetKind;`:

```rust
#[tokio::test]
async fn hints_find_every_interactive_element_in_document_order() {
    let h = harness().await;
    let targets = open(&h, "interactive.html").await.hints().await.expect("hints");

    let kinds: Vec<TargetKind> = targets.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![TargetKind::Clickable, TargetKind::Clickable, TargetKind::Editable],
        "expected the link, the button, and the text field, in that order"
    );
}

#[tokio::test]
async fn hints_skip_what_is_outside_the_viewport() {
    let h = harness().await;
    let targets = open(&h, "interactive.html").await.hints().await.expect("hints");

    assert!(
        targets.iter().all(|t| t.rect.y < 1000.0),
        "a target 3000px down the page was labelled: {targets:?}"
    );
}

#[tokio::test]
async fn hints_skip_what_something_else_is_covering() {
    let h = harness().await;
    let targets = open(&h, "interactive.html").await.hints().await.expect("hints");

    // The covered link is the only thing at x >= 600. A label on it would
    // lie: the click would land on the div on top of it.
    assert!(
        targets.iter().all(|t| t.rect.x < 600.0),
        "a covered target was labelled: {targets:?}"
    );
}

#[tokio::test]
async fn hint_geometry_is_the_element_s_own_box() {
    let h = harness().await;
    let page = open(&h, "interactive.html").await;
    let targets = page.hints().await.expect("hints");
    let button = &targets[1];

    let expected = center_of(&page, "#two").await;
    assert!(
        (button.center().x - expected.x).abs() < 1.0
            && (button.center().y - expected.y).abs() < 1.0,
        "hint centre {:?} should match the button's own centre {expected:?}",
        button.center()
    );
}

#[tokio::test]
async fn measure_hints_on_a_page_full_of_links() {
    let h = harness().await;
    let page = open(&h, "links.html").await;

    // One warm pass, so the number is steady-state rather than first-run.
    page.hints().await.expect("hints");

    let start = std::time::Instant::now();
    let targets = page.hints().await.expect("hints");
    let elapsed = start.elapsed();

    println!("links.html: {} targets found in {elapsed:?}", targets.len());
    assert!(!targets.is_empty(), "the fixture is full of links");
}
```

- [ ] **Step 3: Run the tests and watch them fail**

Run: `cargo test -p wwt-page --test interaction`
Expected: compilation fails, `no method named hints found for struct Page`.

- [ ] **Step 4: Add `hints()` to the injected script**

In `crates/wwt-page/assets/bootstrap.js`, add this above `window.__wwt = ...`:

```js
  // What counts as interactive. Anything a click or a keystroke does
  // something to, which is broader than "has an href".
  const HINT_SELECTOR = [
    "a[href]",
    "button",
    "input:not([type=hidden])",
    "select",
    "textarea",
    "[contenteditable='']",
    "[contenteditable='true']",
    "[role=button]",
    "[role=link]",
    "[role=checkbox]",
    "[role=radio]",
    "[role=menuitem]",
    "[role=tab]",
    "[role=textbox]",
    "[tabindex]:not([tabindex='-1'])",
  ].join(",");

  // Input types you type into, as opposed to the ones you click.
  const TYPABLE = new Set([
    "text", "search", "email", "url", "tel", "password", "number",
    "date", "time", "month", "week", "datetime-local",
  ]);

  function isEditable(el) {
    if (el.isContentEditable) return true;
    if (el.tagName === "TEXTAREA") return true;
    if (el.tagName !== "INPUT") return false;
    return TYPABLE.has((el.getAttribute("type") || "text").toLowerCase());
  }

  // Interactive boxes, in document order.
  //
  // This is deliberately not part of extract(): it sweeps the whole
  // document and pays a hit test per candidate, and extraction runs on
  // every scroll frame. This runs when someone presses `f`.
  function hints() {
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const out = [];

    for (const el of document.querySelectorAll(HINT_SELECTOR)) {
      if (el.disabled) continue;

      const cs = window.getComputedStyle(el);
      if (cs.visibility === "hidden" || cs.display === "none" || cs.opacity === "0") {
        continue;
      }

      const r = el.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) continue;
      if (r.bottom < 0 || r.top > vh || r.right < 0 || r.left > vw) continue;

      // The point a click would land on. If something else is on top of it,
      // a label here would lie about what pressing it does.
      const x = Math.min(Math.max(r.left + r.width / 2, 0), vw - 1);
      const y = Math.min(Math.max(r.top + r.height / 2, 0), vh - 1);
      const hit = document.elementFromPoint(x, y);
      if (!hit) continue;
      if (hit !== el && !el.contains(hit) && !hit.contains(el)) continue;

      out.push({
        x: r.left,
        y: r.top,
        w: r.width,
        h: r.height,
        editable: isEditable(el),
      });
    }

    return out;
  }
```

Change the last line of the script from `window.__wwt = { extract };` to:

```js
  window.__wwt = { extract, hints };
```

- [ ] **Step 5: Add `Page::hints`**

In `crates/wwt-page/src/extract.rs`, add the raw shape beside `RawRun`:

```rust
/// The shape one entry of `__wwt.hints()` returns.
#[derive(Debug, Deserialize)]
struct RawTarget {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    editable: bool,
}
```

Add `HintTarget, TargetKind` to the `use wwt_frame::{...}` line, and this method to `impl Page`:

```rust
    /// Every interactive box on screen, in document order.
    ///
    /// Run when hint mode opens rather than during extraction: it sweeps the
    /// document and hit-tests each candidate, which is too much to pay on
    /// every scroll frame for something pressed occasionally.
    pub async fn hints(&self) -> Result<Vec<HintTarget>> {
        let value = self.eval("window.__wwt.hints()").await.context("run the hint query")?;
        let raw: Vec<RawTarget> = serde_json::from_value(value)
            .context("the hint query returned an unexpected shape")?;

        Ok(raw
            .into_iter()
            .map(|t| HintTarget {
                rect: CssRect { x: t.x, y: t.y, w: t.w, h: t.h },
                kind: if t.editable { TargetKind::Editable } else { TargetKind::Clickable },
            })
            .collect())
    }
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p wwt-page --test interaction`
Expected: PASS, 10 tests.

Run: `cargo test -p wwt-page --test interaction measure_hints -- --nocapture`
Expected: PASS, with a line like `links.html: 1500 targets found in 40ms`. **Record that number in the commit message.** If it is above 250ms, stop and say so: the design accepted one hit test per candidate on the assumption it is affordable, and a number that large is evidence against the assumption rather than something to work around.

Run: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 138 tests, clippy clean.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(page): query the page for interactive boxes

A candidate survives three filters: it has a box, the box is on screen,
and a hit test at its centre reaches it. The third is what keeps a
sticky header from handing out labels for the links it covers.

links.html: <N> targets in <T>."
```

---

### Task 9: The input pump

Every page operation so far has been idempotent or self-cancelling, so spawning each one independently was safe. Keystrokes are not: three keys spawned as three tasks race, and `abc` sometimes arrives as `acb`. One task consuming one channel makes ordering a property of the channel rather than of the scheduler.

**Files:**
- Create: `crates/wwt/src/input.rs`
- Create: `crates/wwt/tests/fixtures/form.html`, `crates/wwt/tests/input.rs`
- Modify: `crates/wwt/src/lib.rs`

**Interfaces:**
- Consumes: `wwt_page::{KeyInput, MouseInput, Page}` from Tasks 5 and 7.
- Produces: `wwt::input::{Input, InputPump}`; `Input::{Key(KeyInput), Mouse(MouseInput)}`; `InputPump::spawn(page: Arc<Page>, errors: mpsc::UnboundedSender<String>) -> InputPump`; `InputPump::send(&self, input: Input)`.

- [ ] **Step 1: Write the fixture**

`crates/wwt/tests/fixtures/form.html`:

```html
<!doctype html>
<meta charset="utf-8">
<title>Form</title>
<body style="margin:0;font:16px monospace">
  <input id="name" style="width:400px">
</body>
```

- [ ] **Step 2: Write the failing test**

Create `crates/wwt/tests/input.rs`:

```rust
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use wwt::input::{Input, InputPump};
use wwt_cdp::{Chromium, Client};
use wwt_frame::{CellSize, GridSize, Viewport};
use wwt_page::{KeyInput, Page};

fn fixture_url(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    format!("file://{}", path.display())
}

fn letter(c: char) -> KeyInput {
    KeyInput {
        key: c.to_string(),
        code: format!("Key{}", c.to_ascii_uppercase()),
        windows_virtual_key_code: c.to_ascii_uppercase() as u32,
        text: c.to_string(),
        modifiers: 0,
    }
}

/// The space bar comes off a key of its own, not `Key `.
fn space() -> KeyInput {
    KeyInput {
        key: " ".to_string(),
        code: "Space".to_string(),
        windows_virtual_key_code: 32,
        text: " ".to_string(),
        modifiers: 0,
    }
}

/// Typing is the first page operation whose order matters. Sending a burst
/// without awaiting anything is exactly what the core loop does, so this is
/// the shape of the bug it would otherwise have.
#[tokio::test]
async fn a_burst_of_keys_arrives_in_the_order_it_was_typed() {
    let browser = Chromium::launch().await.expect("launch chromium");
    let client = Arc::new(Client::connect(browser.ws_url()).await.expect("connect"));
    let vp = Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
    let page = Arc::new(
        Page::open(Arc::clone(&client), &fixture_url("form.html"), vp)
            .await
            .expect("open the fixture"),
    );
    page.eval("document.querySelector('#name').focus()").await.expect("focus");

    let (errors_tx, mut errors_rx) = mpsc::unbounded_channel();
    let pump = InputPump::spawn(Arc::clone(&page), errors_tx);

    let typed = "the quick brown fox";
    for c in typed.chars() {
        // No await between sends: the pump is what keeps these in order.
        pump.send(Input::Key(if c == ' ' { space() } else { letter(c) }));
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut value = String::new();
    while Instant::now() < deadline {
        value = page
            .eval("document.querySelector('#name').value")
            .await
            .expect("read the field")
            .as_str()
            .unwrap_or_default()
            .to_string();
        if value.len() == typed.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert_eq!(value, typed);
    assert!(errors_rx.try_recv().is_err(), "the pump reported an error");
}
```

- [ ] **Step 3: Run the test and watch it fail**

Run: `cargo test -p wwt --test input`
Expected: compilation fails, `unresolved import wwt::input`.

- [ ] **Step 4: Write the pump**

Create `crates/wwt/src/input.rs`:

```rust
//! Ordered input delivery.
//!
//! Every other page operation is idempotent or self-cancelling, so the core
//! spawns each one and lets them race. Keystrokes cannot: three keys as
//! three tasks would sometimes deliver `abc` as `acb`. One long-lived task
//! draining one channel makes ordering a property of the channel rather
//! than of the scheduler, and sending to an unbounded channel does not
//! await, so the loop still never blocks.

use std::sync::Arc;

use tokio::sync::mpsc;
use wwt_page::{KeyInput, MouseInput, Page};

/// One thing to send to the page.
#[derive(Debug, Clone)]
pub enum Input {
    Key(KeyInput),
    Mouse(MouseInput),
}

/// The sending half of the pump. Cheap to clone-free: the core holds one.
pub struct InputPump {
    tx: mpsc::UnboundedSender<Input>,
}

impl InputPump {
    /// Start the pump for a page.
    ///
    /// Failures are reported on `errors` rather than returned: by the time a
    /// keystroke fails, whoever typed it has typed three more.
    pub fn spawn(page: Arc<Page>, errors: mpsc::UnboundedSender<String>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Input>();

        tokio::spawn(async move {
            while let Some(input) = rx.recv().await {
                let result = match &input {
                    Input::Key(key) => page.dispatch_key(key).await,
                    Input::Mouse(mouse) => page.dispatch_mouse(mouse).await,
                };
                if let Err(error) = result {
                    let _ = errors.send(error.to_string());
                }
            }
        });

        Self { tx }
    }

    /// Queue one input. Never blocks, never fails: a closed channel means
    /// the pump task is gone, which only happens on the way out.
    pub fn send(&self, input: Input) {
        let _ = self.tx.send(input);
    }
}
```

Add `pub mod input;` to `crates/wwt/src/lib.rs`.

- [ ] **Step 5: Run the test**

Run: `cargo test -p wwt --test input`
Expected: PASS, 1 test.

Run: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 139 tests, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(wwt): deliver input through one ordered pump

Spawning a task per keystroke would race: abc would sometimes arrive as
acb. One task draining one channel makes ordering a property of the
channel instead of the scheduler."
```

---

### Task 10: Insert mode

`i` hands the keyboard to the page and `Esc` takes it back. This is the task that makes forms usable.

**Files:**
- Modify: `crates/wwt/src/keymap.rs`, `crates/wwt/src/core.rs`, `crates/wwt/tests/smoke.rs`

**Interfaces:**
- Consumes: `wwt::keys::describe`, `wwt::input::{Input, InputPump}`, `wwt_ui::Mode::Insert`.
- Produces: `keymap::Action::Insert`; `Core` fields `input: InputPump` and `errors_rx: mpsc::UnboundedReceiver<String>`; `Core::send_key`, `Core::blur`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/wwt/src/keymap.rs`'s test module:

```rust
    #[test]
    fn i_hands_the_keyboard_to_the_page() {
        assert_eq!(action_for(key('i'), vp()), Some(Action::Insert));
    }
```

Add to `crates/wwt/tests/smoke.rs`:

```rust
/// The same physical key means two different things depending on the mode,
/// which is the whole point of having modes. Normal mode's `q` quits; insert
/// mode's `q` is a letter.
#[test]
fn a_letter_is_a_command_in_normal_mode_and_a_keystroke_in_insert_mode() {
    let vp = wwt_frame::Viewport::new(
        wwt_frame::GridSize { cols: 80, rows: 24 },
        wwt_frame::CellSize { w: 9, h: 20 },
    );
    let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);

    assert_eq!(wwt::keymap::action_for(q, vp), Some(wwt::keymap::Action::Quit));
    assert_eq!(
        wwt::keys::describe(q).expect("q is a key we can send").text,
        "q"
    );

    let i = KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE);
    assert_eq!(
        wwt::keymap::action_for(i, vp),
        Some(wwt::keymap::Action::Insert),
        "`i` is what puts you in the mode where q is a letter"
    );
}
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p wwt`
Expected: compilation fails, `no variant named Insert found for enum Action`.

- [ ] **Step 3: Bind the key**

In `crates/wwt/src/keymap.rs`, add the variant with a comment and the binding:

```rust
    /// Hand the keyboard to the page until `Esc`.
    Insert,
```

```rust
        KeyCode::Char('i') => Some(Action::Insert),
```

Put the binding just above the `:` line so the enum and the match stay in the same order.

- [ ] **Step 4: Give `Core` a pump**

In `crates/wwt/src/core.rs`, extend the imports:

```rust
use crossterm::event::{
    Event as TermEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};

use crate::input::{Input, InputPump};
use crate::keys;
```

`Mode` and `chrome` are already imported from Task 3.

Add the fields to `struct Core`, and to the struct literal `Core::new` returns:

```rust
    /// Ordered delivery of keys and clicks to the page.
    input: InputPump,
    /// Where things that failed after the loop moved on report themselves:
    /// a keystroke that did not land, a blur that did not take.
    errors_tx: mpsc::UnboundedSender<String>,
```

Add a variant to `enum Job`:

```rust
    /// A key, a click, or a blur failed after the loop had moved on.
    InputFailed(String),
```

And build the pump in `Core::new`, before the struct literal:

```rust
        let (errors_tx, mut errors_rx) = mpsc::unbounded_channel::<String>();
        let input = InputPump::spawn(Arc::clone(&page), errors_tx.clone());

        // Input failures arrive on their own channel and are folded into the
        // one the loop already selects on. Two receivers would mean two
        // mutable borrows of `self` inside one `select!`, which does not
        // compile, and a second failure path the loop has to think about.
        let jobs_for_errors = jobs_tx.clone();
        tokio::spawn(async move {
            while let Some(message) = errors_rx.recv().await {
                let _ = jobs_for_errors.send(Job::InputFailed(message));
            }
        });
```

- [ ] **Step 5: Route insert-mode keys**

Replace the `Mode::Insert => false` stub in `Core::on_key` with:

```rust
            Mode::Insert => {
                match key.code {
                    // Never forwarded. A page cannot trap the keyboard,
                    // which is what makes handing it over safe.
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        self.blur();
                    }
                    // A terminal transmits `Ctrl-[` as 0x1B, which is
                    // Escape: the two are one keystroke on the wire. So the
                    // page's Escape lives on `Ctrl-]`, which is 0x1D.
                    KeyCode::Char(']') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.send_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                    }
                    _ => self.send_key(key),
                }
                false
            }
```

Add the `Insert` arm to the `Mode::Normal` match, beside `EnterCommand`:

```rust
                Some(Action::Insert) => {
                    self.mode = Mode::Insert;
                    false
                }
```

And to `run_action`'s "handled by the caller" arm:

```rust
            Action::Quit | Action::EnterCommand(_) | Action::Insert => {}
```

Add the two helpers to `impl Core`:

```rust
    /// Forward one key to the page, if it is one we know how to describe.
    ///
    /// An unknown key is dropped rather than approximated: a wrong `code` is
    /// worse than a missing keystroke, because the page acts on it.
    fn send_key(&self, key: KeyEvent) {
        if let Some(input) = keys::describe(key) {
            self.input.send(Input::Key(input));
        }
    }

    /// Take focus off whatever has it, without waiting for it.
    ///
    /// Leaving insert mode has already happened by the time this runs. If it
    /// fails the statusline says so, and the keyboard is still yours: taking
    /// it back must never depend on the page.
    fn blur(&self) {
        let page = Arc::clone(&self.page);
        let errors = self.errors_tx.clone();
        tokio::spawn(async move {
            if let Err(error) = page.blur().await {
                let _ = errors.send(error.to_string());
            }
        });
    }
```

- [ ] **Step 6: Report out-of-band failures**

Add the arm to `Core::on_job`. The `select!` is untouched: it still has one
job receiver.

```rust
            // The frame stays exactly as it was; only the statusline
            // changes. Spec section 8. Deliberately not `Job::Failed`: that
            // one clears the extraction and navigation flags, and a
            // keystroke that failed has finished neither of those.
            Job::InputFailed(message) => self.state = State::Error(message),
```

- [ ] **Step 7: Run the tests**

Run: `cargo test --workspace`
Expected: PASS, 141 tests.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 8: Try it**

```bash
cargo run -p wwt -- duckduckgo.com
```

Press `i`, type something, and watch it appear in the search box. Press `Esc` and check that `j` scrolls again rather than typing a `j`. Press `q` to leave.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(wwt): hand the keyboard to the page with i

Esc is never forwarded, so the keyboard is always one key away from
being yours again. Ctrl-] sends the page a literal Escape, because a
terminal cannot distinguish Ctrl-[ from Esc."
```

---

### Task 11: Hint mode

**Files:**
- Modify: `crates/wwt/src/keymap.rs`, `crates/wwt/src/core.rs`
- Modify: `crates/wwt-ui/src/chrome.rs`

**Interfaces:**
- Consumes: `Page::hints`, `wwt_ui::hint::{Filtered, HintSession}`, `wwt_frame::{HintTarget, TargetKind}`, `wwt_page::MouseInput`.
- Produces: `keymap::Action::Hints`; `chrome::State::Notice(String)`; `Core` field `hints: Option<Vec<HintTarget>>`; `Job::Hints(Vec<HintTarget>)`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/wwt/src/keymap.rs`'s test module:

```rust
    #[test]
    fn f_opens_the_hints() {
        assert_eq!(action_for(key('f'), vp()), Some(Action::Hints));
    }
```

Add to `crates/wwt-ui/src/chrome.rs`'s test module:

```rust
    #[test]
    fn a_notice_is_not_dressed_up_as_an_error() {
        let line = statusline(
            &Mode::Normal,
            &State::Notice("no hints".to_string()),
            "https://example.com",
            "",
            0.0,
            60,
        );
        assert!(line.starts_with("[no hints]"), "line was {line:?}");
        assert!(!line.contains("error"), "line was {line:?}");
    }
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test --workspace`
Expected: compilation fails, `no variant named Hints` and `no variant named Notice`.

- [ ] **Step 3: Add the notice state**

In `crates/wwt-ui/src/chrome.rs`, add to `enum State`:

```rust
    /// Something worth saying that is not a failure: no hints on this page,
    /// the mouse turned off. Cleared by the next successful extraction.
    Notice(String),
```

And to the tag match in `statusline`:

```rust
        State::Notice(message) => format!("[{message}] "),
```

- [ ] **Step 4: Bind the key**

In `crates/wwt/src/keymap.rs`:

```rust
    /// Label every interactive box and filter them as you type.
    Hints,
```

```rust
        KeyCode::Char('f') => Some(Action::Hints),
```

Add `Action::Hints` to `run_action`'s "handled by the caller" arm in `core.rs` alongside `Insert`.

- [ ] **Step 5: Cache the targets, and drop them when the page moves**

In `crates/wwt/src/core.rs`, add the field:

```rust
    /// The last hint query's targets, held so that pressing `f` twice on a
    /// page that has not moved costs one round trip rather than two.
    hints: Option<Vec<HintTarget>>,
```

initialised to `None`, and add the helper:

```rust
    /// Note that the page has changed under us.
    ///
    /// Hint targets are geometry, so a page that moved has invalidated them.
    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.hints = None;
    }
```

Replace every `self.dirty = true;` in `core.rs` with `self.mark_dirty();`. There are three: the `Runtime.bindingCalled` arm in `run`, the `Job::Settled` arm in `on_job`, and the tail of `on_resize`. Leave `self.dirty = false;` in `start_extract` alone.

- [ ] **Step 6: Query, enter, filter, activate**

Add `Job::Hints(Vec<HintTarget>)` to the `Job` enum:

```rust
    /// The page reported its interactive boxes.
    Hints(Vec<HintTarget>),
```

Add the `Mode::Normal` arm beside `Insert`:

```rust
                Some(Action::Hints) => {
                    match self.hints.clone() {
                        Some(targets) => self.enter_hints(targets),
                        None => self.start_hints(),
                    }
                    false
                }
```

Replace the `Mode::Hint(_) => false` stub with:

```rust
            Mode::Hint(session) => {
                let mut session = session.clone();
                match key.code {
                    KeyCode::Esc => self.mode = Mode::Normal,
                    KeyCode::Backspace => {
                        let filtered = session.pop();
                        self.on_filtered(session, filtered);
                    }
                    KeyCode::Char(c) => {
                        let filtered = session.push(c);
                        self.on_filtered(session, filtered);
                    }
                    _ => {}
                }
                false
            }
```

Add the four methods to `impl Core`:

```rust
    fn start_hints(&mut self) {
        let page = Arc::clone(&self.page);
        let tx = self.jobs_tx.clone();
        let errors = self.errors_tx.clone();
        tokio::spawn(async move {
            match page.hints().await {
                Ok(targets) => {
                    let _ = tx.send(Job::Hints(targets));
                }
                // Not a `Job::Failed`: that one clears the extraction and
                // navigation flags, and a failed hint query has not
                // finished either of those.
                Err(error) => {
                    let _ = errors.send(error.to_string());
                }
            }
        });
    }

    fn enter_hints(&mut self, targets: Vec<HintTarget>) {
        let session = HintSession::new(targets);
        if session.is_empty() {
            // Entering a mode with nothing in it would only need escaping.
            self.state = State::Notice("no hints".to_string());
            return;
        }
        self.mode = Mode::Hint(session);
    }

    /// Apply what filtering decided about the character just typed.
    fn on_filtered(&mut self, session: HintSession, filtered: Filtered) {
        match filtered {
            Filtered::Waiting(_) => self.mode = Mode::Hint(session),
            Filtered::Activate(target) => self.activate(target),
            // Nothing matches, so there is nothing left to type. Leaving is
            // friendlier than sitting there waiting for an Esc.
            Filtered::None => self.mode = Mode::Normal,
        }
    }

    fn activate(&mut self, target: HintTarget) {
        let at = target.center();
        self.input.send(Input::Mouse(MouseInput::press(at)));
        self.input.send(Input::Mouse(MouseInput::release(at)));
        // Clicking a text field is the beginning of typing into it, so that
        // is where the mode goes. Anything else is finished when the click
        // lands.
        self.mode = match target.kind {
            TargetKind::Editable => Mode::Insert,
            TargetKind::Clickable => Mode::Normal,
        };
    }
```

Add the `on_job` arm:

```rust
            Job::Hints(targets) => {
                self.hints = Some(targets.clone());
                self.enter_hints(targets);
            }
```

Extend the imports:

```rust
use wwt_frame::{CellSize, Frame, GridSize, HintTarget, TargetKind, TextRun, Viewport};
use wwt_page::{DIRTY_BINDING, Extraction, MouseInput, Page};
use wwt_ui::hint::{Filtered, HintSession};
```

- [ ] **Step 7: Paint the labels**

In `Core::compose`, between the runs and the chrome:

```rust
        // After the page and before the chrome: labels cover the text they
        // point at, which is what makes them readable, and the chrome still
        // owns its row.
        if let Mode::Hint(session) = &self.mode {
            session.paint(&mut frame, &self.vp);
        }
```

- [ ] **Step 8: Run the tests**

Run: `cargo test --workspace`
Expected: PASS, 143 tests.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 9: Try it**

```bash
cargo run -p wwt -- news.ycombinator.com
```

Press `f`: labels appear over every link. Type one label's characters and watch the story open. Press `f` then `Esc` and check the labels disappear leaving the page as it was. Press `f` on a page with a search box, hint the box, and check the statusline says `-- INSERT --`.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat(wwt): reach every link from the keyboard with f

Targets are queried when f is pressed and cached until the page next
says it changed. Filtering is local, so narrowing the set costs no round
trip."
```

---

### Task 12: The mouse

**Files:**
- Modify: `crates/wwt-ui/src/command.rs`, `crates/wwt/src/core.rs`, `crates/wwt/src/main.rs`

**Interfaces:**
- Consumes: `wwt_page::MouseInput`, `Viewport::to_css` (which already returns cell centres).
- Produces: `command::{Command::Set, Setting::Mouse}`; `wwt::core::page_cell(vp: &Viewport, column: u16, row: u16) -> Option<CellPos>`; `Core::on_mouse`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/wwt-ui/src/command.rs`'s test module:

```rust
    #[test]
    fn set_mouse_takes_on_and_off() {
        assert_eq!(parse("set mouse on"), Ok(Command::Set(Setting::Mouse(true))));
        assert_eq!(parse("set mouse off"), Ok(Command::Set(Setting::Mouse(false))));
    }

    #[test]
    fn a_setting_that_does_not_exist_names_itself() {
        assert_eq!(parse("set zoom 2"), Err("unknown setting: zoom".to_string()));
        assert!(parse("set mouse maybe").is_err());
    }
```

Add to `crates/wwt/src/core.rs`'s test module:

```rust
    #[test]
    fn a_click_on_the_page_keeps_its_cell() {
        let vp = page_viewport(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
        assert_eq!(page_cell(&vp, 5, 7), Some(CellPos { col: 5, row: 7 }));
    }

    #[test]
    fn a_click_on_the_chrome_row_belongs_to_no_page_cell() {
        // Row 23 is the statusline. The page does not know that row exists,
        // so there is nothing to convert a click there into.
        let vp = page_viewport(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
        assert_eq!(page_cell(&vp, 5, 23), None);
    }
```

Add `use wwt_frame::CellPos;` to `core.rs`'s imports.

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test --workspace`
Expected: compilation fails, `cannot find function page_cell` and `cannot find type Setting`.

- [ ] **Step 3: Parse `:set mouse`**

In `crates/wwt-ui/src/command.rs`, add the setting type and the variant:

```rust
/// Something you can turn on or off from the `:` line.
#[derive(Debug, Clone, PartialEq)]
pub enum Setting {
    /// Terminal mouse capture. Off hands text selection back to terminals
    /// that do not give it to you with shift held.
    Mouse(bool),
}
```

```rust
    Set(Setting),
```

And the arm in `parse`, after `"reload"`:

```rust
        "set" => {
            let (setting, value) = match rest.split_once(char::is_whitespace) {
                Some((setting, value)) => (setting, value.trim()),
                None => (rest, ""),
            };
            match (setting, value) {
                ("mouse", "on") => Ok(Command::Set(Setting::Mouse(true))),
                ("mouse", "off") => Ok(Command::Set(Setting::Mouse(false))),
                ("mouse", other) => Err(format!("set mouse takes on or off, not {other:?}")),
                (other, _) => Err(format!("unknown setting: {other}")),
            }
        }
```

- [ ] **Step 4: Convert a terminal cell to a page point**

In `crates/wwt/src/core.rs`, beside `page_viewport`:

```rust
/// The page cell a terminal cell refers to, or `None` when it is one of ours.
///
/// The last row is chrome. The page does not know it exists, so a click there
/// has no page coordinate to become.
pub fn page_cell(vp: &Viewport, column: u16, row: u16) -> Option<CellPos> {
    let grid = vp.grid();
    (column < grid.cols && row < grid.rows).then_some(CellPos { col: column, row })
}
```

- [ ] **Step 5: Dispatch mouse events**

Add the constant near `RESIZE_DEBOUNCE`:

```rust
/// How far one notch of the wheel scrolls, in rows. Three is what a desktop
/// browser does, and matching it is what makes the page feel normal.
const WHEEL_ROWS: f64 = 3.0;
```

Add the field to `Core`, initialised to `None`:

```rust
    /// A mouse capture change waiting for the next write to the terminal,
    /// because that is where we have something to write to.
    mouse_pending: Option<bool>,
```

Add the method:

```rust
    fn on_mouse(&mut self, event: MouseEvent) {
        let Some(cell) = page_cell(&self.vp, event.column, event.row) else {
            return;
        };
        // `to_css` returns the cell's centre, so the click lands
        // unambiguously inside the cell you pointed at.
        let at = self.vp.to_css(cell);
        let notch = WHEEL_ROWS * f64::from(self.cell.h);

        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.input.send(Input::Mouse(MouseInput::press(at)));
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.input.send(Input::Mouse(MouseInput::release(at)));
            }
            MouseEventKind::ScrollDown => {
                self.input.send(Input::Mouse(MouseInput::wheel(at, notch)));
            }
            MouseEventKind::ScrollUp => {
                self.input.send(Input::Mouse(MouseInput::wheel(at, -notch)));
            }
            // Motion would cost a round trip per reported frame, and there is
            // no context menu to open and no tab to middle-click into.
            _ => {}
        }
    }
```

Add the arm to the terminal event match in `Core::run`, beside the key and resize arms:

```rust
                        TermEvent::Mouse(mouse) => self.on_mouse(mouse),
```

Handle the command in `run_command`:

```rust
            Command::Set(Setting::Mouse(on)) => {
                self.mouse_pending = Some(on);
                self.state = State::Notice(
                    if on { "mouse on" } else { "mouse off" }.to_string(),
                );
            }
```

Apply it at the top of `present`, which is the one place that holds the terminal:

```rust
    fn present(&mut self, out: &mut impl Write) -> Result<()> {
        if let Some(on) = self.mouse_pending.take() {
            if on {
                execute!(out, EnableMouseCapture).context("enable mouse capture")?;
            } else {
                execute!(out, DisableMouseCapture).context("disable mouse capture")?;
            }
        }
```

Extend the imports:

```rust
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event as TermEvent, EventStream, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use wwt_ui::command::{self, Command, Setting};
```

- [ ] **Step 6: Turn capture on at startup**

In `crates/wwt/src/main.rs`, add the import:

```rust
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
```

Enable capture in its own `execute!`, because a terminal that refuses it is
still a terminal you can read with. Bundling it with the alternate screen
would make one refusal cost the whole session.

```rust
    execute!(stdout(), EnterAlternateScreen, cursor::Hide)?;
    let mouse = execute!(stdout(), EnableMouseCapture).is_ok();

    let mut core = Core::new(page, client, grid, cell);
    if !mouse {
        core.notice("mouse unavailable");
    }
```

Release it on the way out, where a capture that was never enabled makes this
a harmless no-op:

```rust
    execute!(stdout(), cursor::Show, DisableMouseCapture, LeaveAlternateScreen)?;
```

And add the one method that needs, in `crates/wwt/src/core.rs`:

```rust
    /// Say something in the statusline before the loop starts.
    pub fn notice(&mut self, message: &str) {
        self.state = State::Notice(message.to_string());
    }
```

- [ ] **Step 7: Run the tests**

Run: `cargo test --workspace`
Expected: PASS, 147 tests.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 8: Try it**

```bash
cargo run -p wwt -- news.ycombinator.com
```

Click a link and watch it open. Scroll with the wheel. Open a page with a nested scrolling panel and check the wheel moves the panel under the pointer rather than the document. Click the statusline and check nothing happens. Type `:set mouse off`, then check the terminal's own selection works again and the wheel no longer reaches the page.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(wwt): dispatch clicks and wheels from the terminal mouse

The wheel aims at the pointer, so a nested scroller under the cursor
moves instead of the document. The chrome row is swallowed: the page
does not know that row exists. :set mouse off hands selection back to
terminals that do not offer it with shift held."
```

---

### Task 13: Documentation and the manual pass

The tests prove the pieces. This proves the browser.

**Files:**
- Modify: `README.md`, `CLAUDE.md`
- Modify: `docs/superpowers/specs/2026-08-19-wwt-m3-design.md` (only if implementation forced a deviation)

- [ ] **Step 1: Update `CLAUDE.md`**

Change "Currently at **M2** (navigation and reading)" to **M3** (interaction), update the test count in the commands block from 102 to 147, and add `wwt-ui` to the crate table:

```markdown
| `wwt-ui` | Modes, chrome, `:` commands, hint labels | Depends on `wwt-frame` only. No pages, no CDP, no terminal. |
```

Add a short section after "The injected script":

```markdown
## Input

Three rules carry M3:

- **`Esc` is never forwarded.** A page cannot trap the keyboard. `Ctrl-]`
  sends the page a literal Escape, because a terminal transmits `Ctrl-[` as
  0x1B, which *is* Escape.
- **Mode changes only in response to a keystroke.** No `focusin` listener, no
  page-driven mode. `i` hands the keyboard over, `Esc` takes it back.
- **Input is ordered.** Keys and clicks go through one pump task
  (`wwt/src/input.rs`), not one spawned task each, or `abc` would sometimes
  arrive as `acb`. Everything else about the loop is unchanged: nothing
  blocks it.

Hint targets come from `__wwt.hints()`, queried on `f` and cached until the
next dirty signal. They are deliberately not part of extraction: that path
runs on every scroll frame.
```

- [ ] **Step 2: Update `README.md`**

Add the M3 keys to whatever key table it holds: `i` insert, `Esc` back to normal, `Ctrl-]` a literal Escape for the page, `f` hints, and `:set mouse on|off`. Say that the mouse is on by default and what it costs.

- [ ] **Step 3: Walk through it by hand**

```bash
cargo run -p wwt -- duckduckgo.com
```

Confirm, in order:

1. `i`, type a search, press `Enter`. The results load. The statusline showed `-- INSERT --` while you typed.
2. `Esc`, then `j` and `k` scroll rather than typing letters.
3. `f`. Labels appear over every result link, and only over things you can actually click: nothing labelled is underneath something else.
4. Type a label. The page opens. You are back in normal mode.
5. `f`, then `Esc`. The labels vanish and the page underneath is intact, not smeared.
6. `f`, then a character that labels nothing. Hint mode ends rather than hanging.
7. `H` goes back. `f` again: the labels describe *this* page, not the one you left.
8. On a page with a search box, `f` the box. The statusline says `-- INSERT --` without your pressing `i`.
9. `Ctrl-]` inside a site's own modal dismisses it, and does **not** leave insert mode.
10. Click a link with the mouse. It opens. Click the statusline: nothing happens.
11. Wheel over a page with a nested scrolling panel: the panel moves, not the document.
12. `:set mouse off`, then check your terminal's own text selection works again.
13. `q` exits and the terminal is exactly as it was: no alternate screen, cursor visible, no stray colours, and the mouse is no longer captured.

Any failure here is a bug to fix before the task is done, not a note to file.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "docs: describe M3's input model"
```

---

## Definition of done for M3

- `cargo test --workspace` is green with 147 tests: 26 `wwt-frame`, 18 `wwt-term`, 8 `wwt-cdp`, 32 `wwt-page`, 34 `wwt-ui`, 29 `wwt`. The number is a checksum, not a target: if you added a test deliberately, say so and move on.
- `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- The hint query measurement from Task 8 is recorded in that task's commit message.
- No new dependencies. `git diff main -- Cargo.toml crates/*/Cargo.toml` shows the new `wwt-ui` entries and nothing else.
- Every item in Task 13's manual checklist behaves as described, including the three that matter most: `Esc` always returns the keyboard, a hint label never points at something covered, and `q` restores the terminal with the mouse released.

## Known M3 limitations (deliberate, do not "fix")

- No IME and no multi-byte composition. A composed character types correctly, but there is no composition state and no candidate window.
- The key table is a US layout. The character you type is right on any layout; `e.code` names the physical key a US keyboard would have used.
- Occlusion is one hit test at the box centre. A target whose centre is covered but whose edges are not will be dropped.
- No drag, no hover, no context menu, no middle click.
- One page. `f` never opens a new tab, because there are no tabs until M4.
- No text selection inside the page. M1's limit, unchanged.
- The Chromium process is still unsupervised. M7.
