//! What arrives: things that happened, and things that finished.
//!
//! The inward half of the vocabulary the seam is written in. `Core` makes
//! these out of tokio and the `Session` consumes them; neither needs the
//! other to name one, which is why they live here rather than beside either.

use crossterm::event::{KeyEvent, MouseEvent};
use wwt_cdp::Attached;
use wwt_frame::{CellSize, GridSize, HintTarget};
use wwt_page::{Extraction, ScreencastFrame};

use crate::effect::Source;
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
    /// A picture of a page. Which page matters for the same reason a dirty
    /// signal's does: one browser serves all of them.
    ///
    /// Boxed because an `Event` is moved on every keystroke and a frame's
    /// payload is much the largest thing that can be in one.
    Frame(TabId, Box<ScreencastFrame>),
    /// A page opened a tab for itself. The session has to make room for it
    /// before it can be prepared, because ids are minted on that side.
    TargetOpened(Attached),
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
    /// The page was read, or could not be. One variant rather than two,
    /// for the reason `Hints` is one: it is the only thing that clears
    /// `extracting`, so there must be exactly one place that can forget
    /// the extraction is over. It also carries which source answered,
    /// because a failed script extraction and a failed snapshot mean
    /// different things and `Job::Failed` cannot tell them apart from a
    /// failed scroll.
    Extracted(TabId, Source, Result<Box<Extraction>, String>),
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
    /// blur, a target that would not come to the front. Say so and change
    /// nothing else.
    ///
    /// It names its tab like every other page operation. It did not, and the
    /// message landed on whichever tab happened to be in front, so a target
    /// that would not activate reported the failure on the tab you had just
    /// left. The exception also cost `on_job` a variant it had to prove
    /// could not reach the bottom of the match.
    Noted(TabId, String),
    /// The session file could not be written. The one thing that fails
    /// without a tab to fail on, because the tabs are what it is made of.
    Unsaved(String),
}
