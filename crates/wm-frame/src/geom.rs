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
