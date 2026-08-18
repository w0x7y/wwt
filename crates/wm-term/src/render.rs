//! Writing a `Frame` to a terminal.
//!
//! M1 repaints the whole grid every time. The diffing renderer that only
//! emits changed cells is M2; the signature here does not change when it
//! arrives.

use std::io::Write;

use wm_frame::{CellPos, Frame, Style};

/// Write the whole frame, leaving the terminal with default attributes.
pub fn render(frame: &Frame, out: &mut impl Write) -> std::io::Result<()> {
    let grid = frame.grid();
    // Home the cursor rather than clearing the screen: clearing first causes a
    // visible flash on every repaint.
    write!(out, "\x1b[H")?;

    let mut active: Option<Style> = None;
    for row in 0..grid.rows {
        if row > 0 {
            write!(out, "\r\n")?;
        }
        for col in 0..grid.cols {
            let cell = frame
                .cell(CellPos { col, row })
                .expect("cell within the frame's own grid");
            if active != Some(cell.style) {
                write_style(out, &cell.style)?;
                active = Some(cell.style);
            }
            let mut buf = [0u8; 4];
            out.write_all(cell.ch.encode_utf8(&mut buf).as_bytes())?;
        }
        // Erase anything the previous frame left beyond our last column.
        write!(out, "\x1b[K")?;
    }
    write!(out, "\x1b[0m")?;
    out.flush()
}

fn write_style(out: &mut impl Write, style: &Style) -> std::io::Result<()> {
    // Reset first so that clearing bold does not need a separate sequence.
    write!(out, "\x1b[0m")?;
    if style.bold {
        write!(out, "\x1b[1m")?;
    }
    write!(
        out,
        "\x1b[38;2;{};{};{}m",
        style.fg.r, style.fg.g, style.fg.b
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use wm_frame::{CellSize, CssRect, Frame, GridSize, Rgb, Style, TextRun, Viewport};

    fn vp() -> Viewport {
        Viewport::new(GridSize { cols: 10, rows: 2 }, CellSize { w: 10, h: 20 })
    }

    fn painted(text: &str, style: Style) -> Frame {
        let mut f = Frame::new(vp().grid());
        f.paint_run(
            &vp(),
            &TextRun {
                text: text.to_string(),
                rect: CssRect { x: 0.0, y: 0.0, w: 100.0, h: 16.0 },
                baseline: 14.0,
                style,
                z: 0,
            },
        );
        f
    }

    fn render_to_string(f: &Frame) -> String {
        let mut buf = Vec::new();
        render(f, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn render_homes_the_cursor_first() {
        let out = render_to_string(&Frame::new(GridSize { cols: 10, rows: 2 }));
        assert!(out.starts_with("\x1b[H"), "output was {out:?}");
    }

    #[test]
    fn render_emits_the_cell_text() {
        let out = render_to_string(&painted("hi", Style::default()));
        assert!(out.contains("hi"), "output was {out:?}");
    }

    #[test]
    fn render_clears_to_end_of_each_line() {
        let out = render_to_string(&painted("hi", Style::default()));
        assert_eq!(out.matches("\x1b[K").count(), 2, "one per row");
    }

    #[test]
    fn render_sets_truecolor_foreground() {
        let style = Style { fg: Rgb { r: 255, g: 128, b: 0 }, bold: false };
        let out = render_to_string(&painted("hi", style));
        assert!(out.contains("\x1b[38;2;255;128;0m"), "output was {out:?}");
    }

    #[test]
    fn render_sets_and_clears_bold() {
        let style = Style { fg: Rgb { r: 0, g: 0, b: 0 }, bold: true };
        let out = render_to_string(&painted("hi", style));
        assert!(out.contains("\x1b[1m"), "output was {out:?}");
        assert!(out.ends_with("\x1b[0m"), "output was {out:?}");
    }

    #[test]
    fn render_does_not_repeat_an_unchanged_style() {
        let style = Style { fg: Rgb { r: 10, g: 20, b: 30 }, bold: false };
        let out = render_to_string(&painted("hello", style));
        assert_eq!(
            out.matches("\x1b[38;2;10;20;30m").count(),
            1,
            "style should be emitted once for the run, not once per cell: {out:?}"
        );
    }

    #[test]
    fn render_separates_rows_with_newlines() {
        let out = render_to_string(&Frame::new(GridSize { cols: 4, rows: 3 }));
        assert_eq!(out.matches("\r\n").count(), 2, "no trailing newline");
    }
}
