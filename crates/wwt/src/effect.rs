//! What is asked for: everything the session wants done to the world.
//!
//! The outward half of the vocabulary. One set of words for every page
//! operation, so the loop has one place that spawns rather than one per
//! feature, and so a test can read what a keystroke asked for without
//! anything having to happen.

use wwt_frame::Viewport;
use wwt_page::Input;

use crate::tab::TabId;

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Read the page.
    Extract(TabId),
    /// Ask the page for its interactive boxes.
    Hints(TabId),
    Scroll(TabId, Scroll),
    Navigate(TabId, Navigation),
    /// Send one key or click to the page, in order with the others.
    Send(TabId, Input),
    /// Take focus off whatever has it.
    Blur(TabId),
    /// Tell the page the window is this size. The terminal has already
    /// changed; this is the page catching up. Emitted once per tab, because
    /// a background tab has to be the right size already when you reach it.
    SetViewport(TabId, Viewport),
    /// Create a target for a tab the session has already made room for, and
    /// navigate it.
    OpenTab { id: TabId, url: String },
    CloseTab(TabId),
    /// Make this tab the one the browser has in front. Input dispatch is
    /// answered by whichever target is foreground, so ours and the browser's
    /// have to be the same one.
    Activate(TabId),
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
