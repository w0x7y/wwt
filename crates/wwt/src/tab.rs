//! One page, and everything true of it rather than of the browser.
//!
//! What is left in `Session` after this is what is global: the grid, the
//! mode, the `:` line, and which of these is in front. Splitting it this way
//! is also what lets a background tab keep its runs, so a switch is a
//! repaint rather than a round trip.

use wwt_frame::{Caret, HintTarget, TextRun};
use wwt_ui::chrome::State;

/// A tab's identity, for as long as the tab exists.
///
/// A counter and never a position. A page operation outlives the state that
/// asked for it: close a tab while its extraction is in flight and every
/// later tab shifts down one, so an index would let the answer land on a page
/// that never asked. A value that is never reused makes a stale answer
/// identifiable, which is the difference between dropping it and painting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(pub u32);

/// One page: what it is showing, and what we have asked it for.
#[derive(Debug, Clone)]
pub struct Tab {
    pub id: TabId,
    pub url: String,
    pub title: String,
    pub state: State,

    pub runs: Vec<TextRun>,
    /// Where typing would land, when the page has a field focused.
    pub caret: Option<Caret>,
    /// How far down the document we are, for the statusline.
    pub progress: f64,
    /// Where the document is scrolled to, for the session file.
    pub scroll_y: f64,

    /// The page says it changed and we have not caught up yet. A background
    /// tab sets this and spends it when focus arrives.
    pub dirty: bool,
    /// An extraction is in flight; a second would race it.
    pub extracting: bool,
    /// This tab's injected script threw, so it is read by snapshot until it
    /// navigates. Not one of the in-flight flags: it outlives the effect
    /// that set it, on purpose.
    pub degraded: bool,
    /// A navigation is in flight.
    pub navigating: bool,
    /// The last hint query's targets, held so that pressing `f` twice on a
    /// page that has not moved costs one round trip rather than two.
    pub hints: Option<Vec<HintTarget>>,
    /// A hint query is in flight. Every other effect answers to itself, but
    /// this one comes back and changes the mode, so it needs to be known
    /// about while it is away.
    pub hinting: bool,
    /// A target exists for this tab. False between asking for one and being
    /// told it opened, which is the window in which effects naming this tab
    /// are dropped.
    pub opened: bool,
    /// This tab has been read at least once, so its title is real and its
    /// runs are worth painting. Until then it is read even in the background:
    /// that first read is what makes the first switch to it instant.
    pub read: bool,
}

impl Tab {
    pub fn new(id: TabId, url: String) -> Self {
        Self {
            id,
            url,
            title: String::new(),
            state: State::Loading,
            runs: Vec::new(),
            caret: None,
            progress: 0.0,
            scroll_y: 0.0,
            dirty: true,
            extracting: false,
            degraded: false,
            navigating: false,
            hints: None,
            hinting: false,
            opened: false,
            read: false,
        }
    }

    /// Note that the page has changed under us.
    ///
    /// Hint targets are geometry, so a page that moved has invalidated them.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.hints = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_tab_has_not_been_read_yet() {
        let tab = Tab::new(TabId(0), "https://example.com".to_string());
        assert!(
            tab.dirty,
            "a page nobody has looked at is dirty by definition"
        );
        assert!(!tab.extracting);
        assert_eq!(tab.url, "https://example.com");
    }

    #[test]
    fn a_page_that_moved_has_invalidated_its_hint_targets() {
        // Targets are geometry. A page that changed has moved them, and a
        // click at a remembered rect would land on whatever is there now.
        let mut tab = Tab::new(TabId(0), String::new());
        tab.hints = Some(Vec::new());
        tab.mark_dirty();
        assert!(tab.dirty);
        assert_eq!(tab.hints, None);
    }

    #[test]
    fn tab_ids_are_compared_by_value_and_not_by_position() {
        assert_eq!(TabId(3), TabId(3));
        assert_ne!(TabId(3), TabId(4));
    }
}
