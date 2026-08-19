//! Chrome and modes: what input means, and what the screen says about it.
//!
//! This crate knows about a `Frame` and nothing else. It cannot reach a
//! page, a socket, or the terminal, which is what keeps every mode
//! transition and every painted overlay testable with no browser in the
//! loop.

pub mod chrome;
pub mod command;
