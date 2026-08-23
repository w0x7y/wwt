use crate::cell::{Cell, Style};
use crate::geom::{CellPos, GridSize, Viewport};
use crate::image::{CellRect, Image};
use crate::run::TextRun;
use crate::samples::Samples;

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
    cursor: Option<CellPos>,
    /// The page as a picture, in pixel mode. `None` is text mode, which is
    /// every frame this codebase built before M5.
    image: Option<Image>,
}

impl Frame {
    pub fn new(grid: GridSize) -> Self {
        let len = usize::from(grid.cols) * usize::from(grid.rows);
        Self {
            grid,
            cells: vec![Cell::default(); len],
            cursor: None,
            image: None,
        }
    }

    pub fn grid(&self) -> GridSize {
        self.grid
    }

    /// Where the terminal's own cursor belongs, if anywhere.
    ///
    /// A frame carries this rather than painting it, because a caret drawn
    /// into a cell can only be as wide as a cell: the terminal's cursor is
    /// the only thin line available, and only the terminal can place it.
    pub fn cursor(&self) -> Option<CellPos> {
        self.cursor
    }

    /// Put the cursor on a cell, or take it off the screen with `None`. A
    /// cell outside the grid is no cell at all.
    pub fn set_cursor(&mut self, pos: Option<CellPos>) {
        self.cursor = pos.filter(|&pos| self.index(pos).is_some());
    }

    /// The picture this frame wants shown behind its cells, if any.
    ///
    /// A frame carries it rather than painting it for the same reason it
    /// carries the cursor: only the terminal can put an image on screen,
    /// and this crate is not allowed to know how.
    pub fn image(&self) -> Option<&Image> {
        self.image.as_ref()
    }

