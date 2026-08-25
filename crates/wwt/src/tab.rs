//! One page, and everything true of it rather than of the browser.
//!
//! What is left in `Session` after this is what is global: the grid, the
//! mode, the `:` line, and which of these is in front. Splitting it this way
//! is also what lets a background tab keep its runs, so a switch is a
//! repaint rather than a round trip.

use wwt_frame::{Caret, HintTarget, TextRun};
use wwt_reader::{Document, Layout};
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

/// Whether a target exists for this tab, and if not, whether one is coming.
///
/// `Core` drops every effect naming a tab it holds no page for, so this is
/// the question to ask before emitting one or setting an in-flight flag
/// beside it. It used to be a bool called `opened`, and a bool cannot carry
/// the difference that matters: `Opening` has an answer in flight and
/// `Detached` has nothing, so focusing the first should wait and focusing
/// the second should ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// A target was asked for and `Job::Opened` is coming.
    Opening,
    /// A target exists. The only state in which an effect naming this tab
    /// is not dropped.
    Attached,
    /// No target, and none is coming until this tab is focused. Evicted,
    /// restored but not yet reached, or left behind by a dead browser.
    Detached,
}

/// The semantic view cached beside the live page representation.
#[derive(Debug, Clone)]
pub struct ReaderState {
    pub document: Option<Document>,
    pub layout: Option<Layout>,
    pub top_row: usize,
    pub active: bool,
    pub wanted: bool,
    pub dirty: bool,
}

