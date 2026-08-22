//! Writing a `Frame` to a terminal.
//!
//! Two ways to write one: `render` puts down the whole grid, and `Renderer`
//! holds the last frame and emits only the cells that differ from it. The
//! full repaint is not dead code behind the diff, it is what the diff falls
//! back to when the grid itself changed, and there is nothing to diff
//! against.

use std::io::Write;

use wwt_frame::{CellPos, CellRect, Frame, Image, Style};

use crate::graphics::protocol;

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

/// A steady vertical bar (DECSCUSR 6): a caret between two characters
/// rather than one covering a character. Terminals that do not understand
/// it leave their own cursor shape alone, which is still a cursor in the
/// right cell.
const BAR_CURSOR: &str = "\x1b[6 q";

/// Put the terminal's own cursor where the frame wants it, or take it off
/// the screen. Terminal coordinates are 1-based.
///
/// This is the caret. Inverting a cell would make it as wide as a
/// character, and would hide whether the insertion point is before or after
/// the character it lands on.
fn place_cursor(frame: &Frame, out: &mut impl Write) -> std::io::Result<()> {
    match frame.cursor() {
        Some(pos) => write!(out, "\x1b[{};{}H{BAR_CURSOR}\x1b[?25h", pos.row + 1, pos.col + 1),
        None => write!(out, "\x1b[?25l"),
    }
}

fn write_style(out: &mut impl Write, style: &Style) -> std::io::Result<()> {
    // Reset first so that clearing bold does not need a separate sequence.
    write!(out, "\x1b[0m")?;
    if style.bold {
        write!(out, "\x1b[1m")?;
    }
    if style.reverse {
        write!(out, "\x1b[7m")?;
    }
    write!(
        out,
        "\x1b[38;2;{};{};{}m",
        style.fg.r, style.fg.g, style.fg.b
    )
}

