//! The whole keyboard, in one table.
//!
//! Every mode answers here, not just normal: what a key means is a function
//! of `(mode, key)` and nothing else, so the answer is pure and this is the
//! one file to read to know what a keystroke does. The session decides what
//! to *do* about an action; it never re-reads the key.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use wwt_frame::Viewport;
use wwt_ui::Mode;

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
    /// Hand the keyboard to the page until `Esc`.
    Insert,
    /// Label every interactive box and filter them as you type.
    Hints,
    Quit,

    /// Go back to normal mode. What that costs depends on the mode being
    /// left — insert also gives up focus — which is the session's business.
    Leave,

    /// Add a character to the `:` line.
    CommandPush(char),
    /// Rub one out.
    CommandPop,
    /// Run what the `:` line says.
    CommandRun,

    /// Narrow the visible hints by one character.
    HintPush(char),
    /// Widen them again.
    HintPop,

    /// Close the tab you are looking at.
    TabClose,

    /// Forward this key to the page verbatim.
    Send(KeyEvent),
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

/// What a key means in a mode, or `None` when it means nothing there.
pub fn action_for(mode: &Mode, key: KeyEvent, vp: Viewport) -> Option<Action> {
    match mode {
        Mode::Normal => normal(key, vp),
        Mode::Command(_) => command(key),
        Mode::Hint(_) => hint(key),
        Mode::Insert => insert(key),
    }
}

fn normal(key: KeyEvent, vp: Viewport) -> Option<Action> {
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
        KeyCode::Char('f') => Some(Action::Hints),
        KeyCode::Char('i') => Some(Action::Insert),
        KeyCode::Char(':') => Some(Action::EnterCommand(String::new())),
        KeyCode::Char('o') => Some(Action::EnterCommand("open ".to_string())),
        KeyCode::Char('t') => Some(Action::EnterCommand("tabopen ".to_string())),
        KeyCode::Char('x') => Some(Action::TabClose),
        KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}

fn command(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Esc => Some(Action::Leave),
        KeyCode::Backspace => Some(Action::CommandPop),
        KeyCode::Enter => Some(Action::CommandRun),
        KeyCode::Char(c) => Some(Action::CommandPush(c)),
        _ => None,
    }
}

fn hint(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Esc => Some(Action::Leave),
        KeyCode::Backspace => Some(Action::HintPop),
        KeyCode::Char(c) => Some(Action::HintPush(c)),
        _ => None,
    }
}

