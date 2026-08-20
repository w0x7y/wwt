//! What arrives: things that happened, and things that finished.
//!
//! The inward half of the vocabulary the seam is written in. `Core` makes
//! these out of tokio and the `Session` consumes them; neither needs the
//! other to name one, which is why they live here rather than beside either.

use crossterm::event::{KeyEvent, MouseEvent};
use wwt_frame::{CellSize, GridSize, HintTarget};
use wwt_page::Extraction;

/// Something that happened. Everything that can move the browser arrives
/// as one of these.
#[derive(Debug, Clone)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    /// The terminal has been re-measured after a resize.
    Resized(GridSize, CellSize),
    /// The page says it changed under us.
    Dirty,
    /// Something that ran off the loop's thread finished.
    Done(Job),
}

/// The result of something that ran off the loop's thread.
#[derive(Debug, Clone)]
pub enum Job {
    Extracted(Box<Extraction>),
    Failed(String),
    /// A navigation, history move, or reload finished.
    Settled,
    /// A key, a click, or a blur failed after the loop had moved on.
    InputFailed(String),
    /// The page reported its interactive boxes, or could not. One variant
    /// rather than two, so there is exactly one place that can forget the
    /// query is no longer in flight.
    Hints(Result<Vec<HintTarget>, String>),
    /// The page has been told the window changed size.
    Resized,
}
