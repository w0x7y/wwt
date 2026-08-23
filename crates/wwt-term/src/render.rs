//! Writing a `Frame` to a terminal.
//!
//! Two ways to write one: `render` puts down the whole grid, and `Renderer`
//! holds the last frame and emits only the cells that differ from it. The
//! full repaint is not dead code behind the diff, it is what the diff falls
//! back to when the grid itself changed, and there is nothing to diff
//! against.

use std::io::Write;

use wwt_frame::{Cell, CellPos, CellRect, Frame, Image, Style};

use crate::graphics::protocol;

/// Write the whole frame, leaving the terminal with default attributes.
pub fn render(frame: &Frame, id: u32, out: &mut impl Write) -> std::io::Result<()> {
    let grid = frame.grid();
    // Home the cursor rather than clearing the screen: clearing first causes a
    // visible flash on every repaint.
    write!(out, "\x1b[H")?;

    let mut active: Option<Style> = None;
    // One line assembled and written per row, rather than a write per cell:
    // a placeholder is three codepoints and a row is a hundred cells.
    let mut line = String::new();
    for row in 0..grid.rows {
        if row > 0 {
            write!(out, "\r\n")?;
        }
        line.clear();
        for col in 0..grid.cols {
            let pos = CellPos { col, row };
            let cell = *frame.cell(pos).expect("cell within the frame's own grid");
            active = write_cell(frame, pos, &cell, active, id, &mut line);
        }
        out.write_all(line.as_bytes())?;
        // Erase anything the previous frame left beyond our last column.
        write!(out, "\x1b[K")?;
    }
    write!(out, "\x1b[0m")?;
    out.flush()
}

/// Write one cell, as a glyph or as the picture behind it.
///
/// A blank cell inside the image's area is where the picture shows through,
/// and anything else is a glyph that wins over it. That is the whole of "the
/// grid wins over the image", and putting it here rather than in a pass of
/// its own is what makes a label appearing and a label going away the same
/// event: both are a cell that changed, and the diff already writes those.
///
/// Returns the style now active, which is `None` after a placeholder,
/// because a placeholder sets a foreground of its own.
fn write_cell(
    frame: &Frame,
    pos: CellPos,
    cell: &Cell,
    active: Option<Style>,
    id: u32,
    line: &mut String,
) -> Option<Style> {
    if cell.ch == ' '
        && let Some(image) = frame.image()
        && let Some((row, col)) = within(image.area, pos)
    {
        protocol::fg(id, line);
        if protocol::placeholder(row, col, line) {
            return None;
        }
        // Past what the diacritic table can address. Fall through and write
        // the blank, so the cell is at least the colour it should be.
    }

    let active = if active == Some(cell.style) {
        active
    } else {
        push_style(line, &cell.style);
        Some(cell.style)
    };
    line.push(cell.ch);
    active
}

