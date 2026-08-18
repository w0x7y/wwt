//! Measuring the terminal, which decides the CSS viewport we hand Chromium.

use anyhow::{Context, Result};
use wm_frame::{CellSize, GridSize};

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
