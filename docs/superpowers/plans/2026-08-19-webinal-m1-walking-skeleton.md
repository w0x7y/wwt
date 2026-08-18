# Webinal M1 — Walking Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `webinal <url>` launches headless Chromium, sizes its viewport to the terminal grid, extracts every text run's layout box, paints them into the cell grid, and quits on `q`.

**Architecture:** A Cargo workspace. `wb-frame` owns all coordinate math and the cell grid with zero I/O, so the subtle logic is unit-testable without a browser. `wb-cdp` is a hand-rolled CDP client over a websocket. `wb-page` injects an extraction script and parses its output into `TextRun`s. `wb-term` probes cell size and writes a `Frame` to a sink. The `webinal` binary wires them together behind one testable `render_url` function.

**Tech Stack:** Rust 2024 edition, tokio, tokio-tungstenite, serde/serde_json, crossterm, rustix, anyhow. Chromium 151 as an external process.

**Spec:** `docs/superpowers/specs/2026-08-19-webinal-design.md` — read spec sections 3, 4, and 5 before starting. This plan implements milestone M1 only.

## Global Constraints

- Rust edition **2024**, toolchain **1.97+**.
- Dependency versions, exact, used in every crate that needs them:
  `tokio = "1.53"`, `tokio-tungstenite = "0.30"`, `futures-util = "0.3"`,
  `serde = "1.0"` (feature `derive`), `serde_json = "1.0"`, `crossterm = "0.29"`,
  `rustix = "1.1"` (feature `termios`), `anyhow = "1.0"`, `thiserror = "2.0"`,
  `tempfile = "3"` (dev-dependency).
- `wb-frame` has **no I/O and no dependencies**. If a task tempts you to add one, the design is wrong — stop and ask.
- Chromium is located via the `WEBINAL_CHROMIUM` environment variable, falling back to the first of `chromium`, `chromium-browser`, `google-chrome-stable` found on `PATH`. Never download anything.
- All coordinate conversions go through `Viewport`. No task may open-code a division by cell width or height.
- Every task ends with a passing `cargo test --workspace` and a commit.

## Known M1 limitations (deliberate, do not "fix")

These are scoped to later milestones. Recording them so an implementer does not gold-plate:

- **Full repaint, no diffing.** The diffing renderer is M2.
- **Polling for page load,** not `Page.loadEventFired`. CDP event plumbing is M2.
- **Per-character rect measurement** in the extraction script — O(n) ranges per text node, correct but slow. Binary-search optimization is M2.
- **Runs wider than their box are truncated with an ellipsis.** Because browser glyphs average narrower than a terminal cell, this truncates roughly 8% of a full-width line. Tuning `cell_css` to compensate needs real pages to look at; it is M2 work.
- **No hints, no input to the page, no tabs, no scrolling.** M3 and M4.

---

### Task 1: Workspace and the coordinate model

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/wb-frame/Cargo.toml`
- Create: `crates/wb-frame/src/lib.rs`
- Create: `crates/wb-frame/src/geom.rs`
- Test: `crates/wb-frame/src/geom.rs` (inline `#[cfg(test)]` module)

**Interfaces:**
- Consumes: nothing.
- Produces: `wb_frame::geom::{CellSize, GridSize, CellPos, CssPoint, CssRect, Viewport}`.
  `Viewport::new(grid: GridSize, cell: CellSize) -> Viewport`,
  `Viewport::css_width(&self) -> u32`, `Viewport::css_height(&self) -> u32`,
  `Viewport::to_cell(&self, p: CssPoint) -> Option<CellPos>`,
  `Viewport::to_css(&self, c: CellPos) -> CssPoint`.

- [ ] **Step 1: Create the workspace root**

Create `Cargo.toml`:

```toml
[workspace]
resolver = "3"
members = ["crates/wb-frame"]

[workspace.package]
edition = "2024"
version = "0.1.0"

[workspace.dependencies]
tokio = { version = "1.53", features = ["rt-multi-thread", "macros", "process", "io-util", "sync", "time"] }
tokio-tungstenite = "0.30"
futures-util = "0.3"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
crossterm = "0.29"
rustix = { version = "1.1", features = ["termios"] }
anyhow = "1.0"
thiserror = "2.0"
tempfile = "3"
```

Create `crates/wb-frame/Cargo.toml`:

```toml
[package]
name = "wb-frame"
edition.workspace = true
version.workspace = true

[dependencies]
```

Create `crates/wb-frame/src/lib.rs`:

```rust
pub mod geom;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/wb-frame/src/geom.rs` containing only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn vp(cols: u16, rows: u16, w: u16, h: u16) -> Viewport {
        Viewport::new(GridSize { cols, rows }, CellSize { w, h })
    }

    #[test]
    fn viewport_css_size_is_grid_times_cell() {
        let v = vp(180, 48, 9, 20);
        assert_eq!(v.css_width(), 1620);
        assert_eq!(v.css_height(), 960);
    }

    #[test]
    fn cell_to_css_returns_cell_center() {
        let v = vp(180, 48, 9, 20);
        let p = v.to_css(CellPos { col: 0, row: 0 });
        assert_eq!(p.x, 4.5);
        assert_eq!(p.y, 10.0);

        let p = v.to_css(CellPos { col: 2, row: 3 });
        assert_eq!(p.x, 22.5);
        assert_eq!(p.y, 70.0);
    }

    #[test]
    fn css_to_cell_floors_into_the_grid() {
        let v = vp(180, 48, 9, 20);
        assert_eq!(
            v.to_cell(CssPoint { x: 0.0, y: 0.0 }),
            Some(CellPos { col: 0, row: 0 })
        );
        assert_eq!(
            v.to_cell(CssPoint { x: 8.99, y: 19.99 }),
            Some(CellPos { col: 0, row: 0 })
        );
        assert_eq!(
            v.to_cell(CssPoint { x: 9.0, y: 20.0 }),
            Some(CellPos { col: 1, row: 1 })
        );
    }

    #[test]
    fn css_to_cell_rejects_points_outside_the_viewport() {
        let v = vp(10, 4, 9, 20);
        assert_eq!(v.to_cell(CssPoint { x: -0.1, y: 0.0 }), None);
        assert_eq!(v.to_cell(CssPoint { x: 0.0, y: -0.1 }), None);
        assert_eq!(v.to_cell(CssPoint { x: 90.0, y: 0.0 }), None);
        assert_eq!(v.to_cell(CssPoint { x: 0.0, y: 80.0 }), None);
    }

    /// The load-bearing property from spec section 3: converting a cell to CSS
    /// and back is the identity, at every zoom level, for every cell in the grid.
    #[test]
    fn cell_css_cell_roundtrip_is_identity() {
        for (w, h) in [(8u16, 16u16), (9, 20), (12, 26), (7, 15), (1, 1)] {
            let v = vp(180, 48, w, h);
            for row in 0..v.grid().rows {
                for col in 0..v.grid().cols {
                    let c = CellPos { col, row };
                    assert_eq!(
                        v.to_cell(v.to_css(c)),
                        Some(c),
                        "roundtrip failed at cell {c:?} with cell size {w}x{h}"
                    );
                }
            }
        }
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p wb-frame`
Expected: FAIL to compile — `cannot find type Viewport in this scope` and similar.

- [ ] **Step 4: Write the implementation**

Prepend to `crates/wb-frame/src/geom.rs`, above the test module:

```rust
//! The coordinate model. See spec section 3.
//!
//! One coordinate space, two units: terminal cells and CSS pixels. Every
//! conversion in the system goes through `Viewport`; nothing else divides by
//! a cell dimension.

/// The size of one terminal cell, measured in CSS pixels.
///
/// This is the zoom control. Declaring a cell to be larger shrinks the CSS
/// viewport, so the page genuinely reflows and hits different breakpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellSize {
    pub w: u16,
    pub h: u16,
}

/// The terminal grid, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
}

/// A position in the terminal grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellPos {
    pub col: u16,
    pub row: u16,
}

/// A point in CSS pixels, in the page's viewport coordinate space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CssPoint {
    pub x: f64,
    pub y: f64,
}

/// A rectangle in CSS pixels, as reported by `getBoundingClientRect`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CssRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl CssRect {
    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
}

/// Binds the terminal grid to the CSS viewport we ask Chromium to lay out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    grid: GridSize,
    cell: CellSize,
}

impl Viewport {
    pub fn new(grid: GridSize, cell: CellSize) -> Self {
        assert!(cell.w > 0 && cell.h > 0, "cell size must be non-zero");
        Self { grid, cell }
    }

    pub fn grid(&self) -> GridSize {
        self.grid
    }

    pub fn cell(&self) -> CellSize {
        self.cell
    }

    /// The viewport width in CSS pixels — what Chromium is told the window is.
    pub fn css_width(&self) -> u32 {
        u32::from(self.grid.cols) * u32::from(self.cell.w)
    }

    /// The viewport height in CSS pixels.
    pub fn css_height(&self) -> u32 {
        u32::from(self.grid.rows) * u32::from(self.cell.h)
    }

