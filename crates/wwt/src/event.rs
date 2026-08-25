//! What arrives: things that happened, and things that finished.
//!
//! The inward half of the vocabulary the seam is written in. `Core` makes
//! these out of tokio and the `Session` consumes them; neither needs the
//! other to name one, which is why they live here rather than beside either.

use crossterm::event::{KeyEvent, MouseEvent};
use wwt_cdp::Attached;
use wwt_frame::{CellSize, GridSize, HintTarget};
use wwt_page::{Extraction, ReaderExtraction, ScreencastFrame, Status};

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
    /// The websocket closed. Every target died with it, so every tab has
    /// lost its page, and the frame on screen is the last true thing there
    /// is about them.
    BrowserLost,
    /// A replacement browser is connected and attached. No tab has a target
    /// yet: this is the moment to ask for the one in front.
    BrowserBack,
    /// Something that ran off the loop's thread finished.
    Done(Job),
}

/// Why something did not work, in the only two kinds `Session` treats
/// differently.
///
/// `Core` reports what happened and the session decides what it means,
/// which is the seam M6 drew when the effect started naming its source
/// rather than the page carrying a flag. A string cannot carry the
/// distinction, and every rule about degrading depends on it: a script that
/// threw is a page our extractor cannot read, and a page that did not answer
/// is one whose main thread is not running, which the fallback extractor
/// needs just as much as our script does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The command was never answered. The page is not running.
    TimedOut,
    /// It was answered, with a refusal.
    Failed(String),
}

impl Failure {
    /// What an error from a page operation was.
    ///
    /// The downcast is why `wwt-cdp` has a type for this at all: the reason
    /// has to survive being wrapped in context on the way up.
    pub fn from_error(error: &anyhow::Error) -> Self {
        if error.downcast_ref::<wwt_cdp::TimedOut>().is_some() {
            return Failure::TimedOut;
        }
        Failure::Failed(error.to_string())
    }

    /// What to put in the statusline. A timeout says `[stalled]` instead, so
    /// this is only ever reached by the other kind.
    pub fn message(&self) -> String {
        match self {
            Failure::TimedOut => "the page did not answer".to_string(),
            Failure::Failed(message) => message.clone(),
        }
    }
}

/// The result of something that ran off the loop's thread.
///
/// Every variant that came from a page names it, and a job whose tab is no
/// longer open is dropped. A page operation outlives the state that asked
/// for it, so the answer has to say what it was an answer to.
#[derive(Debug, Clone)]
pub enum Job {
    /// The page was read, or could not be. One variant rather than two,
    /// for the reason `Hints` is one: it and `Status` are the only things
    /// that clear `reading`, so each has to be the single place that can
    /// forget its read is over. It also carries which source answered,
    /// because a failed script extraction and a failed snapshot mean
    /// different things and `Job::Failed` cannot tell them apart from a
    /// failed scroll.
    Extracted(TabId, Source, Result<Box<Extraction>, Failure>),
    /// The semantic reader document came back, or could not be read.
    Reader(TabId, Result<Box<ReaderExtraction>, Failure>),
    /// The chrome's half of a read came back, or could not. One variant
    /// carrying a `Result` for the reason `Extracted` is one: it and
    /// `Extracted` are the only two things that clear `reading`, and each
    /// has to be the single place that can forget its read is over.
    Status(TabId, Result<Status, Failure>),
    Failed(TabId, Failure),
    /// A navigation, history move, or reload finished.
    Settled(TabId),
    /// The page reported its interactive boxes, or could not. One variant
    /// rather than two, so there is exactly one place that can forget the
    /// query is no longer in flight.
    Hints(TabId, Result<Vec<HintTarget>, Failure>),
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
    /// A relaunch gave up. Only ever the failure: a browser that arrived is
    /// `Event::BrowserBack`, because `Core` has to file the browser and the
    /// client before the session can be told anything at all, exactly as
    /// `Finished::Opened` files a page before reporting `Job::Opened`.
    Relaunched(Result<(), String>),
    /// The session file could not be written. The one thing that fails
    /// without a tab to fail on, because the tabs are what it is made of.
    Unsaved(String),
}