fn insert(key: KeyEvent) -> Option<Action> {
    match key.code {
        // Never forwarded. A page cannot trap the keyboard, which is what
        // makes handing it over safe.
        KeyCode::Esc => Some(Action::Leave),
        // A terminal transmits `Ctrl-[` as 0x1B, which *is* Escape, so the
        // two are one keystroke on the wire and `Esc` has to stay ours. The
        // page's Escape lives on `Ctrl-]`, which is 0x1D.
        KeyCode::Char(']') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Action::Send(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)))
        }
        _ => Some(Action::Send(key)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wwt_frame::{CellSize, GridSize, HintTarget};
    use wwt_ui::hint::HintSession;

    fn vp() -> Viewport {
        // 24 page rows of 20 CSS pixels each.
        Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 })
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn code(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn normal_mode() -> Mode {
        Mode::Normal
    }

    fn command_mode() -> Mode {
        Mode::Command("op".to_string())
    }

    fn hint_mode() -> Mode {
        Mode::Hint(HintSession::new(Vec::<HintTarget>::new()))
    }

    #[test]
    fn j_and_k_scroll_one_cell() {
        assert_eq!(action_for(&normal_mode(), key('j'), vp()), Some(Action::Scroll(20.0)));
        assert_eq!(action_for(&normal_mode(), key('k'), vp()), Some(Action::Scroll(-20.0)));
    }

    #[test]
    fn d_and_u_scroll_half_a_page() {
        assert_eq!(action_for(&normal_mode(), key('d'), vp()), Some(Action::Scroll(240.0)));
        assert_eq!(action_for(&normal_mode(), key('u'), vp()), Some(Action::Scroll(-240.0)));
    }

    #[test]
    fn space_and_b_scroll_a_page_less_two_rows_of_overlap() {
        assert_eq!(action_for(&normal_mode(), key(' '), vp()), Some(Action::Scroll(440.0)));
        assert_eq!(action_for(&normal_mode(), key('b'), vp()), Some(Action::Scroll(-440.0)));
    }

    #[test]
    fn g_and_shift_g_jump_to_the_ends() {
        assert_eq!(action_for(&normal_mode(), key('g'), vp()), Some(Action::ScrollTop));
        assert_eq!(action_for(&normal_mode(), key('G'), vp()), Some(Action::ScrollEnd));
    }

    #[test]
    fn history_and_reload_are_bound() {
        assert_eq!(action_for(&normal_mode(), key('H'), vp()), Some(Action::Back));
        assert_eq!(action_for(&normal_mode(), key('L'), vp()), Some(Action::Forward));
        assert_eq!(action_for(&normal_mode(), ctrl('r'), vp()), Some(Action::Reload));
    }

    #[test]
    fn colon_opens_an_empty_command_line_and_o_prefills_open() {
        assert_eq!(
            action_for(&normal_mode(), key(':'), vp()),
            Some(Action::EnterCommand(String::new()))
        );
        assert_eq!(
            action_for(&normal_mode(), key('o'), vp()),
            Some(Action::EnterCommand("open ".to_string()))
        );
    }

    #[test]
    fn q_quits() {
        assert_eq!(action_for(&normal_mode(), key('q'), vp()), Some(Action::Quit));
    }

    #[test]
    fn an_unbound_key_does_nothing() {
        assert_eq!(action_for(&normal_mode(), key('z'), vp()), None);
    }

    #[test]
    fn a_one_row_viewport_never_scrolls_backwards() {
        let tiny = Viewport::new(GridSize { cols: 20, rows: 1 }, CellSize { w: 9, h: 20 });
        // rows - 2 would underflow; a page scroll must still move forward.
        assert_eq!(action_for(&normal_mode(), key(' '), tiny), Some(Action::Scroll(20.0)));
    }

    #[test]
    fn i_hands_the_keyboard_to_the_page() {
        assert_eq!(action_for(&normal_mode(), key('i'), vp()), Some(Action::Insert));
    }

    #[test]
    fn f_opens_the_hints() {
        assert_eq!(action_for(&normal_mode(), key('f'), vp()), Some(Action::Hints));
    }

    #[test]
    fn the_command_line_collects_what_you_type() {
        assert_eq!(
            action_for(&command_mode(), key('e'), vp()),
            Some(Action::CommandPush('e'))
        );
        assert_eq!(
            action_for(&command_mode(), code(KeyCode::Backspace), vp()),
            Some(Action::CommandPop)
        );
        assert_eq!(
            action_for(&command_mode(), code(KeyCode::Enter), vp()),
            Some(Action::CommandRun)
        );
    }

    #[test]
    fn a_normal_mode_binding_is_just_a_character_on_the_command_line() {
        // `q` quits in normal mode, and types a `q` in the `:` line. That
        // the same key means two things is the whole reason the mode is
        // part of the question.
        assert_eq!(action_for(&normal_mode(), key('q'), vp()), Some(Action::Quit));
        assert_eq!(
            action_for(&command_mode(), key('q'), vp()),
            Some(Action::CommandPush('q'))
        );
    }

    #[test]
    fn hint_keys_filter_rather_than_scroll() {
        assert_eq!(action_for(&hint_mode(), key('j'), vp()), Some(Action::HintPush('j')));
        assert_eq!(
            action_for(&hint_mode(), code(KeyCode::Backspace), vp()),
            Some(Action::HintPop)
        );
    }

    #[test]
    fn insert_forwards_everything_it_is_not_keeping() {
        let typed = key('a');
        assert_eq!(action_for(&Mode::Insert, typed, vp()), Some(Action::Send(typed)));
        let arrow = code(KeyCode::Left);
        assert_eq!(action_for(&Mode::Insert, arrow, vp()), Some(Action::Send(arrow)));
        // Even a shortcut the browser would otherwise claim: in insert mode
        // the page has the keyboard.
        let reload = ctrl('r');
        assert_eq!(action_for(&Mode::Insert, reload, vp()), Some(Action::Send(reload)));
    }

    #[test]
    fn escape_is_never_forwarded_from_any_mode() {
        for mode in [normal_mode(), command_mode(), hint_mode(), Mode::Insert] {
            let action = action_for(&mode, code(KeyCode::Esc), vp());
            assert!(
                !matches!(action, Some(Action::Send(_))),
                "Esc reached the page from {mode:?}"
            );
        }
        assert_eq!(action_for(&Mode::Insert, code(KeyCode::Esc), vp()), Some(Action::Leave));
    }

    #[test]
    fn ctrl_bracket_is_how_the_page_hears_an_escape() {
        assert_eq!(
            action_for(&Mode::Insert, ctrl(']'), vp()),
            Some(Action::Send(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)))
        );
    }

    #[test]
    fn t_and_x_open_and_close_tabs_in_normal_mode_only() {
        assert_eq!(
            action_for(&normal_mode(), key('t'), vp()),
            Some(Action::EnterCommand("tabopen ".to_string()))
        );
        assert_eq!(action_for(&normal_mode(), key('x'), vp()), Some(Action::TabClose));
        // Insert mode types them, as it types everything.
        assert!(matches!(action_for(&Mode::Insert, key('x'), vp()), Some(Action::Send(_))));
    }
}
