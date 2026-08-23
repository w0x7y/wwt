/// A 24-bit color. Terminals that cannot do truecolor are M5's problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub fg: Rgb,
    /// The cell's own background, or the terminal's when there is none.
    ///
    /// Extraction never produces one: a page painted over whatever theme
    /// the terminal has is what makes text mode look like the terminal it
    /// is in rather than like a browser pretending to be one. Half-block
    /// is the one thing that sets it, because half a cell is a foreground
    /// and a background and there is no third way to say that.
    pub bg: Option<Rgb>,
    pub bold: bool,
    /// Swap foreground and background. Chrome uses this.
    pub reverse: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fg: Rgb { r: 0xd0, g: 0xd0, b: 0xd0 },
            bg: None,
            bold: false,
            reverse: false,
        }
    }
}

/// One terminal cell.
///
/// `z` is the stacking depth of whatever painted this cell, retained so a
/// later run can decide whether it is allowed to overwrite it. See Task 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
    pub z: i32,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            style: Style::default(),
            z: i32::MIN,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_style_has_no_background_unless_it_is_given_one() {
        // Text mode never sets one. A run is a foreground colour on
        // whatever the terminal's own background is, and that is what
        // makes a page painted over a user's theme look like their theme.
        assert_eq!(Style::default().bg, None);
    }
}
