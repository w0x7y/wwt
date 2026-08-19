//! Crossterm key events, described the way `Input.dispatchKeyEvent` needs.
//!
//! The mapping is a US layout. On other layouts the character you typed is
//! still correct, because crossterm reports the character the terminal
//! produced, but `e.code` names the physical key a US keyboard would have
//! used. The terminal does not report the layout, so there is nothing better
//! available.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use wwt_page::KeyInput;
use wwt_page::input::{ALT, CTRL, META, SHIFT};

/// US-layout punctuation: the character, the physical key that produces it,
/// and that key's virtual key code. Shifted and unshifted characters share a
/// physical key, which is the entire point of the table: `!` is `Digit1`.
const PUNCTUATION: &[(char, &str, u32)] = &[
    (' ', "Space", 32),
    ('`', "Backquote", 192),
    ('~', "Backquote", 192),
    ('-', "Minus", 189),
    ('_', "Minus", 189),
    ('=', "Equal", 187),
    ('+', "Equal", 187),
    ('[', "BracketLeft", 219),
    ('{', "BracketLeft", 219),
    (']', "BracketRight", 221),
    ('}', "BracketRight", 221),
    ('\\', "Backslash", 220),
    ('|', "Backslash", 220),
    (';', "Semicolon", 186),
    (':', "Semicolon", 186),
    ('\'', "Quote", 222),
    ('"', "Quote", 222),
    (',', "Comma", 188),
    ('<', "Comma", 188),
    ('.', "Period", 190),
    ('>', "Period", 190),
    ('/', "Slash", 191),
    ('?', "Slash", 191),
    ('!', "Digit1", 49),
    ('@', "Digit2", 50),
    ('#', "Digit3", 51),
    ('$', "Digit4", 52),
    ('%', "Digit5", 53),
    ('^', "Digit6", 54),
    ('&', "Digit7", 55),
    ('*', "Digit8", 56),
    ('(', "Digit9", 57),
    (')', "Digit0", 48),
];

/// The physical key a character comes from, as (`code`, virtual key code).
fn physical(c: char) -> Option<(String, u32)> {
    if c.is_ascii_alphabetic() {
        let upper = c.to_ascii_uppercase();
        // ASCII uppercase and the virtual key codes agree for letters.
        return Some((format!("Key{upper}"), upper as u32));
    }
    if c.is_ascii_digit() {
        return Some((format!("Digit{c}"), c as u32));
    }
    PUNCTUATION
        .iter()
        .find(|(character, _, _)| *character == c)
        .map(|(_, code, vk)| ((*code).to_string(), *vk))
}

fn modifiers_of(modifiers: KeyModifiers) -> u32 {
    let mut mask = 0;
    if modifiers.contains(KeyModifiers::ALT) {
        mask |= ALT;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        mask |= CTRL;
    }
    if modifiers.contains(KeyModifiers::SUPER) {
        mask |= META;
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        mask |= SHIFT;
    }
    mask
}

