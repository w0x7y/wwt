//! A picture of the page as colours, two to a cell.
//!
//! The other half of pixel mode. With a graphics protocol a frame is a
//! payload the terminal draws; without one it is this, and the cells the
//! terminal already knows how to draw.

use crate::cell::Rgb;

/// A grid of colours, one per half cell.
///
/// `rows` is therefore twice the cell rows it covers. Kept as its own type
/// rather than as a `Vec<Rgb>` and two numbers, because the indexing is
/// the only thing that can be wrong about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Samples {
    pub cols: u16,
    pub rows: u16,
    /// Row major, `cols * rows` long.
    pub pixels: Vec<Rgb>,
}

impl Samples {
    /// Box-filter an RGBA picture down to a sample grid.
    ///
    /// Averaging rather than sampling, because the source is deliberately
    /// larger than the target: Chromium preserves the source aspect ratio
    /// when it scales and the sample grid's aspect is a half cell, which
    /// is not square, so one axis always arrives with pixels to spare.
    /// Dropping them would be dropping most of the page's text.
    ///
    /// `None` when the picture is not the size it claims, or the grid is
    /// empty. Padding a short picture would put a black stripe on a real
    /// page and read as a rendering bug rather than as a bad frame.
    pub fn resampled(
        src_width: usize,
        src_height: usize,
        rgba: &[u8],
        cols: u16,
        rows: u16,
    ) -> Option<Self> {
        if cols == 0 || rows == 0 || src_width == 0 || src_height == 0 {
            return None;
        }
        if rgba.len() != src_width.checked_mul(src_height)?.checked_mul(4)? {
            return None;
        }

        let mut pixels = Vec::with_capacity(usize::from(cols) * usize::from(rows));
        for row in 0..usize::from(rows) {
            // Half-open source spans, so every source pixel belongs to
            // exactly one cell and none is counted twice.
            let top = row * src_height / usize::from(rows);
            let bottom = (((row + 1) * src_height) / usize::from(rows)).max(top + 1);
            for col in 0..usize::from(cols) {
                let left = col * src_width / usize::from(cols);
                let right = (((col + 1) * src_width) / usize::from(cols)).max(left + 1);

                let mut totals = [0u64; 3];
                let mut count = 0u64;
                for y in top..bottom.min(src_height) {
                    for x in left..right.min(src_width) {
                        let at = (y * src_width + x) * 4;
                        totals[0] += u64::from(rgba[at]);
                        totals[1] += u64::from(rgba[at + 1]);
                        totals[2] += u64::from(rgba[at + 2]);
                        count += 1;
                    }
                }
                // Alpha is ignored: a screencast frame is opaque, because
                // it is a picture of a window and not of a layer.
                let count = count.max(1);
                pixels.push(Rgb {
                    r: (totals[0] / count) as u8,
                    g: (totals[1] / count) as u8,
                    b: (totals[2] / count) as u8,
                });
            }
        }

        Some(Self { cols, rows, pixels })
    }

    pub fn at(&self, col: u16, row: u16) -> Option<Rgb> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        self.pixels
            .get(usize::from(row) * usize::from(self.cols) + usize::from(col))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red_and_blue() -> Vec<u8> {
        // 2x2 RGBA: red, blue on the first row; blue, red on the second.
        vec![
            255, 0, 0, 255, 0, 0, 255, 255, //
            0, 0, 255, 255, 255, 0, 0, 255,
        ]
    }

    #[test]
    fn a_picture_the_size_of_the_grid_is_copied_rather_than_averaged() {
        let samples = Samples::resampled(2, 2, &red_and_blue(), 2, 2).expect("same size");
        assert_eq!(samples.at(0, 0), Some(Rgb { r: 255, g: 0, b: 0 }));
        assert_eq!(samples.at(1, 0), Some(Rgb { r: 0, g: 0, b: 255 }));
        assert_eq!(samples.at(0, 1), Some(Rgb { r: 0, g: 0, b: 255 }));
        assert_eq!(samples.at(1, 1), Some(Rgb { r: 255, g: 0, b: 0 }));
    }

    #[test]
    fn shrinking_averages_every_source_pixel_that_lands_in_a_cell() {
        // The whole 2x2 into one sample: two red and two blue average to
        // half of each. A nearest-neighbour resize would answer 255,0,0
        // and throw away three quarters of the picture.
        let samples = Samples::resampled(2, 2, &red_and_blue(), 1, 1).expect("shrink");
        assert_eq!(samples.at(0, 0), Some(Rgb { r: 127, g: 0, b: 127 }));
    }

    #[test]
    fn a_picture_larger_than_the_grid_on_one_axis_still_covers_it() {
        // Chromium preserves the source aspect ratio when it scales, and
        // the sample grid's aspect is deliberately not the source's, so
        // one axis always arrives with pixels to spare. Asking for twice
        // the grid is what guarantees the other axis is not short.
        let rgba = vec![255u8; 8 * 3 * 4];
        let samples = Samples::resampled(8, 3, &rgba, 4, 2).expect("wide");
        assert_eq!(samples.cols, 4);
        assert_eq!(samples.rows, 2);
        assert_eq!(samples.at(3, 1), Some(Rgb { r: 255, g: 255, b: 255 }));
    }

    #[test]
    fn a_truncated_picture_is_refused_rather_than_padded() {
        // Three bytes short of a 2x2 RGBA image. Padding would put a black
        // stripe on a real page and look like a rendering bug.
        let mut rgba = red_and_blue();
        rgba.truncate(13);
        assert_eq!(Samples::resampled(2, 2, &rgba, 2, 2), None);
    }

    #[test]
    fn an_empty_grid_is_refused() {
        assert_eq!(Samples::resampled(2, 2, &red_and_blue(), 0, 2), None);
        assert_eq!(Samples::resampled(0, 0, &[], 2, 2), None);
    }
}
