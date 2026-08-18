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