/// Describe one key, or `None` if it is not one we know how to send.
///
/// Unknown keys are dropped rather than approximated: a wrong `code` is
/// worse than a missing keystroke, because the page acts on it.
pub fn describe(event: KeyEvent) -> Option<KeyInput> {
    let modifiers = modifiers_of(event.modifiers);

    let (key, code, vk, text) = match event.code {
        KeyCode::Char(c) => {
            let (code, vk) = physical(c)?;
            (c.to_string(), code, vk, c.to_string())
        }
        KeyCode::Enter => ("Enter".into(), "Enter".into(), 13, "\r".to_string()),
        KeyCode::Tab => ("Tab".into(), "Tab".into(), 9, "\t".to_string()),
        KeyCode::Backspace => ("Backspace".into(), "Backspace".into(), 8, String::new()),
        KeyCode::Delete => ("Delete".into(), "Delete".into(), 46, String::new()),
        KeyCode::Esc => ("Escape".into(), "Escape".into(), 27, String::new()),
        KeyCode::Left => ("ArrowLeft".into(), "ArrowLeft".into(), 37, String::new()),
        KeyCode::Up => ("ArrowUp".into(), "ArrowUp".into(), 38, String::new()),
        KeyCode::Right => ("ArrowRight".into(), "ArrowRight".into(), 39, String::new()),
        KeyCode::Down => ("ArrowDown".into(), "ArrowDown".into(), 40, String::new()),
        KeyCode::Home => ("Home".into(), "Home".into(), 36, String::new()),
        KeyCode::End => ("End".into(), "End".into(), 35, String::new()),
        KeyCode::PageUp => ("PageUp".into(), "PageUp".into(), 33, String::new()),
        KeyCode::PageDown => ("PageDown".into(), "PageDown".into(), 34, String::new()),
        KeyCode::Insert => ("Insert".into(), "Insert".into(), 45, String::new()),
        KeyCode::F(n @ 1..=12) => {
            let name = format!("F{n}");
            (name.clone(), name, 111 + u32::from(n), String::new())
        }
        _ => return None,
    };

    Some(KeyInput {
        key,
        code,
        windows_virtual_key_code: vk,
        // Ctrl and Meta turn a key into a command rather than a character.
        // Shift does not: crossterm has already applied it.
        text: if modifiers & (CTRL | META) != 0 { String::new() } else { text },
        modifiers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyInput {
        describe(KeyEvent::new(code, modifiers)).expect("a bound key")
    }

    #[test]
    fn a_letter_reports_its_physical_key() {
        let k = key(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(k.key, "a");
        assert_eq!(k.code, "KeyA");
        assert_eq!(k.windows_virtual_key_code, 65);
        assert_eq!(k.text, "a");
        assert_eq!(k.modifiers, 0);
    }

    #[test]
    fn a_capital_is_the_same_physical_key_with_shift() {
        let k = key(KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert_eq!(k.code, "KeyA", "shift does not change which key was pressed");
        assert_eq!(k.windows_virtual_key_code, 65);
        assert_eq!(k.key, "A");
        assert_eq!(k.text, "A");
        assert_eq!(k.modifiers, wwt_page::input::SHIFT);
    }

    #[test]
    fn shifted_punctuation_reports_the_unshifted_code() {
        // `!` is produced by pressing Digit1, and a page reading e.code
        // needs to hear that rather than a key named after the glyph.
        let k = key(KeyCode::Char('!'), KeyModifiers::SHIFT);
        assert_eq!(k.code, "Digit1");
        assert_eq!(k.windows_virtual_key_code, 49);
        assert_eq!(k.key, "!");
        assert_eq!(k.text, "!");
    }

    #[test]
    fn control_suppresses_the_text_but_keeps_the_key() {
        // Ctrl-s must reach a page's save handler without leaving an `s` in
        // whatever box has focus.
        let k = key(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(k.code, "KeyS");
        assert_eq!(k.key, "s");
        assert_eq!(k.text, "", "a modified key inserts nothing");
        assert_eq!(k.modifiers, wwt_page::input::CTRL);
    }

    #[test]
    fn enter_and_tab_carry_their_control_characters() {
        let enter = key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(enter.text, "\r");
        assert_eq!(enter.windows_virtual_key_code, 13);
        let tab = key(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(tab.text, "\t");
        assert_eq!(tab.windows_virtual_key_code, 9);
    }

    #[test]
    fn a_named_key_inserts_nothing() {
        let esc = key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(esc.key, "Escape");
        assert_eq!(esc.code, "Escape");
        assert_eq!(esc.windows_virtual_key_code, 27);
        assert_eq!(esc.text, "");

        let left = key(KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(left.key, "ArrowLeft");
        assert_eq!(left.windows_virtual_key_code, 37);
    }

    #[test]
    fn function_keys_number_themselves() {
        let f5 = key(KeyCode::F(5), KeyModifiers::NONE);
        assert_eq!(f5.key, "F5");
        assert_eq!(f5.code, "F5");
        assert_eq!(f5.windows_virtual_key_code, 116);
    }

    #[test]
    fn an_unmapped_key_is_dropped_rather_than_guessed_at() {
        assert!(describe(KeyEvent::new(KeyCode::Menu, KeyModifiers::NONE)).is_none());
        assert!(describe(KeyEvent::new(KeyCode::F(25), KeyModifiers::NONE)).is_none());
    }

    #[test]
    fn the_space_bar_types_a_space() {
        let k = key(KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(k.code, "Space");
        assert_eq!(k.windows_virtual_key_code, 32);
        assert_eq!(k.text, " ");
    }
}
