//! What is asked for: everything the session wants done to the world.
//!
//! The outward half of the vocabulary. One set of words for every page
//! operation, so the loop has one place that spawns rather than one per
//! feature, and so a test can read what a keystroke asked for without
//! anything having to happen.

use wwt_frame::Viewport;
use wwt_page::Input;

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Read the page.
    Extract,
    /// Ask the page for its interactive boxes.
    Hints,
    Scroll(Scroll),
    Navigate(Navigation),
    /// Send one key or click to the page, in order with the others.
    Send(Input),
    /// Take focus off whatever has it.
    Blur,
    /// Tell the page the window is this size. The terminal has already
    /// changed; this is the page catching up.
    SetViewport(Viewport),
    /// Turn terminal mouse reporting on or off.
    MouseCapture(bool),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scroll {
    /// By a distance in CSS pixels, positive being downward.
    By(f64),
    Top,
    End,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Navigation {
    Open(String),
    Back,
    Forward,
    Reload,
}
