//! An image on its way to the terminal, and the cells it covers.
//!
//! Data and nothing else. What the payload means and how it reaches a
//! terminal is `wwt-term`'s, because this crate knows about no terminal.

use crate::geom::GridSize;

/// A rectangle of cells, in frame coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRect {
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
}

impl CellRect {
    /// The rectangle a viewport's grid occupies at its origin row.
    pub fn of(grid: GridSize, origin_row: u16) -> Self {
        Self {
            col: 0,
            row: origin_row,
            cols: grid.cols,
            rows: grid.rows,
        }
    }
}

/// A picture of the page, as the terminal will be given it.
///
/// The payload is base64 and stays base64: it arrives from CDP encoded and
/// the graphics protocol wants it encoded, so nothing here ever decodes it.
///
/// `generation` is what a renderer diffs on. Comparing payloads would mean
/// comparing a megabyte of base64 per frame to answer a question a counter
/// answers, and two different frames can encode identically anyway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub generation: u64,
    pub payload: String,
    pub area: CellRect,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_images_area_is_the_page_grid_at_its_origin() {
        let grid = GridSize { cols: 80, rows: 22 };
        assert_eq!(
            CellRect::of(grid, 1),
            CellRect {
                col: 0,
                row: 1,
                cols: 80,
                rows: 22
            }
        );
    }

    #[test]
    fn two_frames_of_the_same_picture_differ_by_generation() {
        // The renderer's whole diffing rule rests on this: identical bytes
        // with a new generation is a new frame and must be sent again.
        let area = CellRect::of(GridSize { cols: 4, rows: 2 }, 1);
        let first = Image {
            generation: 1,
            payload: "AAAA".into(),
            area,
        };
        let second = Image {
            generation: 2,
            payload: "AAAA".into(),
            area,
        };
        assert_ne!(first, second);
    }
}
