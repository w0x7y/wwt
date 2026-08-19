//! Normal-mode keys. Pure, so the scroll arithmetic is testable.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use wwt_frame::Viewport;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Scroll by a distance in CSS pixels, positive being downward.
    Scroll(f64),
    ScrollTop,
    ScrollEnd,
    Back,
    Forward,
    Reload,
    /// Open the `:` line, pre-filled with this text.
    EnterCommand(String),
    Quit,
}

/// The distance one `space` moves: a screenful, less two rows kept for
/// context so you do not lose your place across the jump.
fn page(vp: Viewport) -> f64 {
    let rows = vp.grid().rows.saturating_sub(2).max(1);
    f64::from(rows) * f64::from(vp.cell().h)
}

fn half_page(vp: Viewport) -> f64 {
    let rows = (vp.grid().rows / 2).max(1);
    f64::from(rows) * f64::from(vp.cell().h)
}

fn line(vp: Viewport) -> f64 {
    f64::from(vp.cell().h)
}

pub fn action_for(key: KeyEvent, vp: Viewport) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('r') => Some(Action::Reload),
            _ => None,
        };
    }

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(Action::Scroll(line(vp))),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::Scroll(-line(vp))),
        KeyCode::Char('d') => Some(Action::Scroll(half_page(vp))),
        KeyCode::Char('u') => Some(Action::Scroll(-half_page(vp))),
        KeyCode::Char(' ') | KeyCode::PageDown => Some(Action::Scroll(page(vp))),
        KeyCode::Char('b') | KeyCode::PageUp => Some(Action::Scroll(-page(vp))),
        KeyCode::Char('g') | KeyCode::Home => Some(Action::ScrollTop),
        KeyCode::Char('G') | KeyCode::End => Some(Action::ScrollEnd),
        KeyCode::Char('H') => Some(Action::Back),
        KeyCode::Char('L') => Some(Action::Forward),
        KeyCode::Char(':') => Some(Action::EnterCommand(String::new())),
        KeyCode::Char('o') => Some(Action::EnterCommand("open ".to_string())),
        KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use wwt_frame::{CellSize, GridSize};

    fn vp() -> Viewport {
        // 24 page rows of 20 CSS pixels each.
        Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 })
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn j_and_k_scroll_one_cell() {
        assert_eq!(action_for(key('j'), vp()), Some(Action::Scroll(20.0)));
        assert_eq!(action_for(key('k'), vp()), Some(Action::Scroll(-20.0)));
    }

    #[test]
    fn d_and_u_scroll_half_a_page() {
        assert_eq!(action_for(key('d'), vp()), Some(Action::Scroll(240.0)));
        assert_eq!(action_for(key('u'), vp()), Some(Action::Scroll(-240.0)));
    }

    #[test]
    fn space_and_b_scroll_a_page_less_two_rows_of_overlap() {
        assert_eq!(action_for(key(' '), vp()), Some(Action::Scroll(440.0)));
        assert_eq!(action_for(key('b'), vp()), Some(Action::Scroll(-440.0)));
    }

    #[test]
    fn g_and_shift_g_jump_to_the_ends() {
        assert_eq!(action_for(key('g'), vp()), Some(Action::ScrollTop));
        assert_eq!(action_for(key('G'), vp()), Some(Action::ScrollEnd));
    }

    #[test]
    fn history_and_reload_are_bound() {
        assert_eq!(action_for(key('H'), vp()), Some(Action::Back));
        assert_eq!(action_for(key('L'), vp()), Some(Action::Forward));
        assert_eq!(
            action_for(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL), vp()),
            Some(Action::Reload)
        );
    }

    #[test]
    fn colon_opens_an_empty_command_line_and_o_prefills_open() {
        assert_eq!(action_for(key(':'), vp()), Some(Action::EnterCommand(String::new())));
        assert_eq!(
            action_for(key('o'), vp()),
            Some(Action::EnterCommand("open ".to_string()))
        );
    }

    #[test]
    fn q_quits() {
        assert_eq!(action_for(key('q'), vp()), Some(Action::Quit));
    }

    #[test]
    fn an_unbound_key_does_nothing() {
        assert_eq!(action_for(key('z'), vp()), None);
    }

    #[test]
    fn a_one_row_viewport_never_scrolls_backwards() {
        let tiny = Viewport::new(GridSize { cols: 20, rows: 1 }, CellSize { w: 9, h: 20 });
        // rows - 2 would underflow; a page scroll must still move forward.
        assert_eq!(action_for(key(' '), tiny), Some(Action::Scroll(20.0)));
    }
}
