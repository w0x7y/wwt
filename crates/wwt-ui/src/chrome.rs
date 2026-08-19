//! The bottom row: a statusline, or the `:` command line when one is open.

use wwt_frame::{CellPos, Frame, GridSize, Rgb, Style};

use crate::mode::Mode;

/// What the page is doing. Shown in the statusline; never a reason to blank
/// the frame.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Loading,
    Ready,
    Stalled,
    Error(String),
}

fn chrome_style() -> Style {
    Style {
        fg: Rgb { r: 0xd0, g: 0xd0, b: 0xd0 },
        bold: false,
        reverse: true,
    }
}

/// What the statusline says about the mode, if anything.
///
/// Normal mode says nothing: the absence of a tag is what normal looks
/// like, and a browser that shouts its default state at you all day is
/// noise.
fn mode_tag(mode: &Mode) -> String {
    match mode {
        Mode::Normal | Mode::Command(_) => String::new(),
        Mode::Insert => "-- INSERT -- ".to_string(),
        Mode::Hint(session) => session.tag(),
    }
}

/// Build the statusline, padded or truncated to exactly `cols` characters.
pub fn statusline(
    mode: &Mode,
    state: &State,
    url: &str,
    title: &str,
    progress: f64,
    cols: u16,
) -> String {
    let tag = match state {
        State::Ready => String::new(),
        State::Loading => "[loading] ".to_string(),
        State::Stalled => "[stalled] ".to_string(),
        State::Error(message) => format!("[error] {message} — "),
    };

    let left = if title.is_empty() {
        format!("{}{tag}{url}", mode_tag(mode))
    } else {
        format!("{}{tag}{url} — {title}", mode_tag(mode))
    };

    let percent = format!("{:>3}%", (progress * 100.0).round() as i64);
    let cols = usize::from(cols);

    // On a very narrow terminal the percentage is what gets dropped, not the
    // URL: knowing where you are matters more than how far down you are.
    if cols <= percent.chars().count() + 1 {
        return fit(&left, cols);
    }

    let room = cols - percent.chars().count() - 1;
    format!("{}{}{}", fit(&left, room), " ", percent)
}

/// The `:` line, padded or truncated to exactly `cols` characters.
pub fn command_line(buffer: &str, cols: u16) -> String {
    fit(&format!(":{buffer}"), usize::from(cols))
}

/// Truncate or pad a string to exactly `width` characters.
fn fit(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count >= width {
        s.chars().take(width).collect()
    } else {
        let mut out = String::with_capacity(width);
        out.push_str(s);
        out.extend(std::iter::repeat_n(' ', width - count));
        out
    }
}

/// Paint the bottom row of the frame.
pub fn paint(
    frame: &mut Frame,
    mode: &Mode,
    state: &State,
    url: &str,
    title: &str,
    progress: f64,
) {
    let GridSize { cols, rows } = frame.grid();
    if rows == 0 {
        return;
    }
    let text = match mode {
        Mode::Command(buffer) => command_line(buffer, cols),
        _ => statusline(mode, state, url, title, progress, cols),
    };
    frame.paint_text(CellPos { col: 0, row: rows - 1 }, &text, chrome_style());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statusline_shows_url_title_and_progress() {
        let line = statusline(&Mode::Normal, &State::Ready, "https://example.com", "Example", 0.5, 40);
        assert!(line.contains("https://example.com"), "line was {line:?}");
        assert!(line.contains("Example"), "line was {line:?}");
        assert!(line.ends_with(" 50%"), "line was {line:?}");
    }

    #[test]
    fn statusline_is_exactly_the_grid_width() {
        for cols in [10u16, 40, 80, 200] {
            let line = statusline(&Mode::Normal, &State::Ready, "https://example.com", "Example", 0.0, cols);
            assert_eq!(line.chars().count(), usize::from(cols), "at {cols} columns");
        }
    }

    #[test]
    fn statusline_tags_a_loading_page() {
        let line = statusline(&Mode::Normal, &State::Loading, "https://example.com", "", 0.0, 60);
        assert!(line.starts_with("[loading]"), "line was {line:?}");
    }

    #[test]
    fn statusline_shows_the_error_text() {
        let state = State::Error("could not resolve host".to_string());
        let line = statusline(&Mode::Normal, &state, "https://exmaple.com", "", 0.0, 60);
        assert!(line.contains("could not resolve host"), "line was {line:?}");
    }

    #[test]
    fn a_long_url_is_truncated_rather_than_overflowing() {
        let url = "https://example.com/".to_string() + &"a".repeat(500);
        let line = statusline(&Mode::Normal, &State::Ready, &url, "", 0.0, 40);
        assert_eq!(line.chars().count(), 40);
    }

    #[test]
    fn the_command_line_shows_what_was_typed() {
        assert!(command_line("open exa", 20).starts_with(":open exa"));
    }

    #[test]
    fn paint_puts_chrome_on_the_last_row() {
        let mut frame = Frame::new(GridSize { cols: 30, rows: 3 });
        paint(&mut frame, &Mode::Normal, &State::Ready, "https://example.com", "", 0.0);
        assert_eq!(frame.row_text(0), "");
        assert!(frame.row_text(2).contains("example.com"), "row 2 was {:?}", frame.row_text(2));
    }

    #[test]
    fn paint_shows_the_command_line_instead_when_in_command_mode() {
        let mut frame = Frame::new(GridSize { cols: 30, rows: 3 });
        let mode = Mode::Command("open exa".to_string());
        paint(&mut frame, &mode, &State::Ready, "https://example.com", "", 0.0);
        assert!(frame.row_text(2).starts_with(":open exa"), "row 2 was {:?}", frame.row_text(2));
    }
    #[test]
    fn the_statusline_says_when_you_are_in_insert_mode() {
        let line = statusline(&Mode::Insert, &State::Ready, "https://example.com", "", 0.0, 60);
        assert!(line.starts_with("-- INSERT --"), "line was {line:?}");
        assert!(line.contains("https://example.com"), "line was {line:?}");
    }

    #[test]
    fn normal_mode_adds_nothing_to_the_statusline() {
        let line = statusline(&Mode::Normal, &State::Ready, "https://example.com", "", 0.0, 60);
        assert!(line.starts_with("https://example.com"), "line was {line:?}");
    }

    #[test]
    fn the_statusline_shows_what_has_been_typed_at_the_hints() {
        use crate::hint::HintSession;
        use wwt_frame::{CssRect, HintTarget, TargetKind};

        let targets = vec![HintTarget {
            rect: CssRect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 },
            kind: TargetKind::Clickable,
        }];
        let line = statusline(
            &Mode::Hint(HintSession::new(targets)),
            &State::Ready,
            "https://example.com",
            "",
            0.0,
            60,
        );
        assert!(line.starts_with("-- HINT  (1) --"), "line was {line:?}");
    }

}
