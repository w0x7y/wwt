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
        let top = i64::from(vp.origin_row());
        let last_col = i64::from(grid.cols.saturating_sub(1));
        let last_row = top + i64::from(grid.rows.saturating_sub(1));
        CellPos {
            col: vp.col_of(self.rect.x).clamp(0, last_col) as u16,
            row: vp.row_of(self.rect.y).clamp(top, last_row) as u16,
        }
    }
}

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

    #[test]
    fn a_label_clamps_into_the_page_rather_than_onto_the_chrome() {
        let vp = Viewport::with_origin(
            GridSize { cols: 80, rows: 22 },
            CellSize { w: 9, h: 20 },
            1,
        );
        // A box above the viewport keeps a reachable label, but it must not
        // land on the tab bar, which the page does not own.
        let t = HintTarget {
            rect: CssRect { x: -500.0, y: -500.0, w: 40.0, h: 20.0 },
            kind: TargetKind::Clickable,
        };
        assert_eq!(t.label_cell(&vp), CellPos { col: 0, row: 1 });

        let t = HintTarget {
            rect: CssRect { x: 100_000.0, y: 100_000.0, w: 40.0, h: 20.0 },
            kind: TargetKind::Clickable,
        };
        assert_eq!(t.label_cell(&vp), CellPos { col: 79, row: 22 });
    }
}
