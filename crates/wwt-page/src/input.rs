//! The shapes `Input.dispatchKeyEvent` and `Input.dispatchMouseEvent` want.
//!
//! Building them correctly is the caller's problem: this module only names
//! the fields. Where they come from is a keyboard-layout question, and the
//! binary owns it.

/// Alt. The modifier bits are a CDP bitmask, not crossterm's.
pub const ALT: u32 = 1;
pub const CTRL: u32 = 2;
pub const META: u32 = 4;
pub const SHIFT: u32 = 8;

/// One key press, described four ways.
///
/// `key`, `code`, `windows_virtual_key_code` and `text` must agree with each
/// other or web applications misbehave: anything reading `e.code`, and every
/// application-level keyboard shortcut.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyInput {
    /// The value `e.key` reports: `a`, `A`, `Enter`, `ArrowLeft`.
    pub key: String,
    /// The physical key `e.code` reports: `KeyA`, `Digit1`, `Enter`.
    pub code: String,
    pub windows_virtual_key_code: u32,
    /// What the key inserts. Empty for a key that inserts nothing.
    pub text: String,
    pub modifiers: u32,
}
