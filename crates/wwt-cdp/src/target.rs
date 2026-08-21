//! Which target, and which session on it.
//!
//! A CDP fact that travels out through the vocabulary and back: the browser
//! reports a target it attached to, and the answer is a page opened on it.
//! It is a value with no behaviour on purpose, so carrying one costs the
//! carrier no knowledge of what a target is.

/// One target in the browser, for as long as it exists.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TargetId(pub String);

/// A target the browser has attached us to, and the session to speak to it
/// on. Every command a page issues is `call_on` its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attached {
    pub target: TargetId,
    pub session: String,
}