/// A renderer that remembers what it last put on screen.
///
/// A page where one counter ticks costs a handful of bytes per update
/// rather than a full repaint, which is the difference between a browser
/// that is pleasant on a slow link and one that is not.
#[derive(Debug, Default)]
pub struct Renderer {
    last: Option<Frame>,
    /// What the terminal is currently holding: the generation on screen and
    /// the area it covers. A frame that changed neither costs no sequence,
    /// and a frame that changed only its data costs no cells.
    shown: Option<(u64, CellRect)>,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            last: None,
            shown: None,
        }
    }

    /// Discard the cached frame, so the next render repaints everything.
    /// Used after a resize, and after anything else writes to the terminal
    /// behind our back.
    pub fn invalidate(&mut self) {
        self.last = None;
        // The terminal has been written to behind our back, so nothing can
        // be assumed to still be placed where we left it.
        self.shown = None;
    }

    pub fn render(&mut self, frame: &Frame, out: &mut impl Write) -> std::io::Result<()> {
        let reusable = self
            .last
            .as_ref()
            .is_some_and(|prev| prev.grid() == frame.grid());

        let wrote = if reusable {
            self.diff(frame, out)?
        } else {
            // A diff against a frame of different dimensions is meaningless.
            render(frame, out)?;
            true
        };

        // Cells first, image second. The transmission is what the
        // placeholders on screen re-render against, so sending it first
        // would show this frame's picture through the last frame's cells
        // for one paint.
        let touched_image = self.paint_image(frame.image(), out)?;

        // Writing cells leaves the terminal's cursor wherever the last cell
        // went, so anything painted has to be followed by putting it back.
        let moved = self.last.as_ref().map(Frame::cursor) != Some(frame.cursor());
        if wrote || moved || touched_image {
            place_cursor(frame, out)?;
            out.flush()?;
        }

        self.last = Some(frame.clone());
        Ok(())
    }

    /// Bring the terminal's idea of the image up to date with this frame's.
    ///
    /// Returns whether anything was written. Three cases, and they are the
    /// whole protocol policy: no image is a delete if one was showing, an
    /// unchanged generation is nothing at all, and a new generation is a
    /// transmission and a placement, plus the cells addressing it only when
    /// the area moved.
    ///
    /// The placement is not optional. Transmitting to an id that already has
    /// one destroys it, and the cells addressing it then show the terminal's
    /// background until it is re-issued. That was measured rather than
    /// assumed; see the M5 spec, section 4.
    fn paint_image(&mut self, image: Option<&Image>, out: &mut impl Write) -> std::io::Result<bool> {
        let Some(image) = image else {
            if self.shown.take().is_some() {
                protocol::delete(out)?;
                return Ok(true);
            }
            return Ok(false);
        };

        if self.shown == Some((image.generation, image.area)) {
            return Ok(false);
        }

        let moved = self.shown.map(|(_, area)| area) != Some(image.area);
        protocol::transmit(&image.payload, out)?;
        protocol::place(image.area, out)?;
        // Only when the area moved. A scroll re-renders the placeholders
        // already on screen, which is what keeps a frame from costing a
        // repaint.
        if moved {
            protocol::placeholders(image.area, out)?;
        }
        self.shown = Some((image.generation, image.area));
        Ok(true)
    }

    /// Emit the cells that changed. Returns whether anything was written.
    fn diff(&self, frame: &Frame, out: &mut impl Write) -> std::io::Result<bool> {
        let prev = self.last.as_ref().expect("diff runs only with a cached frame");
        let grid = frame.grid();
        let mut wrote = false;

        for row in 0..grid.rows {
            let mut col = 0;
            while col < grid.cols {
                let pos = CellPos { col, row };
                if frame.cell(pos) == prev.cell(pos) {
                    col += 1;
                    continue;
                }

                // Address the start of this changed segment. Terminal
                // coordinates are 1-based.
                write!(out, "\x1b[{};{}H", row + 1, col + 1)?;
                wrote = true;

                let mut active: Option<Style> = None;
                while col < grid.cols {
                    let pos = CellPos { col, row };
                    let cell = frame.cell(pos).expect("cell within the frame's own grid");
                    if Some(cell) == prev.cell(pos) {
                        break;
                    }
                    if active != Some(cell.style) {
                        write_style(out, &cell.style)?;
                        active = Some(cell.style);
                    }
                    let mut buf = [0u8; 4];
                    out.write_all(cell.ch.encode_utf8(&mut buf).as_bytes())?;
                    col += 1;
                }
                write!(out, "\x1b[0m")?;
            }
        }

        Ok(wrote)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wwt_frame::{
        CellRect, CellSize, CssRect, Frame, GridSize, Image, Rgb, Style, TextRun, Viewport,
    };

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

    fn image_at(generation: u64, payload: &str) -> Image {
        Image {
            generation,
            payload: payload.to_string(),
            area: CellRect { col: 0, row: 1, cols: 4, rows: 2 },
        }
    }

    fn framed(image: Option<Image>) -> Frame {
        let mut frame = Frame::new(GridSize { cols: 4, rows: 4 });
        frame.set_image(image);
        frame
    }

    fn rendered(renderer: &mut Renderer, frame: &Frame) -> String {
        let mut out = Vec::new();
        renderer.render(frame, &mut out).expect("render");
        String::from_utf8(out).expect("utf-8")
    }

    #[test]
    fn the_first_image_is_transmitted_placed_and_given_placeholders() {
        let mut renderer = Renderer::new();
        let sent = rendered(&mut renderer, &framed(Some(image_at(1, "AAAA"))));

        assert!(sent.contains("a=t,f=100"), "transmitted");
        assert!(sent.contains("a=p,U=1"), "placed");
        assert!(
            sent.contains(protocol::PLACEHOLDER),
            "the cells addressing it were written"
        );
    }

    #[test]
    fn a_new_frame_is_a_transmission_and_a_placement_and_no_cells() {
        // The latency claim of pixel mode. The placement is re-issued
        // because transmitting destroys it, which was measured against a
        // real Kitty, but not one placeholder cell is touched.
        let mut renderer = Renderer::new();
        rendered(&mut renderer, &framed(Some(image_at(1, "AAAA"))));

        let sent = rendered(&mut renderer, &framed(Some(image_at(2, "BBBB"))));
        assert!(sent.contains("BBBB"), "the new data went out");
        assert!(sent.contains("a=p,U=1"), "and was placed again");
        assert!(
            !sent.contains(protocol::PLACEHOLDER),
            "but no cell was rewritten"
        );
    }

    #[test]
    fn an_unchanged_generation_sends_no_image_at_all() {
        let mut renderer = Renderer::new();
        let frame = framed(Some(image_at(1, "AAAA")));
        rendered(&mut renderer, &frame);

        let sent = rendered(&mut renderer, &frame);
        assert!(!sent.contains("\x1b_G"), "nothing about graphics was said");
    }

    #[test]
    fn a_changed_area_writes_placeholders_again() {
        // A resize. The placement covers a different number of cells, so the
        // cells referring to it have to be laid down again.
        let mut renderer = Renderer::new();
        rendered(&mut renderer, &framed(Some(image_at(1, "AAAA"))));

        let mut wider = Frame::new(GridSize { cols: 8, rows: 6 });
        let mut image = image_at(2, "BBBB");
        image.area = CellRect { col: 0, row: 1, cols: 8, rows: 4 };
        wider.set_image(Some(image));

        let sent = rendered(&mut renderer, &wider);
        assert!(sent.contains("a=p,U=1"), "re-placed at the new size");
        assert!(sent.contains(protocol::PLACEHOLDER), "and re-addressed");
    }

    #[test]
    fn dropping_the_image_deletes_it_from_the_terminal() {
        // Leaving pixel mode. Nothing is left in the terminal's memory for a
        // mode nobody is in.
        let mut renderer = Renderer::new();
        rendered(&mut renderer, &framed(Some(image_at(1, "AAAA"))));

        let sent = rendered(&mut renderer, &framed(None));
        assert!(sent.contains("a=d,d=i"));
    }

    #[test]
    fn a_text_frame_says_nothing_about_graphics() {
        // Text mode is every frame this codebase built before M5 and must
        // cost exactly what it cost then.
        let mut renderer = Renderer::new();
        let sent = rendered(&mut renderer, &framed(None));
        assert!(!sent.contains("\x1b_G"));
    }

    #[test]
    fn invalidating_forgets_what_the_terminal_was_holding() {
        // After a resize the terminal has been written to behind our back,
        // so the next frame must place and address the image again rather
        // than assume it survived.
        let mut renderer = Renderer::new();
        let frame = framed(Some(image_at(1, "AAAA")));
        rendered(&mut renderer, &frame);
        renderer.invalidate();

        let sent = rendered(&mut renderer, &frame);
        assert!(sent.contains("a=t,f=100"), "transmitted again");
        assert!(sent.contains(protocol::PLACEHOLDER), "and addressed again");
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
        let style = Style { fg: Rgb { r: 255, g: 128, b: 0 }, bold: false, reverse: false };
        let out = render_to_string(&painted("hi", style));
        assert!(out.contains("\x1b[38;2;255;128;0m"), "output was {out:?}");
    }

    #[test]
    fn render_sets_and_clears_bold() {
        let style = Style { fg: Rgb { r: 0, g: 0, b: 0 }, bold: true, reverse: false };
        let out = render_to_string(&painted("hi", style));
        assert!(out.contains("\x1b[1m"), "output was {out:?}");
        assert!(out.ends_with("\x1b[0m"), "output was {out:?}");
    }

    #[test]
    fn render_does_not_repeat_an_unchanged_style() {
        let style = Style { fg: Rgb { r: 10, g: 20, b: 30 }, bold: false, reverse: false };
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

    fn diff_to_string(r: &mut Renderer, f: &Frame) -> String {
        let mut buf = Vec::new();
        r.render(f, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn renderer_paints_the_first_frame_in_full() {
        let mut r = Renderer::new();
        let out = diff_to_string(&mut r, &painted("hi", Style::default()));
        assert!(out.starts_with("\x1b[H"), "output was {out:?}");
        assert!(out.contains("hi"), "output was {out:?}");
    }

    #[test]
    fn renderer_emits_nothing_for_an_unchanged_frame() {
        let mut r = Renderer::new();
        let f = painted("hi", Style::default());
        diff_to_string(&mut r, &f);
        assert_eq!(diff_to_string(&mut r, &f), "");
    }

    #[test]
    fn renderer_emits_only_the_changed_cell() {
        let mut r = Renderer::new();
        diff_to_string(&mut r, &painted("hi", Style::default()));
        let out = diff_to_string(&mut r, &painted("ho", Style::default()));

        // Row 0, column 2 in 1-based terminal coordinates.
        assert!(out.contains("\x1b[1;2H"), "output was {out:?}");
        assert!(out.contains('o'), "output was {out:?}");
        assert!(!out.contains('h'), "the unchanged cell was repainted: {out:?}");
    }

    #[test]
    fn renderer_repaints_in_full_when_the_grid_changes() {
        let mut r = Renderer::new();
        diff_to_string(&mut r, &Frame::new(GridSize { cols: 10, rows: 2 }));
        let out = diff_to_string(&mut r, &Frame::new(GridSize { cols: 12, rows: 3 }));
        assert!(out.starts_with("\x1b[H"), "output was {out:?}");
    }

    #[test]
    fn invalidate_forces_the_next_frame_to_repaint_in_full() {
        let mut r = Renderer::new();
        let f = painted("hi", Style::default());
        diff_to_string(&mut r, &f);
        r.invalidate();
        assert!(diff_to_string(&mut r, &f).starts_with("\x1b[H"));
    }

    #[test]
    fn render_sets_reverse_video() {
        let style = Style { fg: Rgb { r: 0, g: 0, b: 0 }, bold: false, reverse: true };
        let out = render_to_string(&painted("hi", style));
        assert!(out.contains("\x1b[7m"), "output was {out:?}");
    }

    #[test]
    fn the_renderer_puts_the_terminal_cursor_on_the_caret_cell() {
        let mut r = Renderer::new();
        let mut f = painted("hi", Style::default());
        f.set_cursor(Some(CellPos { col: 2, row: 0 }));
        let out = diff_to_string(&mut r, &f);
        // Row 1, column 3 in 1-based terminal coordinates, then a bar.
        assert!(out.contains("\x1b[1;3H\x1b[6 q\x1b[?25h"), "output was {out:?}");
    }

    #[test]
    fn the_renderer_hides_the_cursor_when_the_frame_has_no_caret() {
        let mut r = Renderer::new();
        let out = diff_to_string(&mut r, &painted("hi", Style::default()));
        assert!(out.contains("\x1b[?25l"), "output was {out:?}");
    }

    #[test]
    fn a_caret_that_moved_is_emitted_though_no_cell_changed() {
        let mut r = Renderer::new();
        let mut f = painted("hi", Style::default());
        f.set_cursor(Some(CellPos { col: 0, row: 0 }));
        diff_to_string(&mut r, &f);

        f.set_cursor(Some(CellPos { col: 1, row: 0 }));
        let out = diff_to_string(&mut r, &f);
        assert!(out.contains("\x1b[1;2H"), "output was {out:?}");
    }

    #[test]
    fn a_caret_that_stayed_put_costs_nothing() {
        let mut r = Renderer::new();
        let mut f = painted("hi", Style::default());
        f.set_cursor(Some(CellPos { col: 1, row: 0 }));
        diff_to_string(&mut r, &f);
        assert_eq!(diff_to_string(&mut r, &f), "");
    }
}
