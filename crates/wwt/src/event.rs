//! What arrives: things that happened, and things that finished.
//!
//! The inward half of the vocabulary the seam is written in. `Core` makes
//! these out of tokio and the `Session` consumes them; neither needs the
//! other to name one, which is why they live here rather than beside either.

use crossterm::event::{KeyEvent, MouseEvent};
use wwt_frame::{CellSize, GridSize, HintTarget};
use wwt_page::Extraction;

use crate::tab::TabId;

/// Something that happened. Everything that can move the browser arrives
/// as one of these.
#[derive(Debug, Clone)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    /// The terminal has been re-measured after a resize.
    Resized(GridSize, CellSize),
    /// A page says it changed under us. Which page matters: one browser
    /// serves all of them and they all report on one subscription.
    Dirty(TabId),
    /// Something that ran off the loop's thread finished.
    Done(Job),
}

/// The result of something that ran off the loop's thread.
///
/// Every variant that came from a page names it, and a job whose tab is no
/// longer open is dropped. A page operation outlives the state that asked
/// for it, so the answer has to say what it was an answer to.
#[derive(Debug, Clone)]
pub enum Job {
    Extracted(TabId, Box<Extraction>),
    Failed(TabId, String),
    /// A navigation, history move, or reload finished.
    Settled(TabId),
    /// The page reported its interactive boxes, or could not. One variant
    /// rather than two, so there is exactly one place that can forget the
    /// query is no longer in flight.
    Hints(TabId, Result<Vec<HintTarget>, String>),
    /// The page has been told the window changed size.
    Resized(TabId),
    /// A tab's target was created and navigated, or could not be. The page
    /// itself never reaches the session; `Core` keeps it.
    Opened(TabId, Result<(), String>),
    /// Something failed after the loop had moved on: a keystroke, a click, a
    /// blur. Say so in the statusline and change nothing else.
    Noted(String),
}
