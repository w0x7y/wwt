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