    /// The CSS point at the *center* of a cell. Center rather than corner so
    /// that dispatching a click at this point lands unambiguously inside the
    /// cell, and so the roundtrip below is exact.
    pub fn to_css(&self, c: CellPos) -> CssPoint {
        CssPoint {
            x: (f64::from(c.col) + 0.5) * f64::from(self.cell.w),
            y: (f64::from(c.row) + 0.5) * f64::from(self.cell.h),
        }
    }

    /// The cell containing a CSS point, or `None` if it falls outside the grid.
    pub fn to_cell(&self, p: CssPoint) -> Option<CellPos> {
        if p.x < 0.0 || p.y < 0.0 {
            return None;
        }
        let col = (p.x / f64::from(self.cell.w)) as u32;
        let row = (p.y / f64::from(self.cell.h)) as u32;
        if col >= u32::from(self.grid.cols) || row >= u32::from(self.grid.rows) {
            return None;
        }
        Some(CellPos {
            col: col as u16,
            row: row as u16,
        })
    }

    /// The column a CSS x-coordinate falls in, unclamped by the grid's right
    /// edge. Painting uses this so a run starting off-screen still places its
    /// visible tail correctly.
    pub fn col_of(&self, x: f64) -> i64 {
        (x / f64::from(self.cell.w)).floor() as i64
    }

    /// The row a CSS y-coordinate falls in, unclamped.
    pub fn row_of(&self, y: f64) -> i64 {
        (y / f64::from(self.cell.h)).floor() as i64
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p wb-frame`
Expected: PASS, 5 tests.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/wb-frame
git commit -m "feat(frame): add the cell/CSS coordinate model"
```

---

### Task 2: Cells, the frame grid, and painting a text run

**Files:**
- Create: `crates/wb-frame/src/cell.rs`
- Create: `crates/wb-frame/src/run.rs`
- Create: `crates/wb-frame/src/frame.rs`
- Modify: `crates/wb-frame/src/lib.rs`
- Test: inline `#[cfg(test)]` module in `crates/wb-frame/src/frame.rs`

**Interfaces:**
- Consumes: `Viewport`, `GridSize`, `CellPos`, `CssRect` from Task 1.
- Produces: `wb_frame::cell::{Rgb, Style, Cell}`, `wb_frame::run::TextRun`,
  `wb_frame::frame::Frame` with `Frame::new(GridSize) -> Frame`,
  `Frame::grid(&self) -> GridSize`, `Frame::cell(&self, CellPos) -> Option<&Cell>`,
  `Frame::paint_run(&mut self, &Viewport, &TextRun)`,
  `Frame::row_text(&self, u16) -> String`.

- [ ] **Step 1: Write the failing tests**

Create `crates/wb-frame/src/frame.rs` containing only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{Rgb, Style};
    use crate::geom::{CellPos, CellSize, CssRect, GridSize, Viewport};
    use crate::run::TextRun;

    fn vp() -> Viewport {
        Viewport::new(GridSize { cols: 20, rows: 5 }, CellSize { w: 10, h: 20 })
    }

    fn run(text: &str, x: f64, baseline: f64, w: f64) -> TextRun {
        TextRun {
            text: text.to_string(),
            rect: CssRect { x, y: baseline - 14.0, w, h: 16.0 },
            baseline,
            style: Style::default(),
            z: 0,
        }
    }

    #[test]
    fn new_frame_is_all_blanks() {
        let f = Frame::new(GridSize { cols: 20, rows: 5 });
        assert_eq!(f.row_text(0), "");
        assert_eq!(f.cell(CellPos { col: 3, row: 2 }).unwrap().ch, ' ');
    }

    #[test]
    fn paint_places_text_at_the_box_origin() {
        let mut f = Frame::new(vp().grid());
        f.paint_run(&vp(), &run("hello", 30.0, 14.0, 50.0));
        // x=30 with a 10px cell starts at column 3; baseline 14 is in row 0.
        assert_eq!(f.row_text(0), "   hello");
    }

    #[test]
    fn paint_snaps_the_row_by_baseline_not_box_top() {
        let mut f = Frame::new(vp().grid());
        // Box top is 26 (row 1) but the baseline is 41, which is in row 2.
        // Spec section 3: snap by baseline, so this must land in row 2.
        let mut r = run("x", 0.0, 41.0, 10.0);
        r.rect.y = 26.0;
        f.paint_run(&vp(), &r);
        assert_eq!(f.row_text(1), "");
        assert_eq!(f.row_text(2), "x");
    }

    #[test]
    fn paint_elides_runs_wider_than_their_box() {
        let mut f = Frame::new(vp().grid());
        // A 30px-wide box is 3 cells, but the text is 8 characters.
        f.paint_run(&vp(), &run("abcdefgh", 0.0, 14.0, 30.0));
        assert_eq!(f.row_text(0), "ab…");
    }

    #[test]
    fn paint_clips_at_the_right_edge_of_the_grid() {
        let mut f = Frame::new(vp().grid());
        // Starts at column 18 of a 20-column grid with a box wide enough for
        // all of it; only two cells are actually available.
        f.paint_run(&vp(), &run("abcdef", 180.0, 14.0, 60.0));
        assert_eq!(f.row_text(0), "                  a…");
    }

    #[test]
    fn paint_ignores_runs_outside_the_viewport() {
        let mut f = Frame::new(vp().grid());
        f.paint_run(&vp(), &run("above", 0.0, -5.0, 50.0));
        f.paint_run(&vp(), &run("below", 0.0, 500.0, 50.0));
        f.paint_run(&vp(), &run("right", 400.0, 14.0, 50.0));
        for row in 0..5 {
            assert_eq!(f.row_text(row), "", "row {row} should be empty");
        }
    }

    #[test]
    fn paint_places_the_visible_tail_of_a_run_starting_off_screen() {
        let mut f = Frame::new(vp().grid());
        // Starts at x = -20, i.e. two cells left of the grid.
        f.paint_run(&vp(), &run("abcdef", -20.0, 14.0, 60.0));
        assert_eq!(f.row_text(0), "cdef");
    }

    #[test]
    fn paint_carries_style_onto_the_cells() {
        let mut f = Frame::new(vp().grid());
        let mut r = run("hi", 0.0, 14.0, 20.0);
        r.style = Style { fg: Rgb { r: 255, g: 0, b: 0 }, bold: true };
        f.paint_run(&vp(), &r);
        let c = f.cell(CellPos { col: 0, row: 0 }).unwrap();
        assert_eq!(c.ch, 'h');
        assert_eq!(c.style.fg, Rgb { r: 255, g: 0, b: 0 });
        assert!(c.style.bold);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p wb-frame`
Expected: FAIL to compile — `Frame`, `TextRun`, `Style` not found.

- [ ] **Step 3: Write the cell and run types**

Create `crates/wb-frame/src/cell.rs`:

```rust
/// A 24-bit color. Terminals that cannot do truecolor are M5's problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub fg: Rgb,
    pub bold: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fg: Rgb { r: 0xd0, g: 0xd0, b: 0xd0 },
            bold: false,
        }
    }
}

/// One terminal cell.
///
/// `z` is the stacking depth of whatever painted this cell, retained so a
/// later run can decide whether it is allowed to overwrite it. See Task 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
    pub z: i32,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            style: Style::default(),
            z: i32::MIN,
        }
    }
}
```

Create `crates/wb-frame/src/run.rs`:

```rust
use crate::cell::Style;
use crate::geom::CssRect;

/// One horizontal run of text on a single line, as measured by the browser.
///
/// A text node that wraps across three lines yields three `TextRun`s. The
/// extraction script guarantees a run never spans lines, which is what lets
/// painting treat it as a single horizontal span of cells.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub text: String,
    /// The run's client rect, in CSS pixels.
    pub rect: CssRect,
    /// CSS y of the text baseline. Painting snaps rows by this, not by
    /// `rect.y` — see spec section 3.
    pub baseline: f64,
    pub style: Style,
    /// Stacking depth. Higher wins a contested cell.
    pub z: i32,
}
```

Update `crates/wb-frame/src/lib.rs`:

```rust
pub mod cell;
pub mod frame;
pub mod geom;
pub mod run;

pub use cell::{Cell, Rgb, Style};
pub use frame::Frame;
pub use geom::{CellPos, CellSize, CssPoint, CssRect, GridSize, Viewport};
pub use run::TextRun;
```

- [ ] **Step 4: Write the frame implementation**

Prepend to `crates/wb-frame/src/frame.rs`, above the test module:

```rust
use crate::cell::Cell;
use crate::geom::{CellPos, GridSize, Viewport};
use crate::run::TextRun;

/// Shrinks a box's right edge before it is mapped to a column, so an edge
/// sitting exactly on a cell boundary belongs to the cell on its left.
const EDGE_EPSILON: f64 = 1e-6;

