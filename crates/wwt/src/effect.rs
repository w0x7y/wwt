//! What is asked for: everything the session wants done to the world.
//!
//! The outward half of the vocabulary. One set of words for every page
//! operation, so the loop has one place that spawns rather than one per
//! feature, and so a test can read what a keystroke asked for without
//! anything having to happen.

use wwt_cdp::Attached;
use wwt_frame::Viewport;
use wwt_page::Input;

use crate::store::Snapshot;
use crate::tab::TabId;

/// How large a picture to ask a page for, in CSS pixels.
///
/// A decision rather than a measurement: with a graphics protocol it is
/// the viewport, and without one it is twice the sample grid, which is a
/// few thousand pixels rather than a megapixel. Chromium does the scaling
/// either way, so a degraded terminal never pays for a picture it cannot
/// show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

/// Which way to read a page.
///
/// The effect says, rather than the page deciding, so that the rule about
/// when to reach for the second one is written where a test can exercise
/// it without a browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `window.__wwt`, installed into every document. Cheap, complete, and
    /// occasionally broken by the page it is installed in.
    Script,
    /// `DOMSnapshot.captureSnapshot`, which shares no code with it. Costs
    /// the whole document rather than what is on screen, offers no caret,
    /// and works on a page that has broken the script.
    Snapshot,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Read the page.
    Extract(TabId, Source),
    /// Read the page's semantic reader document.
    ReadReader(TabId),
    /// Read only what the chrome needs: the title, the URL and the scroll
    /// geometry, without the walk that produces runs.
    ///
    /// What a dirty signal asks for in pixel mode, where the runs are not
    /// painted and the walk is a forced layout on the main thread that has
    /// to paint the next picture. It names no `Source`: it is our script or
    /// it is nothing, because a degraded tab asks for an extraction instead.
    ReadStatus(TabId),
    /// Ask the page for its interactive boxes.
    Hints(TabId, Source),
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
    /// Create a target for a tab the session has already made room for,
    /// navigate it, and leave it at `scroll_y`.
    ///
    /// The offset is part of opening rather than an effect of its own
    /// because restoring means both. As two effects they are two spawned
    /// tasks, and an extraction that wins that race reads offset zero and
    /// saves it, losing the position being restored.
    OpenTab {
        id: TabId,
        url: String,
        scroll_y: f64,
    },
    /// Prepare a target the browser already attached us to, as the tab the
    /// session has just made for it. It is already loading somewhere of its
    /// own choosing, so unlike `OpenTab` there is no url to give it.
    AdoptTab { id: TabId, target: Attached },
    /// Let go of a tab's target and keep the tab. `CloseTab` without the
    /// tab going away: the URL, the title, the scroll offset and the runs
    /// are all still true, and the page is opened again when you come back.
    Detach(TabId),
    CloseTab(TabId),
    /// Make this tab the one the browser has in front. Input dispatch is
    /// answered by whichever target is foreground, so ours and the browser's
    /// have to be the same one.
    Activate(TabId),
    /// Write the open tabs down. Coalesced by the loop, so asking often is
    /// cheap and asking on every scroll frame is what keeps a crash from
    /// costing you your place.
    Save(Snapshot),
    /// Start sending pictures of this tab. Only ever the focused one: a
    /// background tab is idle, which is the rule extraction already follows.
    StartScreencast(TabId, FrameSize),
    StopScreencast(TabId),
    /// Tell the page a picture arrived, so it sends the next one. Carries
    /// the ack id from the frame, which is not a CDP session id.
    AckFrame(TabId, i64),
    /// Start a browser to replace the one that died, and say how it went.
    ///
    /// `Session` decides that we try; `Core` decides how many times and how
    /// far apart, because a count and a delay are machinery.
    Relaunch,
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
