//! What keys mean right now.

use crate::hint::HintSession;

/// The mode the browser is in.
///
/// Mode changes only in response to a keystroke. A page cannot move you
/// between modes, which is the property that makes handing it the keyboard
/// safe: `Esc` always comes back.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    /// The `:` line is open, holding what has been typed so far.
    Command(String),
    /// Every key goes to the page. Entered with `i` or by hinting a text
    /// field, left with `Esc`, which is never forwarded.
    Insert,
    /// Labels are on screen and the next keys filter them.
    Hint(HintSession),
}