/// A rendered page as a grid of styled cells.
///
/// Every rendering mode produces one of these and the terminal renderer
/// consumes it, so text mode, pixel mode, and reader mode never diverge in
/// how they reach the screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    grid: GridSize,
    cells: Vec<Cell>,
}

impl Frame {
    pub fn new(grid: GridSize) -> Self {
        let len = usize::from(grid.cols) * usize::from(grid.rows);
        Self {
            grid,
            cells: vec![Cell::default(); len],
        }
    }

    pub fn grid(&self) -> GridSize {
        self.grid
    }

    fn index(&self, pos: CellPos) -> Option<usize> {
        if pos.col >= self.grid.cols || pos.row >= self.grid.rows {
            return None;
        }
        Some(usize::from(pos.row) * usize::from(self.grid.cols) + usize::from(pos.col))
    }

    pub fn cell(&self, pos: CellPos) -> Option<&Cell> {
        self.index(pos).map(|i| &self.cells[i])
    }

    /// Paint one text run into the grid.
    ///
    /// The run occupies the row containing its baseline, starting at the
    /// column containing its left edge. It may use at most as many cells as
    /// its box covers; text that does not fit is elided with an ellipsis.
    pub fn paint_run(&mut self, vp: &Viewport, run: &TextRun) {
        let row = vp.row_of(run.baseline);
        if row < 0 || row >= i64::from(self.grid.rows) {
            return;
        }
        let row = row as u16;

        let start_col = vp.col_of(run.rect.x);
        // Nudge off the right edge before asking which column it falls in: a
        // box ending exactly on a cell boundary does not occupy that cell,
        // and without this every run claims one column too many.
        let last_col = vp.col_of(run.rect.right() - EDGE_EPSILON);
        let box_cols = last_col - start_col + 1;
        if box_cols <= 0 {
            return;
        }

        let chars: Vec<char> = run.text.chars().collect();
        if chars.is_empty() {
            return;
        }

        // Drop the leading characters that fall left of the grid, so a run
        // scrolled partly off-screen still shows its visible tail in place.
        let skip = if start_col < 0 { (-start_col) as usize } else { 0 };
        if skip >= chars.len() {
            return;
        }
        let first_col = (start_col + skip as i64) as u16;
        if first_col >= self.grid.cols {
            return;
        }

        // Cells actually available: limited by the run's own box and by the
        // right edge of the grid.
        let box_budget = (box_cols as usize).saturating_sub(skip);
        let grid_budget = usize::from(self.grid.cols - first_col);
        let budget = box_budget.min(grid_budget);
        if budget == 0 {
            return;
        }

        let visible = &chars[skip..];
        let mut out: Vec<char> = Vec::with_capacity(budget);
        if visible.len() <= budget {
            out.extend_from_slice(visible);
        } else if budget == 1 {
            out.push('…');
        } else {
            out.extend_from_slice(&visible[..budget - 1]);
            out.push('…');
        }

        for (i, ch) in out.into_iter().enumerate() {
            let pos = CellPos {
                col: first_col + i as u16,
                row,
            };
            let Some(idx) = self.index(pos) else { break };
            self.cells[idx] = Cell {
                ch,
                style: run.style,
                z: run.z,
            };
        }
    }

