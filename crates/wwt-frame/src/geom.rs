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
///
/// `grid` is the *page's* size in cells, which is the terminal less the rows
/// the chrome occupies. `origin_row` is the frame row the page's first row
/// lands on, so a conversion out of CSS gives a row you can paint at and a
/// conversion into CSS takes one. The page is never told either: how big it
/// is has nothing to do with where it sits on our screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    grid: GridSize,
    cell: CellSize,
    origin_row: u16,
}

impl Viewport {
    pub fn new(grid: GridSize, cell: CellSize) -> Self {
        Self::with_origin(grid, cell, 0)
    }

    pub fn with_origin(grid: GridSize, cell: CellSize, origin_row: u16) -> Self {
        assert!(cell.w > 0 && cell.h > 0, "cell size must be non-zero");
        Self {
            grid,
            cell,
            origin_row,
        }
    }

    pub fn grid(&self) -> GridSize {
        self.grid
    }

    pub fn cell(&self) -> CellSize {
        self.cell
    }

    /// The frame row the page's first row is painted on.
    pub fn origin_row(&self) -> u16 {
        self.origin_row
    }

    /// The viewport width in CSS pixels — what Chromium is told the window is.
    pub fn css_width(&self) -> u32 {
        u32::from(self.grid.cols) * u32::from(self.cell.w)
    }

    /// The viewport height in CSS pixels.
    pub fn css_height(&self) -> u32 {
        u32::from(self.grid.rows) * u32::from(self.cell.h)
    }

    /// The CSS point at the *center* of a frame cell. Center rather than
    /// corner so that dispatching a click at this point lands unambiguously
    /// inside the cell, and so the roundtrip below is exact.
    ///
    /// A row above the origin is chrome, and the point this returns for one
    /// is above the page, which is what makes `to_cell` refuse it.
    pub fn to_css(&self, c: CellPos) -> CssPoint {
        let page_row = f64::from(c.row) - f64::from(self.origin_row);
        CssPoint {
            x: (f64::from(c.col) + 0.5) * f64::from(self.cell.w),
            y: (page_row + 0.5) * f64::from(self.cell.h),
        }
    }

    /// The frame cell containing a CSS point, or `None` if it falls outside
    /// the page.
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
            row: row as u16 + self.origin_row,
        })
    }

    /// The column a CSS x-coordinate falls in, unclamped by the grid's right
    /// edge. Painting uses this so a run starting off-screen still places its
    /// visible tail correctly.
    pub fn col_of(&self, x: f64) -> i64 {
        (x / f64::from(self.cell.w)).floor() as i64
    }

    /// The frame row a CSS y-coordinate falls in, unclamped.
    pub fn row_of(&self, y: f64) -> i64 {
        (y / f64::from(self.cell.h)).floor() as i64 + i64::from(self.origin_row)
    }
}

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

    fn offset_vp(cols: u16, rows: u16, w: u16, h: u16, origin: u16) -> Viewport {
        Viewport::with_origin(GridSize { cols, rows }, CellSize { w, h }, origin)
    }

    #[test]
    fn an_origin_row_moves_the_page_down_the_screen_without_resizing_it() {
        let v = offset_vp(80, 22, 9, 20, 1);
        // The page's own size is what Chromium is told, and it is unaffected
        // by where the page sits on our screen.
        assert_eq!(v.css_height(), 440);
        assert_eq!(v.origin_row(), 1);
    }

    #[test]
    fn the_top_of_the_page_lands_one_row_below_the_top_of_the_frame() {
        let v = offset_vp(80, 22, 9, 20, 1);
        assert_eq!(
            v.to_cell(CssPoint { x: 0.0, y: 0.0 }),
            Some(CellPos { col: 0, row: 1 })
        );
        assert_eq!(v.row_of(0.0), 1);
    }

    #[test]
    fn a_frame_row_above_the_origin_is_not_part_of_the_page() {
        let v = offset_vp(80, 22, 9, 20, 1);
        // Row 0 is the tab bar. Asking for its CSS position gives a point
        // above the page, and no CSS point maps back to it.
        assert!(v.to_css(CellPos { col: 0, row: 0 }).y < 0.0);
        assert_eq!(v.to_cell(CssPoint { x: 0.0, y: -1.0 }), None);
    }

    #[test]
    fn a_point_below_the_last_page_row_is_off_the_page() {
        let v = offset_vp(80, 22, 9, 20, 1);
        // 22 rows of 20px is 440; the last page row is frame row 22.
        assert_eq!(
            v.to_cell(CssPoint { x: 0.0, y: 439.0 }),
            Some(CellPos { col: 0, row: 22 })
        );
        assert_eq!(v.to_cell(CssPoint { x: 0.0, y: 440.0 }), None);
    }

    /// The property from spec section 3, now over origins as well. This is
    /// the one that must not be allowed to fail.
    #[test]
    fn cell_css_cell_roundtrip_is_identity_at_every_origin() {
        for origin in [0u16, 1, 2, 7] {
            for (w, h) in [(8u16, 16u16), (9, 20), (12, 26), (1, 1)] {
                let v = offset_vp(180, 46, w, h, origin);
                for page_row in 0..v.grid().rows {
                    for col in 0..v.grid().cols {
                        let c = CellPos { col, row: page_row + origin };
                        assert_eq!(
                            v.to_cell(v.to_css(c)),
                            Some(c),
                            "roundtrip failed at {c:?}, cell {w}x{h}, origin {origin}"
                        );
                    }
                }
            }
        }
    }
}
