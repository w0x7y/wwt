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
    /// Look at the tab at this position, counting from zero. Out of range
    /// does nothing: the tenth tab is reachable and a tenth key is not.
    TabAt(usize),

    /// Swap between showing the page as text and as a picture.
    TogglePixel,

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

    // Alt and a digit goes straight to that tab. Going straight beats cycling
    // to it: the tab you want is one keystroke away however many are open,
    // and where each one sits is already on screen in the bar.
    //
    // Alt is the modifier because it is the one a terminal reports. It is
    // sent as an Escape and then the key, so the digit arrives intact with
    // `ALT` beside it, whatever the keyboard is. Shift is not: shift and `1`
    // arrives as the byte `!` and nothing else, so a shift binding would be a
    // list of glyphs, and which glyph a layout prints there is its own
    // business.
    //
    // Only the digit, never the glyph above it. On a layout whose number row
    // is punctuation, French among them, shift and that key is how a digit is
    // typed at all, so alt and that digit is still one keystroke. Taking the
    // glyph too would be worse than useless there: `&` is the French `1` and
    // the US `7`, and one of the two would land on a tab nobody asked for.
    //
    // Nothing else is bound under alt. An unbound modifier doing what the
    // bare key does is how a mistyped shortcut scrolls the page.
    if key.modifiers.contains(KeyModifiers::ALT) {
        return match key.code {
            KeyCode::Char(c @ '1'..='9') => Some(Action::TabAt(c as usize - '1' as usize)),
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
        KeyCode::Char('p') => Some(Action::TogglePixel),
        KeyCode::Char('i') => Some(Action::Insert),
        KeyCode::Char(':') => Some(Action::EnterCommand(String::new())),
        KeyCode::Char('o') => Some(Action::EnterCommand("open ".to_string())),
        KeyCode::Char('t') => Some(Action::EnterCommand("tabopen ".to_string())),
        KeyCode::Char('x') => Some(Action::TabClose),
        KeyCode::Char('q') => Some(Action::Quit),
        // The number row is unbound without alt, and so is the glyph above
        // it: the digits are kept for the count prefix a vim-like puts on
        // them, and `!` through `(` for whatever wants them next.
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

    fn alt(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
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

    #[test]
    fn alt_and_a_digit_goes_straight_to_that_tab() {
        assert_eq!(action_for(&normal_mode(), alt('1'), vp()), Some(Action::TabAt(0)));
        assert_eq!(action_for(&normal_mode(), alt('9'), vp()), Some(Action::TabAt(8)));
        // Zero is not a tab: the bar counts from one and there is no tenth key.
        assert_eq!(action_for(&normal_mode(), alt('0'), vp()), None);
    }

    #[test]
    fn the_number_row_is_unbound_without_alt() {
        // Both halves of it. The digits are kept for a count prefix, and the
        // glyphs above them for whatever wants them next; the point of
        // spending a modifier is that neither is spent here.
        for c in "1234567890".chars() {
            assert_eq!(action_for(&normal_mode(), key(c), vp()), None, "bare {c}");
        }
        for c in "!@#$%^&*()".chars() {
            assert_eq!(action_for(&normal_mode(), key(c), vp()), None, "glyph {c}");
        }
        // Including the one spelling a terminal will never send us: shift and
        // a digit arrives as the glyph, and the glyph is unbound too.
        let shifted = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::SHIFT);
        assert_eq!(action_for(&normal_mode(), shifted, vp()), None);
    }

    #[test]
    fn only_the_digit_is_a_tab_and_never_the_glyph_over_it() {
        // `&` is the unshifted `1` of a French keyboard and the shifted `7`
        // of a US one, so taking it would send one of the two to a tab it did
        // not ask for. A French keyboard types its digit with the shift it
        // was already holding.
        assert_eq!(action_for(&normal_mode(), alt('&'), vp()), None);
        assert_eq!(action_for(&normal_mode(), alt('!'), vp()), None);
        assert_eq!(action_for(&normal_mode(), alt('§'), vp()), None);
    }

    #[test]
    fn alt_does_not_fall_through_to_what_the_bare_key_does() {
        // A modifier nothing is bound under must not quietly scroll the page.
        assert_eq!(action_for(&normal_mode(), alt('j'), vp()), None);
        assert_eq!(action_for(&normal_mode(), alt('q'), vp()), None);
        assert_eq!(action_for(&normal_mode(), alt('x'), vp()), None);
    }

    #[test]
    fn find_in_page_keeps_its_slash() {
        // European shift-7, and left unbound on purpose.
        assert_eq!(action_for(&normal_mode(), key('/'), vp()), None);
    }

    #[test]
    fn shift_j_and_shift_k_no_longer_move_between_tabs() {
        assert_eq!(action_for(&normal_mode(), key('J'), vp()), None);
        assert_eq!(action_for(&normal_mode(), key('K'), vp()), None);
    }
}
