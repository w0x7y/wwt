//! The rows the page does not own: a tab bar on top, and a statusline, or
//! the `:` command line, underneath.

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
    /// Something worth saying that is not a failure: no hints on this page,
    /// the mouse turned off. Cleared by the next successful extraction.
    Notice(String),
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
        State::Notice(message) => format!("[{message}] "),
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

/// Where the cursor belongs on the `:` line.
///
/// The `:` line is a field you are typing into, so it gets the same caret an
/// insert-mode field gets. Past the right edge it stops at the last column,
/// beside the last character the truncated line actually shows.
///
/// Reported rather than placed: the chrome paints cells, and one caller
/// decides where the one cursor goes.
pub fn command_caret(buffer: &str, grid: GridSize) -> Option<CellPos> {
    let GridSize { cols, rows } = grid;
    if rows == 0 || cols == 0 {
        return None;
    }
    let typed = u16::try_from(buffer.chars().count()).unwrap_or(u16::MAX);
    Some(CellPos { col: typed.saturating_add(1).min(cols - 1), row: rows - 1 })
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

/// The narrowest slot worth painting: a number, a space, and a character or
/// two of title. Below this a tab says nothing, so fewer tabs are shown
/// instead of more unreadable ones.
const MIN_SLOT: usize = 8;

/// One tab's place on the bar.
pub struct TabSlot {
    pub col: u16,
    pub text: String,
    pub focused: bool,
}

/// The focused tab, against a bar that is reversed everywhere else.
///
/// The frame carries a foreground colour and no background, so an unreversed
/// run inside a reversed row is the only highlight there is.
fn focus_style() -> Style {
    Style {
        fg: Rgb { r: 0xd0, g: 0xd0, b: 0xd0 },
        bold: true,
        reverse: false,
    }
}

/// Where each visible tab goes, and which one is yours.
///
/// Titles come from pages, and the focus index can come off a session file,
/// so neither is trusted: an absurd index still produces a paintable bar.
pub fn tab_slots(titles: &[String], focus: usize, cols: u16) -> Vec<TabSlot> {
    if titles.is_empty() || cols == 0 {
        return Vec::new();
    }
    let cols = usize::from(cols);
    let focus = focus.min(titles.len() - 1);

    // How many fit at a readable width, and how wide they are once that many
    // share the row.
    let visible = (cols / MIN_SLOT).clamp(1, titles.len());
    let width = cols / visible;

    // Centre the window on the focused tab, then push it back inside the
    // list. Without the second step, focusing the last tab would scroll the
    // window off the end and show nothing.
    let start = focus.saturating_sub(visible / 2).min(titles.len() - visible);

    titles
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .enumerate()
        .map(|(offset, (index, title))| TabSlot {
            col: u16::try_from(offset * width).unwrap_or(u16::MAX),
            text: segment(index, title, width),
            focused: index == focus,
        })
        .collect()
}

/// One tab's text, exactly `width` characters: its number, then as much of
/// its title as is left.
fn segment(index: usize, title: &str, width: usize) -> String {
    let head = format!(" {} ", index + 1);
    let room = width.saturating_sub(head.chars().count() + 1);
    let shown: String = if title.chars().count() > room {
        title
            .chars()
            .take(room.saturating_sub(1))
            .chain(std::iter::once('…'))
            .collect()
    } else {
        title.to_string()
    };
    fit(&format!("{head}{shown}"), width)
}

/// Paint the top row of the frame.
pub fn paint_tabs(frame: &mut Frame, titles: &[String], focus: usize) {
    let GridSize { cols, rows } = frame.grid();
    if rows == 0 || cols == 0 {
        return;
    }
    // The whole row first, so the unused end of the bar is part of the bar
    // rather than a hole onto the page behind it.
    let blank = " ".repeat(usize::from(cols));
    frame.paint_text(CellPos { col: 0, row: 0 }, &blank, chrome_style());

    for slot in tab_slots(titles, focus, cols) {
        let style = if slot.focused {
            focus_style()
        } else {
            chrome_style()
        };
        frame.paint_text(CellPos { col: slot.col, row: 0 }, &slot.text, style);
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
    if rows == 0 || cols == 0 {
        return;
    }
    let row = rows - 1;
    let text = match mode {
        Mode::Command(buffer) => command_line(buffer, cols),
        _ => statusline(mode, state, url, title, progress, cols),
    };
    frame.paint_text(CellPos { col: 0, row }, &text, chrome_style());
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

    #[test]
    fn a_notice_is_not_dressed_up_as_an_error() {
        let line = statusline(
            &Mode::Normal,
            &State::Notice("no hints".to_string()),
            "https://example.com",
            "",
            0.0,
            60,
        );
        assert!(line.starts_with("[no hints]"), "line was {line:?}");
        assert!(!line.contains("error"), "line was {line:?}");
    }

    #[test]
    fn the_command_line_puts_the_cursor_after_what_you_typed() {
        // Column 0 is the `:` itself, so seven typed characters put the
        // cursor on column 8, on the row the chrome owns.
        assert_eq!(
            command_caret("open ex", GridSize { cols: 40, rows: 5 }),
            Some(CellPos { col: 8, row: 4 })
        );
    }

    #[test]
    fn an_overlong_command_keeps_its_cursor_on_screen() {
        assert_eq!(
            command_caret(&"x".repeat(50), GridSize { cols: 10, rows: 3 }),
            Some(CellPos { col: 9, row: 2 })
        );
    }

    #[test]
    fn a_grid_with_no_row_to_spare_has_nowhere_to_put_a_cursor() {
        assert_eq!(command_caret("open", GridSize { cols: 0, rows: 0 }), None);
    }

    #[test]
    fn painting_the_chrome_never_moves_the_cursor() {
        // The chrome paints cells and says where its caret would go. Placing
        // the one cursor is the composer's, because two modes want it and
        // only the composer can see both.
        let mut frame = Frame::new(GridSize { cols: 40, rows: 5 });
        paint(&mut frame, &Mode::Command("open".to_string()), &State::Ready, "", "", 0.0);
        assert_eq!(frame.cursor(), None);
    }

    fn titles(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("Tab {i}")).collect()
    }

    #[test]
    fn every_tab_gets_a_slot_when_they_all_fit() {
        let slots = tab_slots(&titles(3), 0, 80);
        assert_eq!(slots.len(), 3);
        assert_eq!(slots[0].col, 0);
        assert!(slots[0].text.contains('1'), "tabs are numbered from one: {:?}", slots[0].text);
        assert!(slots[2].text.contains("Tab 2"));
    }

    #[test]
    fn exactly_one_slot_is_the_focused_one() {
        let slots = tab_slots(&titles(4), 2, 80);
        let focused: Vec<usize> = slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.focused)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(focused, vec![2]);
    }

    #[test]
    fn the_slots_never_run_past_the_right_edge() {
        for cols in [4u16, 10, 40, 80, 200] {
            for count in [1usize, 2, 5, 30] {
                for slot in tab_slots(&titles(count), 0, cols) {
                    let end = usize::from(slot.col) + slot.text.chars().count();
                    assert!(end <= usize::from(cols), "{count} tabs in {cols} columns overran");
                }
            }
        }
    }

    #[test]
    fn a_long_title_is_elided_rather_than_stealing_the_next_tabs_room() {
        let long = vec!["a".repeat(200), "second".to_string()];
        let slots = tab_slots(&long, 0, 40);
        assert_eq!(slots.len(), 2);
        assert!(slots[0].text.contains('…'), "slot 0 was {:?}", slots[0].text);
        assert!(slots[1].col >= 20, "the second tab kept its half: {:?}", slots[1].col);
    }

    #[test]
    fn more_tabs_than_fit_show_a_window_around_the_focused_one() {
        // Twenty tabs cannot fit in forty columns, so the bar shows the
        // neighbourhood of wherever you are rather than the first few tabs
        // forever.
        let slots = tab_slots(&titles(20), 15, 40);
        assert!(slots.iter().any(|slot| slot.focused), "the focused tab must always be visible");
        assert!(slots.len() < 20, "it cannot possibly show them all");
    }

    #[test]
    fn one_tab_still_gets_a_bar() {
        let slots = tab_slots(&titles(1), 0, 80);
        assert_eq!(slots.len(), 1);
        assert!(slots[0].focused);
    }

    #[test]
    fn no_tabs_is_an_empty_bar_rather_than_a_panic() {
        assert_eq!(tab_slots(&[], 0, 80).len(), 0);
    }

    #[test]
    fn a_focus_index_past_the_end_does_not_panic() {
        // The index can come off a session file, which is data from disk.
        let slots = tab_slots(&titles(3), 99, 80);
        assert_eq!(slots.len(), 3);
    }

    #[test]
    fn paint_puts_the_tab_bar_on_the_first_row() {
        let mut frame = Frame::new(GridSize { cols: 40, rows: 4 });
        paint_tabs(&mut frame, &titles(2), 1);
        assert!(frame.row_text(0).contains("Tab 0"), "row 0 was {:?}", frame.row_text(0));
        assert!(frame.row_text(0).contains("Tab 1"), "row 0 was {:?}", frame.row_text(0));
        assert_eq!(frame.row_text(1), "", "the page's rows are not the bar's");
    }

    #[test]
    fn painting_the_tab_bar_never_moves_the_cursor() {
        let mut frame = Frame::new(GridSize { cols: 40, rows: 4 });
        paint_tabs(&mut frame, &titles(2), 0);
        assert_eq!(frame.cursor(), None);
    }
}