/// Where `pos` sits inside `area`, if it does at all.
fn within(area: CellRect, pos: CellPos) -> Option<(u16, u16)> {
    let row = pos.row.checked_sub(area.row).filter(|r| *r < area.rows)?;
    let col = pos.col.checked_sub(area.col).filter(|c| *c < area.cols)?;
    Some((row, col))
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

/// What a style looks like, pushed onto a line being assembled.
///
/// Pushing cannot fail, so the per-cell paths never handle an error, and
/// there is still one place that knows the sequences.
fn push_style(out: &mut String, style: &Style) {
    use std::fmt::Write as _;
    // Reset first so that clearing bold does not need a separate sequence.
    out.push_str("\x1b[0m");
    if style.bold {
        out.push_str("\x1b[1m");
    }
    if style.reverse {
        out.push_str("\x1b[7m");
    }
    let _ = write!(out, "\x1b[38;2;{};{};{}m", style.fg.r, style.fg.g, style.fg.b);
    // Only when there is one. The reset at the top of this function is
    // what clears a background the previous cell set, so there is no
    // "\x1b[49m" branch to forget.
    if let Some(bg) = style.bg {
        let _ = write!(out, "\x1b[48;2;{};{};{}m", bg.r, bg.g, bg.b);
    }
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
    /// the area it covers. A frame that changed neither costs no sequence.
    shown: Option<(u64, CellRect)>,
    /// Which of the two image ids is the one on screen. The other is where
    /// the next picture is assembled while this one is still being looked
    /// at. See `protocol::IMAGE_IDS`.
    slot: usize,
    /// One row of placeholders, built here and written in one go.
    ///
    /// A cell is three codepoints and a row is a hundred cells, so writing
    /// them as they are produced is thousands of small writes a frame, each
    /// one a capacity check to copy two or three bytes. Kept on the renderer
    /// rather than made per row, so a frame allocates nothing.
    scratch: String,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            last: None,
            shown: None,
            slot: 0,
            scratch: String::new(),
        }
    }

    /// The image id the cells on screen are pointing at.
    fn current_id(&self) -> u32 {
        protocol::IMAGE_IDS[self.slot]
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
        // The image appearing, going away, or moving is a repaint for the
        // same reason a changed grid is: the cells that show the picture
        // through are decided per cell, and a diff only writes the cells
        // that changed. Entering pixel mode over an already-blank page
        // changes no cell, so without this nothing would lay the
        // placeholders down and the picture would never appear. It happens
        // on `p`, on a resize and on a switch, and never on a frame.
        let area_of = |frame: &Frame| frame.image().map(|image| image.area);
        let reusable = self.last.as_ref().is_some_and(|prev| {
            prev.grid() == frame.grid() && area_of(prev) == area_of(frame)
        });

        let wrote = if reusable {
            self.diff(frame, out)?
        } else {
            // A diff against a frame of different dimensions is meaningless.
            render(frame, self.current_id(), out)?;
            true
        };

        // Cells first, image second. The transmission is what the
        // placeholders on screen re-render against, so sending it first
        // would show this frame's picture through the last frame's cells
        // for one paint.
        let touched_image = self.paint_image(frame, frame.image(), out)?;

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
    fn paint_image(
        &mut self,
        frame: &Frame,
        image: Option<&Image>,
        out: &mut impl Write,
    ) -> std::io::Result<bool> {
        let Some(image) = image else {
            if self.shown.take().is_some() {
                protocol::delete(protocol::IMAGE_IDS[self.slot], out)?;
                return Ok(true);
            }
            return Ok(false);
        };

        if self.shown == Some((image.generation, image.area)) {
            return Ok(false);
        }

        // Into the slot that is not on screen, and only then onto the
        // screen. Transmitting to an id tears down its placement for as long
        // as the transmission lasts, and a full-page PNG is dozens of
        // chunks: aimed at the id being looked at, that is the flicker.
        let showing = protocol::IMAGE_IDS[self.slot];
        let next = protocol::IMAGE_IDS[1 - self.slot];

        protocol::transmit(&image.payload, next, out)?;
        protocol::place(image.area, next, out)?;
        // Now the cells can be pointed at it. Every blank cell in the area,
        // because a glyph in there is an overlay and wins.
        self.placeholders(frame, image.area, next, out)?;
        // Last, so nothing is ever without a picture to show.
        if self.shown.is_some() {
            protocol::delete(showing, out)?;
        }

        self.slot = 1 - self.slot;
        self.shown = Some((image.generation, image.area));
        Ok(true)
    }

    /// Point every cell the picture shows through at `id`.
    ///
    /// Skips any cell carrying a glyph: that is an overlay, and the grid
    /// wins over the image. Costs a rewrite of the page area per frame,
    /// which the alternating ids make unavoidable, since a cell says which
    /// image it belongs to in its foreground colour.
    fn placeholders(
        &mut self,
        frame: &Frame,
        area: CellRect,
        id: u32,
        out: &mut impl Write,
    ) -> std::io::Result<()> {
        for row in 0..area.rows {
            // Built whole and written once. Three codepoints a cell and a
            // hundred cells a row is thousands of small writes a frame
            // otherwise, each of them a capacity check to copy two bytes.
            let line = &mut self.scratch;
            line.clear();
            protocol::fg(id, line);

            let mut pointing = true;
            for col in 0..area.cols {
                let pos = CellPos {
                    col: area.col + col,
                    row: area.row + row,
                };
                let Some(cell) = frame.cell(pos) else { break };
                if cell.ch != ' ' {
                    // An overlay. The grid wins over the image, and the
                    // foreground stops being the image's.
                    push_style(line, &cell.style);
                    line.push(cell.ch);
                    pointing = false;
                    continue;
                }
                if !pointing {
                    protocol::fg(id, line);
                    pointing = true;
                }
                if !protocol::placeholder(row, col, line) {
                    break;
                }
            }

            write!(out, "\x1b[{};{}H", area.row + row + 1, area.col + 1)?;
            out.write_all(line.as_bytes())?;
        }
        write!(out, "\x1b[0m")
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
                let mut line = String::new();
                while col < grid.cols {
                    let pos = CellPos { col, row };
                    let cell = *frame.cell(pos).expect("cell within the frame's own grid");
                    if Some(&cell) == prev.cell(pos) {
                        break;
                    }
                    active = write_cell(frame, pos, &cell, active, self.current_id(), &mut line);
                    col += 1;
                }
                out.write_all(line.as_bytes())?;
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
        CellPos, CellRect, CellSize, CssRect, Frame, GridSize, Image, Rgb, Style, TextRun,
        Viewport,
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
            payload: std::sync::Arc::new(payload.to_string()),
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
    fn render_sets_a_background_only_when_a_cell_has_one() {
        let mut frame = Frame::new(GridSize { cols: 2, rows: 1 });
        frame.paint_text(
            CellPos { col: 0, row: 0 },
            "a",
            Style { fg: Rgb { r: 1, g: 2, b: 3 }, bg: None, bold: false, reverse: false },
        );
        frame.paint_text(
            CellPos { col: 1, row: 0 },
            "b",
            Style {
                fg: Rgb { r: 1, g: 2, b: 3 },
                bg: Some(Rgb { r: 9, g: 8, b: 7 }),
                bold: false,
                reverse: false,
            },
        );

        let out = rendered(&mut Renderer::new(), &frame);

        assert!(out.contains("\x1b[48;2;9;8;7m"), "output was {out:?}");
        // Exactly once: the cell without a background must not inherit the
        // one beside it, and the reset in front of every style is what
        // stops it.
        assert_eq!(out.matches("\x1b[48;2;").count(), 1, "output was {out:?}");
    }

    #[test]
    fn the_first_image_is_transmitted_placed_and_given_placeholders() {
        let mut renderer = Renderer::new();
        let sent = rendered(&mut renderer, &framed(Some(image_at(1, "AAAA"))));

        assert!(sent.contains("a=t,f=100"), "transmitted");
        assert!(sent.contains("a=p,U=1"), "then placed");
        assert!(
            sent.contains(protocol::PLACEHOLDER),
            "the cells addressing it were written"
        );
        assert!(
            sent.contains("\x1b[38;2;119;119;116m"),
            "carrying the image id as their foreground"
        );
    }

    #[test]
    fn a_new_frame_arrives_on_the_slot_that_is_not_on_screen() {
        // Transmitting to an id tears down its placement for as long as the
        // transmission lasts, and a full-page PNG is dozens of chunks. Aimed
        // at the id being looked at, that window is the flicker.
        let mut renderer = Renderer::new();
        rendered(&mut renderer, &framed(Some(image_at(1, "AAAA"))));
        let showing = renderer.current_id();

        let sent = rendered(&mut renderer, &framed(Some(image_at(2, "BBBB"))));
        let arriving = renderer.current_id();
        assert_ne!(showing, arriving, "the slots alternate");
        assert!(
            sent.contains(&format!("i={arriving},m=0;BBBB")),
            "the new data went to the other slot"
        );
        assert!(sent.contains(&format!("a=p,U=1,i={arriving}")), "then placed");
        assert!(sent.contains(protocol::PLACEHOLDER), "then the cells follow it");
    }

    #[test]
    fn the_old_picture_is_deleted_only_after_the_new_one_is_on_screen() {
        // Never blank the frame you are looking at, at the granularity of a
        // single frame.
        let mut renderer = Renderer::new();
        rendered(&mut renderer, &framed(Some(image_at(1, "AAAA"))));
        let showing = renderer.current_id();

        let sent = rendered(&mut renderer, &framed(Some(image_at(2, "BBBB"))));
        let deleted = sent
            .find(&format!("a=d,d=i,i={showing}"))
            .expect("the old one is deleted");
        let placed = sent
            .find(&format!("a=p,U=1,i={}", renderer.current_id()))
            .expect("the new one is placed");
        assert!(placed < deleted, "placed before the old one is forgotten");
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
        assert!(sent.contains("c=8,r=4"), "re-placed at the new size");
        assert!(sent.contains(protocol::PLACEHOLDER), "and re-addressed");
    }

    #[test]
    fn a_label_over_the_picture_is_a_glyph_and_the_rest_stay_placeholders() {
        let mut renderer = Renderer::new();
        rendered(&mut renderer, &framed(Some(image_at(1, "AAAA"))));

        let mut labelled = framed(Some(image_at(2, "AAAA")));
        labelled.paint_text(
            CellPos { col: 1, row: 1 },
            "x",
            Style::default(),
        );
        let sent = rendered(&mut renderer, &labelled);
        assert!(sent.contains('x'), "the label was written");
    }

    #[test]
    fn a_label_going_away_gives_its_cell_back_to_the_picture() {
        // The bug this exists for: the diff wrote a space where the label
        // had been, and nothing put the placeholder back, so leaving hint
        // mode left a hole in the picture for every label that had been on
        // it.
        let mut renderer = Renderer::new();
        let mut labelled = framed(Some(image_at(1, "AAAA")));
        labelled.paint_text(CellPos { col: 1, row: 1 }, "x", Style::default());
        rendered(&mut renderer, &labelled);

        let sent = rendered(&mut renderer, &framed(Some(image_at(2, "AAAA"))));
        assert!(
            sent.contains(protocol::PLACEHOLDER),
            "the cell the label had is picture again, not a blank"
        );
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

    /// What a pixel frame costs to write. Run with:
    ///
    ///     cargo test -p wwt-term --lib measure_pixel_frame -- --nocapture
    ///
    /// A realistic payload: a PNG of a full page is a few hundred kilobytes,
    /// which is about 400KB of base64. The claim in section 4 of the M5 spec
    /// is that repointing the cells is the smaller half of what a frame
    /// costs, so this asserts the ratio as well as printing the time.
    #[test]
    fn measure_pixel_frame() {
        let mut renderer = Renderer::new();
        let payload = std::sync::Arc::new("A".repeat(400 * 1024));
        let area = CellRect { col: 0, row: 1, cols: 120, rows: 38 };
        let mut frame = Frame::new(GridSize { cols: 120, rows: 40 });
        frame.set_image(Some(Image { generation: 1, payload: std::sync::Arc::clone(&payload), area }));
        renderer.render(&frame, &mut Vec::new()).expect("the first frame");

        let mut worst = std::time::Duration::ZERO;
        let mut wrote = 0;
        for generation in 2..102 {
            let mut frame = Frame::new(GridSize { cols: 120, rows: 40 });
            frame.set_image(Some(Image {
                generation,
                payload: std::sync::Arc::clone(&payload),
                area,
            }));
            let mut out = Vec::with_capacity(payload.len() * 2);

            let start = std::time::Instant::now();
            renderer.render(&frame, &mut out).expect("a later frame");
            worst = worst.max(start.elapsed());
            wrote = out.len();
        }

        let cells = wrote - payload.len();
        eprintln!(
            "pixel frame, worst of 100: {worst:?}; {} bytes, of which {cells} are cells",
            wrote
        );
        assert!(
            cells < payload.len(),
            "repointing the cells must be the smaller half: {cells} against {}",
            payload.len()
        );
        assert!(worst < std::time::Duration::from_millis(20), "frame took {worst:?}");
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
        render(f, protocol::IMAGE_IDS[0], &mut buf).unwrap();
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
        let style = Style { fg: Rgb { r: 255, g: 128, b: 0 }, bg: None, bold: false, reverse: false };
        let out = render_to_string(&painted("hi", style));
        assert!(out.contains("\x1b[38;2;255;128;0m"), "output was {out:?}");
    }

    #[test]
    fn render_sets_and_clears_bold() {
        let style = Style { fg: Rgb { r: 0, g: 0, b: 0 }, bg: None, bold: true, reverse: false };
        let out = render_to_string(&painted("hi", style));
        assert!(out.contains("\x1b[1m"), "output was {out:?}");
        assert!(out.ends_with("\x1b[0m"), "output was {out:?}");
    }

    #[test]
    fn render_does_not_repeat_an_unchanged_style() {
        let style = Style { fg: Rgb { r: 10, g: 20, b: 30 }, bg: None, bold: false, reverse: false };
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
        let style = Style { fg: Rgb { r: 0, g: 0, b: 0 }, bg: None, bold: false, reverse: true };
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