    /// The text of one row with trailing blanks removed. Test helper — this is
    /// what makes golden snapshots readable as ASCII art.
    pub fn row_text(&self, row: u16) -> String {
        if row >= self.grid.rows {
            return String::new();
        }
        let start = usize::from(row) * usize::from(self.grid.cols);
        let end = start + usize::from(self.grid.cols);
        let s: String = self.cells[start..end].iter().map(|c| c.ch).collect();
        s.trim_end().to_string()
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p wb-frame`
Expected: PASS, 13 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/wb-frame
git commit -m "feat(frame): add cells, text runs, and baseline-snapped painting"
```

---

### Task 3: Painter's algorithm for contested cells

**Files:**
- Modify: `crates/wb-frame/src/frame.rs` (the `paint_run` write loop)
- Test: inline `#[cfg(test)]` module in `crates/wb-frame/src/frame.rs`

**Interfaces:**
- Consumes: `Frame::paint_run` from Task 2.
- Produces: no new signatures. `paint_run` gains the rule that a cell is only
  overwritten by a run whose `z` is greater than or equal to the `z` already
  recorded on that cell.

- [ ] **Step 1: Write the failing tests**

Append these tests inside the existing `mod tests` in `crates/wb-frame/src/frame.rs`:

```rust
    #[test]
    fn a_higher_stack_wins_a_contested_cell() {
        let mut f = Frame::new(vp().grid());
        let mut under = run("aaaa", 0.0, 14.0, 40.0);
        under.z = 0;
        let mut over = run("BB", 0.0, 14.0, 20.0);
        over.z = 5;

        f.paint_run(&vp(), &under);
        f.paint_run(&vp(), &over);
        assert_eq!(f.row_text(0), "BBaa");
    }

    #[test]
    fn a_lower_stack_does_not_overwrite() {
        let mut f = Frame::new(vp().grid());
        let mut over = run("BBBB", 0.0, 14.0, 40.0);
        over.z = 5;
        let mut under = run("aa", 0.0, 14.0, 20.0);
        under.z = 0;

        f.paint_run(&vp(), &over);
        f.paint_run(&vp(), &under);
        assert_eq!(f.row_text(0), "BBBB");
    }

    #[test]
    fn equal_stacks_resolve_in_document_order() {
        let mut f = Frame::new(vp().grid());
        // Same z: the later run wins, matching paint order within a stacking
        // context.
        f.paint_run(&vp(), &run("aaaa", 0.0, 14.0, 40.0));
        f.paint_run(&vp(), &run("BB", 0.0, 14.0, 20.0));
        assert_eq!(f.row_text(0), "BBaa");
    }

    #[test]
    fn a_blocked_run_still_paints_its_uncontested_cells() {
        let mut f = Frame::new(vp().grid());
        let mut over = run("BB", 0.0, 14.0, 20.0);
        over.z = 5;
        let mut under = run("aaaa", 0.0, 14.0, 40.0);
        under.z = 0;

        f.paint_run(&vp(), &over);
        f.paint_run(&vp(), &under);
        // The first two cells are held by the higher run; the rest are free.
        assert_eq!(f.row_text(0), "BBaa");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p wb-frame`
Expected: FAIL — `a_lower_stack_does_not_overwrite` and
`a_blocked_run_still_paints_its_uncontested_cells` fail, because painting is
currently unconditional. `assertion failed: left: "aaBB"` or similar.

- [ ] **Step 3: Add the occlusion check**

In `Frame::paint_run`, replace the write loop:

```rust
        for (i, ch) in out.into_iter().enumerate() {
            let pos = CellPos {
                col: first_col + i as u16,
                row,
            };
            let Some(idx) = self.index(pos) else { break };
            self.cells[idx] = Cell {
                ch,
                style: run.style,
                z: run.z,
            };
        }
```

with:

```rust
        for (i, ch) in out.into_iter().enumerate() {
            let pos = CellPos {
                col: first_col + i as u16,
                row,
            };
            let Some(idx) = self.index(pos) else { break };
            // Painter's algorithm: a run may take a cell only from something
            // at or below its own stacking depth. Equal depth means the later
            // run wins, which matches paint order inside a stacking context.
            if run.z < self.cells[idx].z {
                continue;
            }
            self.cells[idx] = Cell {
                ch,
                style: run.style,
                z: run.z,
            };
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p wb-frame`
Expected: PASS, 17 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/wb-frame
git commit -m "feat(frame): resolve contested cells with painter's algorithm"
```

---

### Task 4: Probing the terminal's cell size

**Files:**
- Create: `crates/wb-term/Cargo.toml`
- Create: `crates/wb-term/src/lib.rs`
- Create: `crates/wb-term/src/probe.rs`
- Modify: `Cargo.toml` (workspace members)
- Test: inline `#[cfg(test)]` module in `crates/wb-term/src/probe.rs`

**Interfaces:**
- Consumes: `wb_frame::{CellSize, GridSize}`.
- Produces: `wb_term::probe::{WinSize, DEFAULT_CELL, cell_size_from, probe}`.
  `WinSize { cols: u16, rows: u16, xpixel: u16, ypixel: u16 }`,
  `cell_size_from(ws: WinSize) -> Option<CellSize>`,
  `probe() -> anyhow::Result<(GridSize, CellSize)>`.

The syscall is separated from the arithmetic so the arithmetic is testable without a tty.

- [ ] **Step 1: Create the crate**

Create `crates/wb-term/Cargo.toml`:

```toml
[package]
name = "wb-term"
edition.workspace = true
version.workspace = true

[dependencies]
wb-frame = { path = "../wb-frame" }
anyhow.workspace = true
crossterm.workspace = true
rustix.workspace = true
```

Create `crates/wb-term/src/lib.rs`:

```rust
pub mod probe;
```

Add `"crates/wb-term"` to `members` in the workspace `Cargo.toml`.

- [ ] **Step 2: Write the failing tests**

Create `crates/wb-term/src/probe.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_size_divides_pixels_by_the_grid() {
        let ws = WinSize { cols: 180, rows: 48, xpixel: 1620, ypixel: 960 };
        assert_eq!(cell_size_from(ws), Some(CellSize { w: 9, h: 20 }));
    }

    #[test]
    fn cell_size_truncates_a_non_integral_division() {
        // A terminal whose window has a few unused pixels at the edges.
        let ws = WinSize { cols: 100, rows: 10, xpixel: 950, ypixel: 205 };
        assert_eq!(cell_size_from(ws), Some(CellSize { w: 9, h: 20 }));
    }

    #[test]
    fn cell_size_is_none_when_the_terminal_reports_no_pixels() {
        // The common case for terminals that do not implement the pixel
        // fields, and for a piped stdout.
        let ws = WinSize { cols: 180, rows: 48, xpixel: 0, ypixel: 0 };
        assert_eq!(cell_size_from(ws), None);
    }

    #[test]
    fn cell_size_is_none_when_the_grid_is_degenerate() {
        assert_eq!(
            cell_size_from(WinSize { cols: 0, rows: 48, xpixel: 1620, ypixel: 960 }),
            None
        );
        assert_eq!(
            cell_size_from(WinSize { cols: 180, rows: 0, xpixel: 1620, ypixel: 960 }),
            None
        );
    }

    #[test]
    fn cell_size_is_none_when_the_division_rounds_to_zero() {
        let ws = WinSize { cols: 180, rows: 48, xpixel: 100, ypixel: 20 };
        assert_eq!(cell_size_from(ws), None);
    }

    #[test]
    fn the_default_cell_is_a_plausible_monospace_cell() {
        assert!(DEFAULT_CELL.w > 0 && DEFAULT_CELL.h > 0);
        assert!(DEFAULT_CELL.h > DEFAULT_CELL.w, "cells are taller than wide");
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p wb-term`
Expected: FAIL to compile — `WinSize`, `cell_size_from`, `DEFAULT_CELL` not found.

- [ ] **Step 4: Write the implementation**

Prepend to `crates/wb-term/src/probe.rs`:

```rust
//! Measuring the terminal, which decides the CSS viewport we hand Chromium.

use anyhow::{Context, Result};
use wb_frame::{CellSize, GridSize};

/// Used when the terminal will not tell us its pixel dimensions. Roughly a
/// 10pt monospace cell; wrong but usable, and the user can override it.
pub const DEFAULT_CELL: CellSize = CellSize { w: 9, h: 20 };

/// The four fields of `struct winsize`, lifted out of the syscall so the
/// arithmetic below can be tested without a tty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinSize {
    pub cols: u16,
    pub rows: u16,
    pub xpixel: u16,
    pub ypixel: u16,
}

/// Cell size in pixels, or `None` if the terminal did not report enough to
/// compute one.
pub fn cell_size_from(ws: WinSize) -> Option<CellSize> {
    if ws.cols == 0 || ws.rows == 0 || ws.xpixel == 0 || ws.ypixel == 0 {
        return None;
    }
    let w = ws.xpixel / ws.cols;
    let h = ws.ypixel / ws.rows;
    if w == 0 || h == 0 {
        return None;
    }
    Some(CellSize { w, h })
}

/// Ask the controlling terminal for its grid and cell size.
///
/// Falls back to `DEFAULT_CELL` when the terminal does not report pixel
/// dimensions, which is normal under some multiplexers and whenever stdout is
/// not a tty.
pub fn probe() -> Result<(GridSize, CellSize)> {
    let ws = read_winsize().context("could not read the terminal size")?;
    let grid = GridSize {
        cols: ws.cols,
        rows: ws.rows,
    };
    let cell = cell_size_from(ws).unwrap_or(DEFAULT_CELL);
    Ok((grid, cell))
}

fn read_winsize() -> Result<WinSize> {
    let size = rustix::termios::tcgetwinsize(rustix::stdio::stdout())
        .context("TIOCGWINSZ failed; is stdout a terminal?")?;
    Ok(WinSize {
        cols: size.ws_col,
        rows: size.ws_row,
        xpixel: size.ws_xpixel,
        ypixel: size.ws_ypixel,
    })
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p wb-term`
Expected: PASS, 6 tests.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/wb-term
git commit -m "feat(term): probe terminal grid and cell size"
```

---

### Task 5: Rendering a frame to the terminal

**Files:**
- Create: `crates/wb-term/src/render.rs`
- Modify: `crates/wb-term/src/lib.rs`
- Test: inline `#[cfg(test)]` module in `crates/wb-term/src/render.rs`

**Interfaces:**
- Consumes: `wb_frame::{Frame, GridSize, CellPos, Style, Rgb}`.
- Produces: `wb_term::render::render(frame: &Frame, out: &mut impl std::io::Write) -> std::io::Result<()>`.

Writing to any `Write` rather than to stdout is what makes this testable against a byte buffer.

- [ ] **Step 1: Write the failing tests**

Create `crates/wb-term/src/render.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wb_frame::{CellSize, CssRect, Frame, GridSize, Rgb, Style, TextRun, Viewport};

    fn vp() -> Viewport {
        Viewport::new(GridSize { cols: 10, rows: 2 }, CellSize { w: 10, h: 20 })
    }

    fn painted(text: &str, style: Style) -> Frame {
        let mut f = Frame::new(vp().grid());
        f.paint_run(
            &vp(),
            &TextRun {
                text: text.to_string(),
                rect: CssRect { x: 0.0, y: 0.0, w: 100.0, h: 16.0 },
                baseline: 14.0,
                style,
                z: 0,
            },
        );
        f
    }

    fn render_to_string(f: &Frame) -> String {
        let mut buf = Vec::new();
        render(f, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn render_homes_the_cursor_first() {
        let out = render_to_string(&Frame::new(GridSize { cols: 10, rows: 2 }));
        assert!(out.starts_with("\x1b[H"), "output was {out:?}");
    }

    #[test]
    fn render_emits_the_cell_text() {
        let out = render_to_string(&painted("hi", Style::default()));
        assert!(out.contains("hi"), "output was {out:?}");
    }

    #[test]
    fn render_clears_to_end_of_each_line() {
        let out = render_to_string(&painted("hi", Style::default()));
        assert_eq!(out.matches("\x1b[K").count(), 2, "one per row");
    }

    #[test]
    fn render_sets_truecolor_foreground() {
        let style = Style { fg: Rgb { r: 255, g: 128, b: 0 }, bold: false };
        let out = render_to_string(&painted("hi", style));
        assert!(out.contains("\x1b[38;2;255;128;0m"), "output was {out:?}");
    }

    #[test]
    fn render_sets_and_clears_bold() {
        let style = Style { fg: Rgb { r: 0, g: 0, b: 0 }, bold: true };
        let out = render_to_string(&painted("hi", style));
        assert!(out.contains("\x1b[1m"), "output was {out:?}");
        assert!(out.ends_with("\x1b[0m"), "output was {out:?}");
    }

    #[test]
    fn render_does_not_repeat_an_unchanged_style() {
        let style = Style { fg: Rgb { r: 10, g: 20, b: 30 }, bold: false };
        let out = render_to_string(&painted("hello", style));
        assert_eq!(
            out.matches("\x1b[38;2;10;20;30m").count(),
            1,
            "style should be emitted once for the run, not once per cell: {out:?}"
        );
    }

    #[test]
    fn render_separates_rows_with_newlines() {
        let out = render_to_string(&Frame::new(GridSize { cols: 4, rows: 3 }));
        assert_eq!(out.matches("\r\n").count(), 2, "no trailing newline");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p wb-term`
Expected: FAIL to compile — `render` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/wb-term/src/render.rs`:

```rust
//! Writing a `Frame` to a terminal.
//!
//! M1 repaints the whole grid every time. The diffing renderer that only
//! emits changed cells is M2; the signature here does not change when it
//! arrives.

use std::io::Write;

use wb_frame::{CellPos, Frame, Style};

/// Write the whole frame, leaving the terminal with default attributes.
pub fn render(frame: &Frame, out: &mut impl Write) -> std::io::Result<()> {
    let grid = frame.grid();
    // Home the cursor rather than clearing the screen: clearing first causes a
    // visible flash on every repaint.
    write!(out, "\x1b[H")?;

    let mut active: Option<Style> = None;
    for row in 0..grid.rows {
        if row > 0 {
            write!(out, "\r\n")?;
        }
        for col in 0..grid.cols {
            let cell = frame
                .cell(CellPos { col, row })
                .expect("cell within the frame's own grid");
            if active != Some(cell.style) {
                write_style(out, &cell.style)?;
                active = Some(cell.style);
            }
            let mut buf = [0u8; 4];
            out.write_all(cell.ch.encode_utf8(&mut buf).as_bytes())?;
        }
        // Erase anything the previous frame left beyond our last column.
        write!(out, "\x1b[K")?;
    }
    write!(out, "\x1b[0m")?;
    out.flush()
}

fn write_style(out: &mut impl Write, style: &Style) -> std::io::Result<()> {
    // Reset first so that clearing bold does not need a separate sequence.
    write!(out, "\x1b[0m")?;
    if style.bold {
        write!(out, "\x1b[1m")?;
    }
    write!(
        out,
        "\x1b[38;2;{};{};{}m",
        style.fg.r, style.fg.g, style.fg.b
    )
}
```

Update `crates/wb-term/src/lib.rs`:

```rust
pub mod probe;
pub mod render;

pub use probe::{DEFAULT_CELL, WinSize, cell_size_from, probe};
pub use render::render;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p wb-term`
Expected: PASS, 13 tests.

Note: `render_clears_to_end_of_each_line` counts `\x1b[K` and `render_sets_and_clears_bold` asserts the trailing reset; if `write_style` emitted `\x1b[0m` at a different point these counts would shift. Verify the counts rather than adjusting the tests to fit.

- [ ] **Step 5: Commit**

```bash
git add crates/wb-term
git commit -m "feat(term): render a frame to an ANSI sink"
```

---

### Task 6: The CDP client

**Files:**
- Create: `crates/wb-cdp/Cargo.toml`
- Create: `crates/wb-cdp/src/lib.rs`
- Create: `crates/wb-cdp/src/launch.rs`
- Create: `crates/wb-cdp/src/client.rs`
- Create: `crates/wb-cdp/tests/browser.rs`
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  `wb_cdp::launch::{find_chromium, Chromium}` with
  `find_chromium() -> anyhow::Result<PathBuf>` and
  `Chromium::launch() -> anyhow::Result<Chromium>`, `Chromium::ws_url(&self) -> &str`;
  `wb_cdp::client::Client` with
  `Client::connect(ws_url: &str) -> anyhow::Result<Client>`,
  `Client::call(&self, method: &str, params: serde_json::Value) -> anyhow::Result<serde_json::Value>`,
  `Client::call_on(&self, session_id: &str, method: &str, params: serde_json::Value) -> anyhow::Result<serde_json::Value>`.

`call` returns the `result` object of the CDP response, and turns a CDP `error` into an `Err`.

- [ ] **Step 1: Install Chromium and confirm the toolchain can reach it**

Chromium is an external dependency and is not currently installed. Run:

```bash
sudo pacman -S --needed chromium
chromium --version
```

Expected: a version string, `Chromium 151.` or newer. If you prefer a different
build, set `WEBINAL_CHROMIUM` to its absolute path instead; the code honors it.

- [ ] **Step 2: Create the crate**

Create `crates/wb-cdp/Cargo.toml`:

```toml
[package]
name = "wb-cdp"
edition.workspace = true
version.workspace = true

[dependencies]
anyhow.workspace = true
futures-util.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tokio-tungstenite.workspace = true
tempfile.workspace = true

[dev-dependencies]
tokio = { workspace = true, features = ["rt-multi-thread", "macros"] }
```

Create `crates/wb-cdp/src/lib.rs`:

```rust
pub mod client;
pub mod launch;

pub use client::Client;
pub use launch::{Chromium, find_chromium};
```

Add `"crates/wb-cdp"` to `members` in the workspace `Cargo.toml`.

- [ ] **Step 3: Write the failing integration test**

Create `crates/wb-cdp/tests/browser.rs`:

```rust
//! These tests launch a real Chromium. They are the only tests in the
//! workspace that need one.

use serde_json::json;
use wb_cdp::{Chromium, Client};

#[tokio::test]
async fn launches_chromium_and_reports_its_version() {
    let browser = Chromium::launch().await.expect("launch chromium");
    let client = Client::connect(browser.ws_url()).await.expect("connect");

    let result = client
        .call("Browser.getVersion", json!({}))
        .await
        .expect("Browser.getVersion");

    let product = result["product"].as_str().expect("product string");
    assert!(
        product.contains("Chrome"),
        "unexpected product string: {product}"
    );
}

#[tokio::test]
async fn attaches_to_a_page_target_and_evaluates_javascript() {
    let browser = Chromium::launch().await.expect("launch chromium");
    let client = Client::connect(browser.ws_url()).await.expect("connect");

    let target = client
        .call("Target.createTarget", json!({ "url": "about:blank" }))
        .await
        .expect("createTarget");
    let target_id = target["targetId"].as_str().expect("targetId").to_string();

    let attached = client
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
        )
        .await
        .expect("attachToTarget");
    let session_id = attached["sessionId"].as_str().expect("sessionId").to_string();

    let evaluated = client
        .call_on(
            &session_id,
            "Runtime.evaluate",
            json!({ "expression": "6 * 7", "returnByValue": true }),
        )
        .await
        .expect("Runtime.evaluate");

    assert_eq!(evaluated["result"]["value"].as_i64(), Some(42));
}

#[tokio::test]
async fn a_failing_command_returns_an_error_rather_than_hanging() {
    let browser = Chromium::launch().await.expect("launch chromium");
    let client = Client::connect(browser.ws_url()).await.expect("connect");

    let err = client
        .call("Nonexistent.method", json!({}))
        .await
        .expect_err("an unknown method must be an error");

    assert!(
        err.to_string().contains("Nonexistent.method") || err.to_string().contains("not found"),
        "unhelpful error: {err}"
    );
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p wb-cdp`
Expected: FAIL to compile — `Chromium` and `Client` not found.

- [ ] **Step 5: Write the launcher**

Create `crates/wb-cdp/src/launch.rs`:

```rust
//! Starting a Chromium and finding its websocket endpoint.

use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Context, Result, anyhow, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{Duration, timeout};

const CANDIDATES: &[&str] = &["chromium", "chromium-browser", "google-chrome-stable"];
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);

/// Locate a Chromium binary. `WEBINAL_CHROMIUM` wins if set.
///
/// We never download a browser; an absent one is a clear error with an
/// actionable message, per spec section 8.
pub fn find_chromium() -> Result<PathBuf> {
    if let Ok(explicit) = std::env::var("WEBINAL_CHROMIUM") {
        let path = PathBuf::from(&explicit);
        if !path.is_file() {
            bail!("WEBINAL_CHROMIUM is set to {explicit}, which is not a file");
        }
        return Ok(path);
    }

    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for name in CANDIDATES {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err(anyhow!(
        "no Chromium found. Install one (`sudo pacman -S chromium`) or set \
         WEBINAL_CHROMIUM to the absolute path of a Chromium binary."
    ))
}

/// A running headless Chromium. Killed on drop.
pub struct Chromium {
    child: Child,
    ws_url: String,
    /// Held so the profile directory outlives the browser. M4 replaces this
    /// with a persistent profile under the user's data directory.
    _profile: tempfile::TempDir,
}

impl Chromium {
    pub async fn launch() -> Result<Self> {
        let binary = find_chromium()?;
        let profile = tempfile::tempdir().context("create a temporary profile directory")?;

        let mut child = Command::new(&binary)
            .arg("--headless=new")
            // Port 0 lets the OS pick; we read the real one back off stderr.
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-gpu")
            .arg("about:blank")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start {}", binary.display()))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("chromium stderr was not piped"))?;

        let ws_url = timeout(STARTUP_TIMEOUT, read_ws_url(stderr))
            .await
            .map_err(|_| anyhow!("chromium did not report a debugging endpoint within 20s"))??;

        Ok(Self {
            child,
            ws_url,
            _profile: profile,
        })
    }

    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }
}

impl Drop for Chromium {
    fn drop(&mut self) {
        // kill_on_drop handles the process; start_kill makes it prompt.
        let _ = self.child.start_kill();
    }
}

/// Chromium announces its endpoint on stderr as
/// `DevTools listening on ws://127.0.0.1:PORT/devtools/browser/UUID`.
async fn read_ws_url(stderr: tokio::process::ChildStderr) -> Result<String> {
    let mut lines = BufReader::new(stderr).lines();
    while let Some(line) = lines.next_line().await? {
        if let Some(idx) = line.find("ws://") {
            return Ok(line[idx..].trim().to_string());
        }
    }
    bail!("chromium exited before announcing a debugging endpoint")
}
```

- [ ] **Step 6: Write the client**

Create `crates/wb-cdp/src/client.rs`:

```rust
//! A minimal CDP client: request/response correlation over one websocket.
//!
//! M1 discards protocol events. The event pump that feeds the extraction loop
//! is M2; it hooks into `read_loop` below without changing this API.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::{Duration, timeout};

/// Every command carries a deadline, so a wedged page cannot hang the caller.
/// Spec section 8.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

pub struct Client {
    next_id: AtomicU64,
    outgoing: mpsc::UnboundedSender<String>,
    pending: Pending,
}

impl Client {
    pub async fn connect(ws_url: &str) -> Result<Self> {
        let (stream, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .with_context(|| format!("failed to connect to {ws_url}"))?;
        let (mut sink, stream) = stream.split();

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if sink.send(msg.into()).await.is_err() {
                    break;
                }
            }
        });

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        tokio::spawn(read_loop(stream, Arc::clone(&pending)));

        Ok(Self {
            next_id: AtomicU64::new(1),
            outgoing: tx,
            pending,
        })
    }

    /// Send a command to the browser target.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.send(method, params, None).await
    }