impl Default for ReaderState {
    fn default() -> Self {
        Self {
            document: None,
            layout: None,
            top_row: 0,
            active: false,
            wanted: false,
            dirty: true,
        }
    }
}

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
    /// The reader document and its terminal-width layout.
    pub reader: ReaderState,

    /// The page says it changed and we have not caught up yet. A background
    /// tab sets this and spends it when focus arrives.
    pub dirty: bool,
    /// A read is in flight; a second would race it. Either kind: pixel mode
    /// asks only for a status, and one flag rather than two is what keeps
    /// the two from being asked at once.
    pub reading: bool,
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
    /// Whether this tab has a target behind it. See `Presence`.
    pub presence: Presence,
    /// When this tab was last focused, as a count and never a clock.
    ///
    /// A counter, so the recency rule is asserted with data and its tests
    /// need neither a browser nor time.
    pub focused_at: u64,
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
            reader: ReaderState::default(),
            dirty: true,
            reading: false,
            degraded: false,
            navigating: false,
            hints: None,
            hinting: false,
            presence: Presence::Opening,
            focused_at: 0,
            read: false,
        }
    }

    /// Note that the page has changed under us.
    ///
    /// Hint targets are geometry, so a page that moved has invalidated them.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.reader.dirty = true;
        self.hints = None;
    }

    /// Let go of this tab's target, and keep the tab.
    ///
    /// The one place that says what a tab is without a browser behind it.
    /// Eviction detaches one tab, a dead Chromium detaches all of them, and
    /// a restored tab starts this way, so getting the list wrong here is
    /// three bugs rather than one.
    pub fn detach(&mut self) {
        self.presence = Presence::Detached;
        // Every answer in flight is an answer that will not arrive: `Core`
        // holds no page for this tab any more. A flag left set is a flag
        // nothing can clear, which is `f` dead for the rest of the run.
        self.reading = false;
        self.navigating = false;
        self.hinting = false;
        // Geometry, belonging to a document that is about to stop existing.
        self.hints = None;
        // A reattached page is a new document with our script freshly in
        // it, so it has earned the fast path back. The same reason a
        // navigation clears this.
        self.degraded = false;
        // The runs stay, and are what a switch back paints first. They are
        // also no longer authoritative, which is what the flag says.
        self.dirty = true;
        self.reader.dirty = true;
    }

    /// Whether an effect naming this tab would reach a page.
    pub fn attached(&self) -> bool {
        self.presence == Presence::Attached
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wwt_frame::{CssRect, Style};
    use wwt_reader::{Block, BlockKind, Document, Layout, Span};

    #[test]
    fn a_new_tab_has_not_been_read_yet() {
        let tab = Tab::new(TabId(0), "https://example.com".to_string());
        assert!(
            tab.dirty,
            "a page nobody has looked at is dirty by definition"
        );
        assert!(!tab.reading);
        assert_eq!(tab.url, "https://example.com");
    }

    #[test]
    fn a_new_tab_has_no_reader_view_and_needs_its_first_reader_document() {
        let tab = Tab::new(TabId(0), "https://example.com".to_string());

        assert_eq!(tab.reader.document, None);
        assert_eq!(tab.reader.layout, None);
        assert_eq!(tab.reader.top_row, 0);
        assert!(!tab.reader.active);
        assert!(!tab.reader.wanted);
        assert!(tab.reader.dirty);
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
    fn a_tab_that_has_only_been_asked_for_has_no_target_yet() {
        let tab = Tab::new(TabId(0), "https://example.com".to_string());
        assert_eq!(tab.presence, Presence::Opening);
        assert!(
            !tab.attached(),
            "an effect naming this tab would be dropped, so none may be emitted for it"
        );
    }

    #[test]
    fn only_an_attached_tab_can_be_asked_for_anything() {
        // The two states without a target are not interchangeable: one has
        // an answer coming and one is waiting to be focused. Both answer no
        // to the only question `Core` asks.
        let mut tab = Tab::new(TabId(0), String::new());
        tab.presence = Presence::Attached;
        assert!(tab.attached());
        tab.presence = Presence::Detached;
        assert!(!tab.attached());
    }

    #[test]
    fn a_detached_tab_keeps_what_it_looked_like_and_loses_what_it_was_waiting_for() {
        let mut tab = Tab::new(TabId(0), "https://example.com".to_string());
        tab.presence = Presence::Attached;
        tab.title = "Example".to_string();
        tab.scroll_y = 400.0;
        tab.runs = vec![TextRun {
            text: "text".to_string(),
            rect: CssRect { x: 0.0, y: 0.0, w: 400.0, h: 20.0 },
            baseline: 16.0,
            style: Style::default(),
            z: 0,
        }];
        tab.read = true;
        tab.degraded = true;
        tab.reading = true;
        tab.navigating = true;
        tab.hinting = true;
        tab.hints = Some(Vec::new());

        tab.detach();

        assert_eq!(tab.presence, Presence::Detached);
        // What it looked like, which is what makes switching back a repaint.
        assert_eq!(tab.title, "Example");
        assert_eq!(tab.scroll_y, 400.0);
        assert_eq!(tab.runs.len(), 1);
        // Answers that are never coming. `Core` drops every effect naming a
        // tab with no page, so a flag left set here is a flag nothing will
        // ever clear.
        assert!(!tab.reading);
        assert!(!tab.navigating);
        assert!(!tab.hinting);
        // Geometry belonging to a document that is about to stop existing.
        assert_eq!(tab.hints, None);
        // A new document reinstalls bootstrap.js, so the tab has earned
        // another attempt at the fast path. The same reason navigation
        // clears it.
        assert!(!tab.degraded);
        assert!(tab.dirty, "nothing about the old document is authoritative");
    }

    #[test]
    fn detaching_keeps_the_reader_view_but_marks_its_document_dirty() {
        let mut tab = Tab::new(TabId(0), "https://example.com".to_string());
        let document = Document {
            blocks: vec![Block {
                kind: BlockKind::Paragraph,
                spans: vec![Span {
                    text: "reader text".to_string(),
                    link: None,
                }],
            }],
            links: Vec::new(),
        };
        tab.reader.document = Some(document.clone());
        tab.reader.layout = Some(Layout::new(&document, 40));
        tab.reader.top_row = 3;
        tab.reader.active = true;
        tab.reader.wanted = true;
        tab.reader.dirty = false;
        tab.dirty = false;
        tab.reading = true;

        tab.detach();

        assert_eq!(tab.reader.document, Some(document));
        assert!(tab.reader.layout.is_some());
        assert_eq!(tab.reader.top_row, 3);
        assert!(tab.reader.active);
        assert!(tab.reader.wanted);
        assert!(tab.reader.dirty);
        assert!(tab.dirty);
        assert!(!tab.reading);
    }

    #[test]
    fn tab_ids_are_compared_by_value_and_not_by_position() {
        assert_eq!(TabId(3), TabId(3));
        assert_ne!(TabId(3), TabId(4));
    }
}
