//! Wiring: browser, page, frame.
//!
//! The modules and nothing else. There was a `render_url` here that launched
//! a browser, read one URL and returned a `Frame`, which is what M1 was; the
//! session that keeps a browser and its targets alive across navigations
//! replaced it, and it stayed on as a second way to get from a URL to a frame
//! that no longer painted the same one. `Core` and `Session` are the way.

pub mod config;
pub mod core;
pub mod effect;
pub mod event;
pub mod input;
pub mod keys;
pub mod keymap;
mod page_view;
pub mod session;
pub mod store;
pub mod tab;
