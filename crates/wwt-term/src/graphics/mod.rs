//! The Kitty graphics protocol: how an image reaches a terminal.
//!
//! Everything here is bytes-from-data. What to send is `Renderer`'s
//! decision and what an image is is `wwt-frame`'s; this module knows only
//! what the protocol looks like, which is why it can be tested with no
//! terminal anywhere.

pub mod detect;
pub mod diacritics;
pub mod protocol;