    /// Send a command to an attached session (a page).
    pub async fn call_on(&self, session_id: &str, method: &str, params: Value) -> Result<Value> {
        self.send(method, params, Some(session_id)).await
    }

    async fn send(&self, method: &str, params: Value, session: Option<&str>) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(session_id) = session {
            msg["sessionId"] = json!(session_id);
        }

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        self.outgoing
            .send(msg.to_string())
            .map_err(|_| anyhow!("the CDP connection is closed"))?;

        let response = match timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(v)) => v,
            Ok(Err(_)) => {
                return Err(anyhow!("the CDP connection closed while awaiting {method}"));
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(anyhow!("{method} timed out after {CALL_TIMEOUT:?}"));
            }
        };

        if let Some(error) = response.get("error") {
            let message = error["message"].as_str().unwrap_or("unknown error");
            let data = error["data"].as_str().unwrap_or_default();
            return Err(anyhow!("{method} failed: {message} {data}").context(method.to_string()));
        }

        Ok(response
            .get("result")
            .cloned()
            .unwrap_or_else(|| json!({})))
    }
}

async fn read_loop<S>(mut stream: S, pending: Pending)
where
    S: futures_util::Stream<Item = Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    while let Some(Ok(msg)) = stream.next().await {
        let Ok(text) = msg.into_text() else { continue };
        let Ok(value): Result<Value, _> = serde_json::from_str(&text) else {
            continue;
        };
        // Messages with an `id` are responses; everything else is an event,
        // which M1 drops on the floor.
        let Some(id) = value.get("id").and_then(Value::as_u64) else {
            continue;
        };
        if let Some(tx) = pending.lock().await.remove(&id) {
            let _ = tx.send(value);
        }
    }
    // The socket is gone; wake every caller rather than letting them wait out
    // their deadlines.
    pending.lock().await.clear();
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p wb-cdp`
Expected: PASS, 3 tests. They take a few seconds each because they each start a browser.

If `a_failing_command_returns_an_error_rather_than_hanging` fails on the assertion
rather than on the timeout, read the actual message and widen the assertion to
match what Chromium 151 reports — but only after confirming the call returned
promptly.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock crates/wb-cdp
git commit -m "feat(cdp): add chromium launcher and CDP client"
```