    pub fn set_image(&mut self, image: Option<Image>) {
        self.image = image;
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

    /// Paint a page's runs into the grid.
    ///
    /// Every mode that shows a page reaches the screen through here, so
    /// there is one place that knows how a list of runs becomes cells.
    pub fn paint_runs(&mut self, vp: &Viewport, runs: &[TextRun]) {
        for run in runs {
            self.paint_run(vp, run);
        }
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

        // Counted rather than collected: this runs for every run of every
        // frame, and a `Vec<char>` per run is an allocation per run.
        let total = run.text.chars().count();
        if total == 0 {
            return;
        }

        // Drop the leading characters that fall left of the grid, so a run
        // scrolled partly off-screen still shows its visible tail in place.
        let skip = if start_col < 0 { (-start_col) as usize } else { 0 };
        if skip >= total {
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

        // What does not fit gives up its last cell to the ellipsis, and a
        // one-cell budget is all ellipsis.
        let visible = total - skip;
        let elided = visible > budget;
        let taken = if elided { budget - 1 } else { visible };
        let painted = run.text.chars().skip(skip).take(taken).chain(elided.then_some('…'));

        for (i, ch) in painted.enumerate() {
            let Ok(offset) = u16::try_from(i) else { break };
            let Some(col) = first_col.checked_add(offset) else { break };
            let Some(idx) = self.index(CellPos { col, row }) else { break };
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

    /// Paint a string starting at one cell, clipped at the right edge.
    ///
    /// Chrome uses this. It paints at the maximum stacking depth, and the
    /// compositor paints chrome last, so it takes every cell it touches:
    /// `paint_run` yields a cell to anything at or above its own depth.
    /// Paint a picture as half blocks: the upper half block glyph, the top
    /// sample as its foreground and the bottom sample as its background.
    ///
    /// Painted at the lowest possible depth, because it is the page and
    /// everything else is on top of it. `paint_text` takes what it touches
    /// unconditionally, so a hint label over a picture needs nothing else.
    pub fn paint_samples(&mut self, area: CellRect, samples: &Samples) {
        for row in 0..area.rows {
            for col in 0..area.cols {
                let Some(top) = samples.at(col, row * 2) else { continue };
                // A missing bottom sample means an odd sample row count at
                // the bottom edge. Repeating the top makes the cell a
                // solid block; leaving the background unset would show the
                // terminal's own colour as a stripe.
                let bottom = samples.at(col, row * 2 + 1).unwrap_or(top);
                let Some(pos) = area
                    .col
                    .checked_add(col)
                    .zip(area.row.checked_add(row))
                    .map(|(col, row)| CellPos { col, row })
                else {
                    continue;
                };
                let Some(index) = self.index(pos) else { continue };
                self.cells[index] = Cell {
                    ch: '\u{2580}',
                    style: Style { fg: top, bg: Some(bottom), bold: false, reverse: false },
                    z: i32::MIN,
                };
            }
        }
    }

    pub fn paint_text(&mut self, pos: CellPos, text: &str, style: Style) {
        for (i, ch) in text.chars().enumerate() {
            let Ok(offset) = u16::try_from(i) else { break };
            let Some(col) = pos.col.checked_add(offset) else { break };
            let Some(idx) = self.index(CellPos { col, row: pos.row }) else {
                break;
            };
            self.cells[idx] = Cell { ch, style, z: i32::MAX };
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
    use crate::image::{CellRect, Image};
    use crate::run::TextRun;
    use crate::samples::Samples;

    #[test]
    fn painting_samples_gives_every_cell_two_colours() {
        let mut frame = Frame::new(GridSize { cols: 2, rows: 3 });
        // One cell row of the frame, so two sample rows.
        let samples = Samples {
            cols: 2,
            rows: 2,
            pixels: vec![
                Rgb { r: 1, g: 1, b: 1 },
                Rgb { r: 2, g: 2, b: 2 },
                Rgb { r: 3, g: 3, b: 3 },
                Rgb { r: 4, g: 4, b: 4 },
            ],
        };
        frame.paint_samples(CellRect { col: 0, row: 1, cols: 2, rows: 1 }, &samples);

        let cell = frame.cell(CellPos { col: 0, row: 1 }).expect("painted");
        assert_eq!(cell.ch, '\u{2580}');
        assert_eq!(cell.style.fg, Rgb { r: 1, g: 1, b: 1 });
        assert_eq!(cell.style.bg, Some(Rgb { r: 3, g: 3, b: 3 }));

        let cell = frame.cell(CellPos { col: 1, row: 1 }).expect("painted");
        assert_eq!(cell.style.fg, Rgb { r: 2, g: 2, b: 2 });
        assert_eq!(cell.style.bg, Some(Rgb { r: 4, g: 4, b: 4 }));
    }

    #[test]
    fn a_label_over_a_half_block_page_is_the_label() {
        // The property M5 spent a section of its spec buying with unicode
        // placeholders, and which half-block gets for nothing: a cell is a
        // glyph or it is picture, and whatever painted last decides.
        let mut frame = Frame::new(GridSize { cols: 2, rows: 2 });
        let samples = Samples { cols: 2, rows: 2, pixels: vec![Rgb { r: 9, g: 9, b: 9 }; 4] };
        frame.paint_samples(CellRect { col: 0, row: 0, cols: 2, rows: 1 }, &samples);
        frame.paint_text(CellPos { col: 0, row: 0 }, "a", Style::default());

        assert_eq!(frame.cell(CellPos { col: 0, row: 0 }).expect("label").ch, 'a');
        assert_eq!(frame.cell(CellPos { col: 1, row: 0 }).expect("picture").ch, '\u{2580}');
    }

    #[test]
    fn a_cell_with_no_bottom_sample_is_a_solid_block() {
        // An odd number of sample rows, which a resample can produce at
        // the bottom edge. Leaving the background unset would show the
        // terminal's own colour in a stripe across the last row.
        let mut frame = Frame::new(GridSize { cols: 1, rows: 1 });
        let samples = Samples { cols: 1, rows: 1, pixels: vec![Rgb { r: 5, g: 5, b: 5 }] };
        frame.paint_samples(CellRect { col: 0, row: 0, cols: 1, rows: 1 }, &samples);

        let cell = frame.cell(CellPos { col: 0, row: 0 }).expect("painted");
        assert_eq!(cell.style.fg, Rgb { r: 5, g: 5, b: 5 });
        assert_eq!(cell.style.bg, Some(Rgb { r: 5, g: 5, b: 5 }));
    }

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
    fn a_frame_carries_no_image_until_it_is_given_one() {
        let frame = Frame::new(GridSize { cols: 10, rows: 4 });
        assert_eq!(frame.image(), None);
    }

    #[test]
    fn an_image_survives_being_put_on_a_frame() {
        let mut frame = Frame::new(GridSize { cols: 10, rows: 4 });
        let image = Image {
            generation: 7,
            payload: std::sync::Arc::new("iVBOR".to_string()),
            area: CellRect { col: 0, row: 1, cols: 10, rows: 2 },
        };
        frame.set_image(Some(image.clone()));
        assert_eq!(frame.image(), Some(&image));
    }

    #[test]
    fn a_frame_with_an_image_still_paints_cells() {
        // Pixel mode leaves the page rows blank but the chrome rows are
        // cells like any other, so an image must not disturb painting.
        let mut f = Frame::new(vp().grid());
        f.set_image(Some(Image {
            generation: 1,
            payload: std::sync::Arc::new("AAAA".to_string()),
            area: CellRect::of(vp().grid(), 0),
        }));
        f.paint_run(&vp(), &run("hi", 0.0, 14.0, 20.0));
        assert_eq!(f.cell(CellPos { col: 0, row: 0 }).map(|c| c.ch), Some('h'));
        assert!(f.image().is_some());
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
        r.style = Style { fg: Rgb { r: 255, g: 0, b: 0 }, bg: None, bold: true, reverse: false };
        f.paint_run(&vp(), &r);
        let c = f.cell(CellPos { col: 0, row: 0 }).unwrap();
        assert_eq!(c.ch, 'h');
        assert_eq!(c.style.fg, Rgb { r: 255, g: 0, b: 0 });
        assert!(c.style.bold);
    }

    #[test]
    fn paint_text_writes_at_the_given_cell() {
        let mut f = Frame::new(GridSize { cols: 10, rows: 2 });
        f.paint_text(CellPos { col: 2, row: 1 }, "hi", Style::default());
        assert_eq!(f.row_text(1), "  hi");
        assert_eq!(f.row_text(0), "");
    }

    #[test]
    fn paint_text_clips_at_the_right_edge() {
        let mut f = Frame::new(GridSize { cols: 4, rows: 1 });
        f.paint_text(CellPos { col: 2, row: 0 }, "abcd", Style::default());
        assert_eq!(f.row_text(0), "  ab");
    }

    #[test]
    fn paint_text_off_the_bottom_is_a_no_op() {
        let mut f = Frame::new(GridSize { cols: 4, rows: 1 });
        f.paint_text(CellPos { col: 0, row: 5 }, "abcd", Style::default());
        assert_eq!(f.row_text(0), "");
    }

    #[test]
    fn paint_text_carries_its_style() {
        let mut f = Frame::new(GridSize { cols: 4, rows: 1 });
        let style = Style { fg: Rgb { r: 1, g: 2, b: 3 }, bg: None, bold: false, reverse: true };
        f.paint_text(CellPos { col: 0, row: 0 }, "x", style);
        assert_eq!(f.cell(CellPos { col: 0, row: 0 }).unwrap().style, style);
    }

    #[test]
    fn paint_text_outranks_any_page_run() {
        // Composition order: the page is painted first, then chrome over it.
        // Chrome must win even against the deepest run expressible, which is
        // what lets the compositor paint chrome without checking anything.
        let mut f = Frame::new(GridSize { cols: 4, rows: 1 });
        f.paint_run(
            &Viewport::new(GridSize { cols: 4, rows: 1 }, CellSize { w: 10, h: 20 }),
            &TextRun {
                text: "zz".to_string(),
                rect: CssRect { x: 0.0, y: 0.0, w: 40.0, h: 16.0 },
                baseline: 14.0,
                style: Style::default(),
                z: i32::MAX,
            },
        );
        f.paint_text(CellPos { col: 0, row: 0 }, "ab", Style::default());
        assert_eq!(f.row_text(0), "ab");
    }
    #[test]
    fn a_new_frame_has_no_cursor() {
        assert_eq!(Frame::new(GridSize { cols: 20, rows: 5 }).cursor(), None);
    }

    #[test]
    fn the_cursor_is_kept_where_it_was_put() {
        let mut f = Frame::new(GridSize { cols: 20, rows: 5 });
        f.set_cursor(Some(CellPos { col: 3, row: 2 }));
        assert_eq!(f.cursor(), Some(CellPos { col: 3, row: 2 }));
    }

    #[test]
    fn a_cursor_off_the_grid_is_no_cursor() {
        let mut f = Frame::new(GridSize { cols: 20, rows: 5 });
        f.set_cursor(Some(CellPos { col: 20, row: 0 }));
        assert_eq!(f.cursor(), None, "there is nothing at column 20 to sit on");
    }

    #[test]
    fn painting_a_list_of_runs_matches_painting_them_one_by_one() {
        let vp = Viewport::new(GridSize { cols: 20, rows: 5 }, CellSize { w: 10, h: 20 });
        let runs = vec![run("ab", 0.0, 16.0, 20.0), run("cd", 0.0, 36.0, 20.0)];

        let mut one = Frame::new(vp.grid());
        for r in &runs {
            one.paint_run(&vp, r);
        }
        let mut many = Frame::new(vp.grid());
        many.paint_runs(&vp, &runs);

        let rows = |f: &Frame| (0..5).map(|r| f.row_text(r)).collect::<Vec<_>>();
        assert_eq!(rows(&one), rows(&many));
    }
}
