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
    pub bold: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fg: Rgb { r: 0xd0, g: 0xd0, b: 0xd0 },
            bold: false,
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
