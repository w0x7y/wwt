use crate::geom::{CellPos, Viewport};

/// Where typing would land, as the page measured it.
///
/// Not a point. A caret measured in CSS pixels lands on the wrong cell,
/// because the frame re-lays text out at one character per cell: a run
/// starts at the column its box starts in and then advances one cell per
/// character, whatever the font's real advance was. Two thirds of the way
/// along a proportional word is nowhere near two thirds of the way along
/// the cells it was painted into.
///
/// So the caret is reported the way the text around it is painted: the line
/// it sits on, and how many characters into that line it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Caret {
    /// The left edge of the line the caret is on, in CSS pixels: the same
    /// coordinate the run painting that line starts from.
    pub x: f64,
    /// That line's baseline, so the caret picks the row its text picked.
    pub baseline: f64,
    /// Characters into the line, which is how many cells past its first.
    pub offset: usize,
}

impl Caret {
    /// The cell the insertion point sits in, or `None` when it is off the
    /// page: scrolled out of view, or past the last column.
    pub fn cell(&self, vp: &Viewport) -> Option<CellPos> {
        let row = vp.row_of(self.baseline);
        let col = vp.col_of(self.x) + i64::try_from(self.offset).ok()?;
        let grid = vp.grid();
        let on_grid =
            row >= 0 && row < i64::from(grid.rows) && col >= 0 && col < i64::from(grid.cols);
        on_grid.then_some(CellPos { col: col as u16, row: row as u16 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{CellSize, GridSize};

    fn vp() -> Viewport {
        Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 })
    }

    #[test]
    fn the_caret_lands_on_the_row_its_line_is_on() {
        let caret = Caret { x: 90.0, baseline: 56.0, offset: 0 };
        assert_eq!(caret.cell(&vp()), Some(CellPos { col: 10, row: 2 }));
    }

    #[test]
    fn the_caret_counts_cells_the_way_the_text_was_painted() {
        // Four characters into a line, whatever those four characters
        // measured in CSS pixels: the frame gave each one a cell.
        let caret = Caret { x: 90.0, baseline: 16.0, offset: 4 };
        assert_eq!(caret.cell(&vp()), Some(CellPos { col: 14, row: 0 }));
    }

    #[test]
    fn a_caret_scrolled_off_the_page_has_no_cell() {
        let caret = Caret { x: 90.0, baseline: -100.0, offset: 0 };
        assert_eq!(caret.cell(&vp()), None, "nothing above the viewport has a cell");
    }

    #[test]
    fn a_caret_past_the_last_column_has_no_cell() {
        let caret = Caret { x: 700.0, baseline: 16.0, offset: 20 };
        assert_eq!(caret.cell(&vp()), None, "column 97 of an 80 column grid");
    }
}