---

### Task 7: Extracting text runs from a page

**Files:**
- Create: `crates/wb-page/Cargo.toml`
- Create: `crates/wb-page/src/lib.rs`
- Create: `crates/wb-page/src/color.rs`
- Create: `crates/wb-page/src/extract.rs`
- Create: `crates/wb-page/assets/extract.js`
- Create: `crates/wb-page/tests/fixtures/simple.html`
- Create: `crates/wb-page/tests/extraction.rs`
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Consumes: `wb_cdp::{Chromium, Client}`, `wb_frame::{Viewport, TextRun, Style, Rgb, CssRect}`.
- Produces:
  `wb_page::color::parse_css_color(s: &str) -> Rgb`;
  `wb_page::extract::Page` with
  `Page::open(client: &Client, url: &str, vp: Viewport) -> anyhow::Result<Page>`,
  `Page::extract(&self) -> anyhow::Result<Vec<TextRun>>`,
  `Page::title(&self) -> anyhow::Result<String>`.

- [ ] **Step 1: Create the crate and write the failing color tests**

Create `crates/wb-page/Cargo.toml`:

```toml
[package]
name = "wb-page"
edition.workspace = true
version.workspace = true

[dependencies]
wb-cdp = { path = "../wb-cdp" }
wb-frame = { path = "../wb-frame" }
anyhow.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true

[dev-dependencies]
tokio = { workspace = true, features = ["rt-multi-thread", "macros"] }
```

Create `crates/wb-page/src/lib.rs`:

```rust
pub mod color;
pub mod extract;

pub use extract::Page;
```

Add `"crates/wb-page"` to `members` in the workspace `Cargo.toml`.

Create `crates/wb-page/src/color.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rgb() {
        assert_eq!(
            parse_css_color("rgb(255, 128, 0)"),
            Rgb { r: 255, g: 128, b: 0 }
        );
    }

    #[test]
    fn parses_rgb_without_spaces() {
        assert_eq!(parse_css_color("rgb(1,2,3)"), Rgb { r: 1, g: 2, b: 3 });
    }

    #[test]
    fn parses_rgba_and_ignores_alpha() {
        assert_eq!(
            parse_css_color("rgba(10, 20, 30, 0.5)"),
            Rgb { r: 10, g: 20, b: 30 }
        );
    }

    #[test]
    fn parses_the_modern_space_separated_form() {
        assert_eq!(
            parse_css_color("rgb(10 20 30 / 0.5)"),
            Rgb { r: 10, g: 20, b: 30 }
        );
    }

    #[test]
    fn clamps_out_of_range_components() {
        assert_eq!(parse_css_color("rgb(300, -5, 0)"), Rgb { r: 255, g: 0, b: 0 });
    }

    #[test]
    fn falls_back_to_the_default_style_color_on_junk() {
        // getComputedStyle always returns rgb()/rgba(), so anything else means
        // we misread something; a readable default beats a panic.
        assert_eq!(parse_css_color("chartreuse"), Style::default().fg);
        assert_eq!(parse_css_color(""), Style::default().fg);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p wb-page`
Expected: FAIL to compile — `parse_css_color` not found.

- [ ] **Step 3: Write the color parser**

Prepend to `crates/wb-page/src/color.rs`:

```rust
//! Parsing the colors `getComputedStyle` hands back.

use wb_frame::{Rgb, Style};

/// Parse an `rgb()` or `rgba()` string. Anything unrecognized falls back to
/// the default foreground rather than failing the whole extraction.
pub fn parse_css_color(s: &str) -> Rgb {
    let fallback = Style::default().fg;
    let Some(open) = s.find('(') else { return fallback };
    let Some(close) = s.rfind(')') else {
        return fallback;
    };
    let body = &s[open + 1..close];
    // Handles "1, 2, 3", "1 2 3", and "1 2 3 / 0.5" in one pass.
    let body = body.split('/').next().unwrap_or(body);

    let mut parts = body
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty());

    let mut next = || -> Option<u8> {
        let raw: f64 = parts.next()?.parse().ok()?;
        Some(raw.clamp(0.0, 255.0) as u8)
    };

    match (next(), next(), next()) {
        (Some(r), Some(g), Some(b)) => Rgb { r, g, b },
        _ => fallback,
    }
}
```

- [ ] **Step 4: Run the color tests to verify they pass**

Run: `cargo test -p wb-page`
Expected: PASS, 6 tests.

- [ ] **Step 5: Write the extraction script**

Create `crates/wb-page/assets/extract.js`:

```javascript
// Extract every visible text run, one per line of text, with its layout box.
//
// M1 measures each character's rect individually and groups by rounded top.
// That is O(n) ranges per text node and slow on large pages, but it is exact
// and needs no heuristics about where lines break. M2 replaces the inner loop
// with a binary search over character offsets.
(() => {
  const runs = [];
  const vw = window.innerWidth;
  const vh = window.innerHeight;

  const walker = document.createTreeWalker(
    document.body,
    NodeFilter.SHOW_TEXT,
    null
  );

  const range = document.createRange();
  let node;

  while ((node = walker.nextNode())) {
    const text = node.nodeValue;
    if (!text || !text.trim()) continue;

    const parent = node.parentElement;
    if (!parent) continue;

    const cs = window.getComputedStyle(parent);
    if (cs.visibility === "hidden" || cs.display === "none" || cs.opacity === "0") {
      continue;
    }
    if (parent.tagName === "SCRIPT" || parent.tagName === "STYLE") continue;

    // Group the node's characters into lines by their rounded top edge.
    const lines = new Map();
    for (let i = 0; i < text.length; i++) {
      range.setStart(node, i);
      range.setEnd(node, i + 1);
      const r = range.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) continue;

      const key = Math.round(r.top);
      let line = lines.get(key);
      if (!line) {
        line = { chars: [], left: r.left, right: r.right, top: r.top, bottom: r.bottom };
        lines.set(key, line);
      }
      line.chars.push(text[i]);
      line.left = Math.min(line.left, r.left);
      line.right = Math.max(line.right, r.right);
      line.bottom = Math.max(line.bottom, r.bottom);
    }

    const fontSize = parseFloat(cs.fontSize) || 16;
    const weight = parseInt(cs.fontWeight, 10) || 400;

    for (const line of lines.values()) {
      const content = line.chars.join("").replace(/\s+/g, " ").trim();
      if (!content) continue;

      // Cull runs entirely outside the viewport.
      if (line.bottom < 0 || line.top > vh || line.right < 0 || line.left > vw) {
        continue;
      }

      runs.push({
        text: content,
        x: line.left,
        y: line.top,
        w: line.right - line.left,
        h: line.bottom - line.top,
        // The descender is roughly a fifth of the font size; close enough to
        // put the baseline in the right cell row.
        baseline: line.bottom - fontSize * 0.21,
        color: cs.color,
        bold: weight >= 600,
        z: 0,
      });
    }
  }

  return { runs, title: document.title };
})()
```

- [ ] **Step 6: Write the failing extraction test**

Create `crates/wb-page/tests/fixtures/simple.html`:

```html
<!doctype html>
<html>
  <head>
    <title>Fixture Page</title>
    <style>
      body { margin: 0; font-family: monospace; font-size: 16px; }
      h1 { margin: 0; font-size: 32px; font-weight: 700; color: rgb(255, 0, 0); }
      p { margin: 0; color: rgb(0, 0, 255); }
      .hidden { visibility: hidden; }
    </style>
  </head>
  <body>
    <h1>Heading</h1>
    <p>First paragraph.</p>
    <p class="hidden">Invisible text.</p>
  </body>
</html>
```

Create `crates/wb-page/tests/extraction.rs`:

```rust
use wb_cdp::{Chromium, Client};
use wb_frame::{CellSize, GridSize, Viewport};
use wb_page::Page;

fn fixture_url(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    format!("file://{}", path.display())
}

fn viewport() -> Viewport {
    Viewport::new(
        GridSize { cols: 80, rows: 24 },
        CellSize { w: 9, h: 20 },
    )
}

async fn open_fixture(name: &str) -> (Chromium, Client, Page) {
    let browser = Chromium::launch().await.expect("launch chromium");
    let client = Client::connect(browser.ws_url()).await.expect("connect");
    let page = Page::open(&client, &fixture_url(name), viewport())
        .await
        .expect("open the fixture");
    (browser, client, page)
}

#[tokio::test]
async fn extracts_the_visible_text_of_a_page() {
    let (_browser, _client, page) = open_fixture("simple.html").await;
    let runs = page.extract().await.expect("extract");

    let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
    assert!(texts.contains(&"Heading"), "runs were {texts:?}");
    assert!(texts.contains(&"First paragraph."), "runs were {texts:?}");
}

#[tokio::test]
async fn skips_hidden_text() {
    let (_browser, _client, page) = open_fixture("simple.html").await;
    let runs = page.extract().await.expect("extract");

    let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
    assert!(
        !texts.contains(&"Invisible text."),
        "visibility:hidden text must not be extracted: {texts:?}"
    );
}

#[tokio::test]
async fn carries_color_and_weight_through() {
    let (_browser, _client, page) = open_fixture("simple.html").await;
    let runs = page.extract().await.expect("extract");

    let heading = runs.iter().find(|r| r.text == "Heading").expect("heading run");
    assert_eq!(heading.style.fg, wb_frame::Rgb { r: 255, g: 0, b: 0 });
    assert!(heading.style.bold, "font-weight 700 is bold");

    let para = runs
        .iter()
        .find(|r| r.text == "First paragraph.")
        .expect("paragraph run");
    assert_eq!(para.style.fg, wb_frame::Rgb { r: 0, g: 0, b: 255 });
    assert!(!para.style.bold);
}

#[tokio::test]
async fn orders_runs_down_the_page() {
    let (_browser, _client, page) = open_fixture("simple.html").await;
    let runs = page.extract().await.expect("extract");

    let heading = runs.iter().find(|r| r.text == "Heading").expect("heading");
    let para = runs
        .iter()
        .find(|r| r.text == "First paragraph.")
        .expect("paragraph");
    assert!(
        heading.baseline < para.baseline,
        "the heading sits above the paragraph"
    );
}

#[tokio::test]
async fn reads_the_document_title() {
    let (_browser, _client, page) = open_fixture("simple.html").await;
    assert_eq!(page.title().await.expect("title"), "Fixture Page");
}

#[tokio::test]
async fn lays_the_page_out_at_the_viewport_we_asked_for() {
    let (_browser, _client, page) = open_fixture("simple.html").await;
    let runs = page.extract().await.expect("extract");

    // The viewport is 80 * 9 = 720 CSS px wide; nothing may be laid out
    // beyond it, which is how we know setDeviceMetricsOverride took effect.
    for run in &runs {
        assert!(
            run.rect.x < 720.0,
            "run {:?} starts outside the 720px viewport",
            run.text
        );
    }
}
```

- [ ] **Step 7: Run the tests to verify they fail**

Run: `cargo test -p wb-page`
Expected: FAIL to compile — `Page` not found.

- [ ] **Step 8: Write the page implementation**

Create `crates/wb-page/src/extract.rs`:

```rust
//! One page: navigate, size it to the terminal, pull its text runs out.

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::json;
use tokio::time::{Duration, Instant, sleep};
use wb_cdp::Client;
use wb_frame::{CssRect, Style, TextRun, Viewport};

use crate::color::parse_css_color;

const EXTRACT_JS: &str = include_str!("../assets/extract.js");
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
const LOAD_POLL: Duration = Duration::from_millis(50);

/// The shape `extract.js` returns.
#[derive(Debug, Deserialize)]
struct RawExtraction {
    runs: Vec<RawRun>,
}

#[derive(Debug, Deserialize)]
struct RawRun {
    text: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    baseline: f64,
    color: String,
    bold: bool,
    z: i32,
}

pub struct Page<'a> {
    client: &'a Client,
    session_id: String,
}

impl<'a> Page<'a> {
    /// Create a target, size it to the viewport, navigate, and wait for load.
    pub async fn open(client: &'a Client, url: &str, vp: Viewport) -> Result<Page<'a>> {
        let target = client
            .call("Target.createTarget", json!({ "url": "about:blank" }))
            .await
            .context("create a page target")?;
        let target_id = target["targetId"]
            .as_str()
            .ok_or_else(|| anyhow!("Target.createTarget returned no targetId"))?
            .to_string();

        let attached = client
            .call(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
            )
            .await
            .context("attach to the page target")?;
        let session_id = attached["sessionId"]
            .as_str()
            .ok_or_else(|| anyhow!("Target.attachToTarget returned no sessionId"))?
            .to_string();

        let page = Page { client, session_id };
        page.set_viewport(vp).await?;
        page.navigate(url).await?;
        Ok(page)
    }

    /// Tell Chromium the window is exactly the terminal grid. Spec section 3.
    pub async fn set_viewport(&self, vp: Viewport) -> Result<()> {
        self.client
            .call_on(
                &self.session_id,
                "Emulation.setDeviceMetricsOverride",
                json!({
                    "width": vp.css_width(),
                    "height": vp.css_height(),
                    "deviceScaleFactor": 1,
                    "mobile": false,
                }),
            )
            .await
            .context("set the device metrics override")?;
        Ok(())
    }

    async fn navigate(&self, url: &str) -> Result<()> {
        self.client
            .call_on(&self.session_id, "Page.enable", json!({}))
            .await
            .context("enable the Page domain")?;

        let result = self
            .client
            .call_on(&self.session_id, "Page.navigate", json!({ "url": url }))
            .await
            .with_context(|| format!("navigate to {url}"))?;

        if let Some(error) = result.get("errorText").and_then(|v| v.as_str()) {
            bail!("navigation to {url} failed: {error}");
        }

        self.wait_for_load().await
    }

    /// M1 polls `document.readyState`. M2 replaces this with the
    /// `Page.loadEventFired` event once the CDP event pump exists.
    async fn wait_for_load(&self) -> Result<()> {
        let deadline = Instant::now() + LOAD_TIMEOUT;
        loop {
            let state = self
                .client
                .call_on(
                    &self.session_id,
                    "Runtime.evaluate",
                    json!({ "expression": "document.readyState", "returnByValue": true }),
                )
                .await
                .context("poll document.readyState")?;

            if state["result"]["value"].as_str() == Some("complete") {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("the page did not finish loading within {LOAD_TIMEOUT:?}");
            }
            sleep(LOAD_POLL).await;
        }
    }

    pub async fn title(&self) -> Result<String> {
        let result = self
            .client
            .call_on(
                &self.session_id,
                "Runtime.evaluate",
                json!({ "expression": "document.title", "returnByValue": true }),
            )
            .await
            .context("read document.title")?;
        Ok(result["result"]["value"].as_str().unwrap_or_default().to_string())
    }

    /// Run the extraction script and convert its output into `TextRun`s.
    pub async fn extract(&self) -> Result<Vec<TextRun>> {
        let result = self
            .client
            .call_on(
                &self.session_id,
                "Runtime.evaluate",
                json!({
                    "expression": EXTRACT_JS,
                    "returnByValue": true,
                    "awaitPromise": false,
                }),
            )
            .await
            .context("run the extraction script")?;

        if let Some(details) = result.get("exceptionDetails") {
            bail!("the extraction script threw: {details}");
        }

        let raw: RawExtraction = serde_json::from_value(result["result"]["value"].clone())
            .context("the extraction script returned an unexpected shape")?;

        Ok(raw
            .runs
            .into_iter()
            .map(|r| TextRun {
                text: r.text,
                rect: CssRect { x: r.x, y: r.y, w: r.w, h: r.h },
                baseline: r.baseline,
                style: Style {
                    fg: parse_css_color(&r.color),
                    bold: r.bold,
                },
                z: r.z,
            })
            .collect())
    }
}
```

- [ ] **Step 9: Run the tests to verify they pass**

Run: `cargo test -p wb-page`
Expected: PASS, 12 tests (6 color, 6 extraction).

- [ ] **Step 10: Commit**

```bash
git add Cargo.toml Cargo.lock crates/wb-page
git commit -m "feat(page): extract positioned text runs from a live page"
```

---

### Task 8: The binary, end to end

**Files:**
- Create: `crates/webinal/Cargo.toml`
- Create: `crates/webinal/src/lib.rs`
- Create: `crates/webinal/src/main.rs`
- Create: `crates/webinal/tests/fixtures/skeleton.html`
- Create: `crates/webinal/tests/smoke.rs`
- Modify: `Cargo.toml` (workspace members)
- Create: `README.md`

**Interfaces:**
- Consumes: everything from Tasks 1 through 7.
- Produces: `webinal::render_url(url: &str, vp: Viewport) -> anyhow::Result<Frame>`.

The pipeline lives in `lib.rs` so the smoke test can drive it without a terminal; `main.rs` only handles the terminal and the key loop.

- [ ] **Step 1: Create the crate**

Create `crates/webinal/Cargo.toml`:

```toml
[package]
name = "webinal"
edition.workspace = true
version.workspace = true

[dependencies]
wb-cdp = { path = "../wb-cdp" }
wb-frame = { path = "../wb-frame" }
wb-page = { path = "../wb-page" }
wb-term = { path = "../wb-term" }
anyhow.workspace = true
crossterm.workspace = true
tokio.workspace = true

[dev-dependencies]
tokio = { workspace = true, features = ["rt-multi-thread", "macros"] }
```

Add `"crates/webinal"` to `members` in the workspace `Cargo.toml`.

- [ ] **Step 2: Write the failing smoke test**

Create `crates/webinal/tests/fixtures/skeleton.html`:

```html
<!doctype html>
<html>
  <head>
    <title>Skeleton</title>
    <style>
      body { margin: 0; font-family: monospace; font-size: 16px; }
      p { margin: 0; }
    </style>
  </head>
  <body>
    <p>WEBINAL WALKS</p>
  </body>
</html>
```

Create `crates/webinal/tests/smoke.rs`:

```rust
use wb_frame::{CellSize, GridSize, Viewport};

fn fixture_url(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    format!("file://{}", path.display())
}

#[tokio::test]
async fn renders_a_page_into_the_cell_grid() {
    let vp = Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
    let frame = webinal::render_url(&fixture_url("skeleton.html"), vp)
        .await
        .expect("render the fixture");

    assert_eq!(frame.grid(), vp.grid());

    let rendered: Vec<String> = (0..vp.grid().rows).map(|r| frame.row_text(r)).collect();
    assert!(
        rendered.iter().any(|line| line.contains("WEBINAL WALKS")),
        "the page text is missing from the frame:\n{}",
        rendered.join("\n")
    );
}

#[tokio::test]
async fn text_lands_in_the_top_left_of_an_unstyled_page() {
    let vp = Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
    let frame = webinal::render_url(&fixture_url("skeleton.html"), vp)
        .await
        .expect("render the fixture");

    // body margin is 0 and the paragraph is the first thing on the page, so
    // it must land in row 0 starting at column 0. This is the assertion that
    // proves the coordinate model is wired correctly end to end.
    assert!(
        frame.row_text(0).starts_with("WEBINAL"),
        "row 0 was {:?}",
        frame.row_text(0)
    );
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p webinal`
Expected: FAIL to compile — `webinal::render_url` not found.

- [ ] **Step 4: Write the pipeline**

Create `crates/webinal/src/lib.rs`:

```rust
//! Wiring: browser, page, frame.

use anyhow::{Context, Result};
use wb_cdp::{Chromium, Client};
use wb_frame::{Frame, Viewport};
use wb_page::Page;

/// Launch a browser, render one URL, and return the resulting frame.
///
/// M1 tears the browser down on return. M4 replaces this with a session that
/// keeps the browser and its targets alive across navigations.
pub async fn render_url(url: &str, vp: Viewport) -> Result<Frame> {
    let browser = Chromium::launch().await.context("launch chromium")?;
    let client = Client::connect(browser.ws_url())
        .await
        .context("connect to chromium")?;
    let page = Page::open(&client, url, vp).await?;

    let runs = page.extract().await?;
    let mut frame = Frame::new(vp.grid());
    for run in &runs {
        frame.paint_run(&vp, run);
    }
    Ok(frame)
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p webinal`
Expected: PASS, 2 tests.

If `text_lands_in_the_top_left_of_an_unstyled_page` fails by one row, the
baseline approximation in `extract.js` is off for this font size. Do not adjust
the test — print the run's `baseline` and `rect` and confirm against the cell
height before changing the constant.

- [ ] **Step 6: Write the binary**

Create `crates/webinal/src/main.rs`:

```rust
use std::io::{Write, stdout};

use anyhow::{Context, Result, bail};
use crossterm::event::{Event, KeyCode, KeyEvent, read};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, cursor};
use wb_frame::Viewport;

#[tokio::main]
async fn main() -> Result<()> {
    let Some(url) = std::env::args().nth(1) else {
        bail!("usage: webinal <url>");
    };

    let (grid, cell) = wb_term::probe().context("measure the terminal")?;
    let vp = Viewport::new(grid, cell);

    // Render before touching the terminal, so a failure leaves the user's
    // screen exactly as it was.
    let frame = webinal::render_url(&url, vp).await?;

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, cursor::Hide)?;

    let result = run(&frame);

    execute!(stdout(), cursor::Show, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    result
}

fn run(frame: &wb_frame::Frame) -> Result<()> {
    let mut out = stdout();
    wb_term::render(frame, &mut out)?;
    out.flush()?;

    loop {
        if let Event::Key(KeyEvent { code, .. }) = read()? {
            match code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                _ => {}
            }
        }
    }
}
```

- [ ] **Step 7: Verify the whole workspace builds and tests green**

Run: `cargo test --workspace`
Expected: PASS, 47 tests across five crates.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings. Fix any that appear.

- [ ] **Step 8: Verify by hand in a real terminal**

Run, in Kitty, not through a pipe:

```bash
cargo run -p webinal -- https://example.com
```

Expected: the terminal switches to an alternate screen showing the text of
example.com laid out roughly where a browser puts it — the heading near the
top left, the paragraph beneath it — in color. Press `q`; the terminal returns
to your shell exactly as it was.

Confirm all three before continuing: text appears, layout resembles the real
page, and `q` restores the terminal cleanly. **This is the M1 acceptance
criterion.** If any of the three fails, stop and debug rather than proceeding.

- [ ] **Step 9: Write the README**

Create `README.md`:

```markdown
# Webinal

A terminal web browser in Rust. It drives a real headless Chromium over the
Chrome DevTools Protocol and renders pages into the terminal grid: crisp text
by default, true pixels on demand.

**Status: M1, the walking skeleton.** It renders one page's text and quits.
Navigation, input, tabs, and pixel mode are M2 through M5.

## Requirements

- Rust 1.97+
- Chromium (`sudo pacman -S chromium`), or `WEBINAL_CHROMIUM` set to a
  Chromium binary
- A terminal that reports its pixel dimensions; Kitty is the development
  target

## Usage

    cargo run -p webinal -- https://example.com

Press `q` to quit.

## Layout

| Crate | Responsibility |
|---|---|
| `wb-frame` | Coordinate model and the cell grid. No I/O. |
| `wb-cdp` | Chromium launcher and CDP client. |
| `wb-page` | Text-run extraction from a live page. |
| `wb-term` | Terminal probing and rendering. |
| `webinal` | The binary. |

## Documentation

- Design: `docs/superpowers/specs/2026-08-19-webinal-design.md`
- M1 plan: `docs/superpowers/plans/2026-08-19-webinal-m1-walking-skeleton.md`
```

- [ ] **Step 10: Commit**

```bash
git add Cargo.toml Cargo.lock crates/webinal README.md
git commit -m "feat: render a URL into the terminal grid end to end"
```

---

## Definition of done for M1

- `cargo test --workspace` is green, with 47 tests: 17 in `wb-frame`, 13 in
  `wb-term`, 3 in `wb-cdp`, 12 in `wb-page`, 2 in `webinal`.
- `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- `cargo run -p webinal -- https://example.com` shows the page's text laid out
  approximately where a browser would put it, in color, and `q` restores the
  terminal.
- The coordinate roundtrip property test passes at every cell size, because
  every later milestone assumes it.
