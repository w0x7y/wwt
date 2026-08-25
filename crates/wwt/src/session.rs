//! What the browser decides, with nothing it decides it *with*.
//!
//! `Session` owns every piece of state there is and is the only thing that
//! mutates it. It reaches nothing: no page, no socket, no terminal. Events
//! go in, effects and a `Frame` come out, and the loop in `core` is the
//! adapter that turns tokio into the first and the first into the second.
//!
//! The seam is here because this is where the rules live — when an
//! extraction may start, what a key means in each mode, what a finished job
//! does to the statusline — and rules you cannot run are rules you cannot
//! trust. Everything in this file is testable with no browser and no tty.
//!
//! The words the seam is written in are next door: `event` for what arrives,
//! `effect` for what is asked for. Both sides name them, so neither owns
//! them.

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use wwt_cdp::Attached;
use wwt_frame::{
    CellPos, CellRect, CellSize, Frame, GridSize, HintTarget, Image, Samples, TargetKind,
    Viewport,
};
use wwt_page::{Input, MouseInput, ScreencastFrame, Status};
use wwt_reader::{Layout, LinkId};
use wwt_ui::Mode;
use wwt_ui::chrome::{self, Chrome, State};
use wwt_ui::command::{self, Command, Setting};
use wwt_ui::hint::{Filtered, HintSession};

use crate::effect::{Effect, FrameSize, Navigation, Scroll, Source};
use crate::event::{Event, Failure, Job};
use crate::keymap::{Action, ScrollAmount, action_for};
use crate::keys;
use crate::store::{SavedTab, Snapshot};
use crate::tab::{Presence, Tab, TabId};

/// How far one notch of the wheel scrolls, in rows. Three is what a desktop
/// browser does, and matching it is what makes the page feel normal.
const WHEEL_ROWS: i32 = 3;

/// What Chromium navigates to when it cannot reach a host.
const CHROME_ERROR_SCHEME: &str = "chrome-error://";

/// The picture last received for the focused tab, in whichever form the
/// terminal can show.
///
/// Two shapes rather than one because they leave by different doors: an
/// `Image` is a payload the renderer hands to a graphics protocol, and
/// `Samples` are cells the renderer already knows how to write.
#[derive(Debug, Clone, PartialEq)]
enum Picture {
    Graphics(Image),
    Blocks(Samples),
}

/// What the indices in the open UI hint session name.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HintSource {
    Page,
    Reader(Vec<LinkId>),
}

pub struct Session {
    grid: GridSize,
    cell: CellSize,
    vp: Viewport,

    mode: Mode,
    hint_source: Option<HintSource>,

    tabs: Vec<Tab>,
    focus: usize,
    /// Never reused, which is what makes a job from a closed tab safe to
    /// drop rather than plausible to paint.
    next_id: u32,

    /// Whether the terminal can show a picture at all, asked once at
    /// startup. A session told no refuses pixel mode rather than emitting
    /// escapes into a terminal that would print them as text.
    graphics: bool,
    /// Where anything that is not a URL goes. From the config file, and
    /// held here because `Session` is what interprets a `:` line.
    search: String,
    /// How many live targets to hold. See `evict`.
    max_tabs: usize,
    /// Global rather than per-tab: only the focused tab screencasts either
    /// way, so per-tab would buy a preference rather than a cost, and it
    /// would have to be remembered in the session file, which is a snapshot
    /// version bump and a rejected file for everyone who upgrades.
    pixel: bool,
    /// The picture last received for the focused tab.
    ///
    /// Not on `Tab`: a background tab does not screencast, so there is never
    /// a second one to hold.
    picture: Option<Picture>,
    /// Counts pictures. The renderer diffs on this rather than on the
    /// payload, so two frames that encode identically are still two frames.
    generations: u64,
    /// Counts focus changes, and stamps `Tab::focused_at` with each one.
    /// See `evict`.
    focus_counter: u64,
    /// The websocket closed and no replacement has arrived. Every tab is
    /// detached; the frame on screen is the last true thing about them.
    browser_lost: bool,
    /// A relaunch is in flight. The fourth in-flight flag in this file, and
    /// it is here for the reason the other three are: a held `j` after a
    /// failed relaunch would otherwise ask for a browser per repeat.
    relaunching: bool,
}

/// The rows the page does not get: the tab bar above it and the statusline
/// below. Unconditional, so opening a tab never reflows a page.
pub const CHROME_ROWS: u16 = 2;

/// The page viewport: the terminal grid, less the rows chrome occupies, and
/// sitting below the tab bar.
///
/// Chromium is told this is the whole window, so the page genuinely does not
/// know either chrome row exists.
pub fn page_viewport(grid: GridSize, cell: CellSize) -> Viewport {
    let rows = grid.rows.saturating_sub(CHROME_ROWS).max(1);
    Viewport::with_origin(GridSize { cols: grid.cols, rows }, cell, 1)
}

/// The page cell a terminal cell refers to, or `None` when it is one of ours.
///
/// The first row is the tab bar and the last is the statusline. The page does
/// not know either exists, so a click on one has no page coordinate to become.
pub fn page_cell(vp: &Viewport, column: u16, row: u16) -> Option<CellPos> {
    let grid = vp.grid();
    let top = vp.origin_row();
    let below = top.checked_add(grid.rows)?;
    (column < grid.cols && row >= top && row < below).then_some(CellPos { col: column, row })
}

impl Session {
    /// A session with one tab that already has a page. Tests and nothing
    /// else, now that every real tab comes into being through an effect.
    pub fn new(grid: GridSize, cell: CellSize) -> Self {
        let mut session = Self::empty(grid, cell);
        let id = session.mint();
        let mut tab = Tab::new(id, String::new());
        tab.presence = Presence::Attached;
        session.tabs.push(tab);
        session
    }

    /// The shape of a session with nothing in it. Never handed out: a
    /// browser with no tab is not a state, and both constructors put one in.
    fn empty(grid: GridSize, cell: CellSize) -> Self {
        Self {
            grid,
            cell,
            vp: page_viewport(grid, cell),
            mode: Mode::Normal,
            hint_source: None,
            tabs: Vec::new(),
            focus: 0,
            next_id: 0,
            graphics: false,
            search: crate::config::Config::default().search,
            max_tabs: crate::config::Config::default().max_tabs,
            pixel: false,
            picture: None,
            generations: 0,
            focus_counter: 0,
            browser_lost: false,
            relaunching: false,
        }
    }

    /// The tabs a restart should come back to, plus whatever was asked for
    /// on the command line.
    ///
    /// The snapshot is data from disk and is not trusted: an empty tab list
    /// and a focus index past the end both have to produce a browser you can
    /// use, because the alternative is a crash on launch you cannot get past
    /// without finding the file yourself.
    pub fn restore(
        grid: GridSize,
        cell: CellSize,
        snapshot: Option<Snapshot>,
        open: Option<String>,
    ) -> Self {
        let mut session = Self::empty(grid, cell);

        let (focus, saved) = snapshot.map_or((0, Vec::new()), |s| (s.focus, s.tabs));
        for restored in saved {
            let id = session.mint();
            let mut tab = Tab::new(id, restored.url);
            // The title from the file, so the bar reads as the tabs you left
            // rather than as a row of blanks until each one loads.
            tab.title = restored.title;
            tab.scroll_y = restored.scroll_y;
            // No target, and none asked for: the focused one is opened by
            // `begin` and the rest wait to be reached. Startup launches one
            // page rather than however many were open, and a tab you never
            // switch to costs nothing at all.
            tab.presence = Presence::Detached;
            session.tabs.push(tab);
        }
        session.focus = focus.min(session.tabs.len().saturating_sub(1));

        if let Some(url) = open {
            let id = session.mint();
            let mut tab = Tab::new(id, url);
            tab.navigating = true;
            session.tabs.push(tab);
            session.focus = session.tabs.len() - 1;
        }

        if session.tabs.is_empty() {
            let id = session.mint();
            let mut tab = Tab::new(id, "about:blank".to_string());
            tab.navigating = true;
            session.tabs.push(tab);
        }
        // The tab you left off on is the one you have most recently looked
        // at, and every other restored tab is equally long ago.
        session.look_at(session.focus);
        session
    }

    /// The next unused tab id.
    fn mint(&mut self) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        id
    }

    /// The tab you are looking at. There is always one: closing the last tab
    /// quits, so a session with no tabs never reaches a caller.
    pub fn focused(&self) -> &Tab {
        &self.tabs[self.focus]
    }

    fn focused_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.focus]
    }

    /// Look at the tab at `index`, and note when.
    ///
    /// The one place `focus` is assigned when the tab under it changes, so
    /// that the recency stamp cannot be forgotten at one of the four sites
    /// focus lands: switching, opening, adopting, and closing the tab you
    /// were on. A tab you just opened is the one you are looking at, and a
    /// stamp missed there makes the newest tab look like the oldest and
    /// evicts it first.
    ///
    /// Not called where `focus` merely shifts because a tab to the left
    /// went: you are looking at the same page, and only its index moved.
    fn look_at(&mut self, index: usize) {
        self.clear_hints();
        self.focus = index;
        self.focus_counter += 1;
        let counter = self.focus_counter;
        self.focused_mut().focused_at = counter;
    }

    pub fn focused_id(&self) -> TabId {
        self.focused().id
    }

    /// The tab a job is about, or `None` if it has since been closed.
    ///
    /// A page operation outlives the state that asked for it. Looking the id
    /// up rather than assuming the focused tab is what lets a slow load in a
    /// backgrounded tab land in that tab, and a load in a closed one land
    /// nowhere.
    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id == id)
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Make room for a tab and ask for its page.
    ///
    /// The tab exists before its page does. Between here and `Job::Opened`
    /// it is marked loading and `Core` holds nothing for it, so effects
    /// naming it are dropped; nothing could have expected to land.
    fn open_tab(&mut self, url: String, effects: &mut Vec<Effect>) {
        self.leave_for_a_new_tab(effects);
        let id = self.mint();
        let mut tab = Tab::new(id, url.clone());
        tab.navigating = true;
        self.tabs.push(tab);
        self.look_at(self.tabs.len() - 1);
        effects.push(Effect::OpenTab {
            id,
            url,
            scroll_y: 0.0,
        });
        self.save(effects);
        // A target is about to exist, so make room for it now rather than
        // at the next switch: a session built by opening tabs and never
        // switching would otherwise never reach the limit at all.
        self.evict(effects);
    }

    /// The open tabs, as they would be restored.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            version: crate::store::VERSION,
            focus: self.focus,
            tabs: self
                .tabs
                .iter()
                .map(|tab| SavedTab {
                    url: tab.url.clone(),
                    title: tab.title.clone(),
                    scroll_y: tab.scroll_y,
                })
                .collect(),
        }
    }

    /// Note that what a restart would come back to has changed.
    fn save(&self, effects: &mut Vec<Effect>) {
        effects.push(Effect::Save(self.snapshot()));
    }

    /// Make room for a tab the page opened for itself.
    ///
    /// `open_tab` with the target already in hand and no url to give it: the
    /// browser chose where it goes. It arrives focused, which is what
    /// following such a link does anywhere else.
    fn adopt_tab(&mut self, target: Attached, effects: &mut Vec<Effect>) {
        self.leave_for_a_new_tab(effects);
        let id = self.mint();
        let mut tab = Tab::new(id, String::new());
        tab.navigating = true;
        self.tabs.push(tab);
        self.look_at(self.tabs.len() - 1);
        effects.push(Effect::AdoptTab { id, target });
        // Room for the target the browser is about to hand us, for the
        // reason `open_tab` makes room.
        self.evict(effects);
        // Deliberately no save. The browser has not said where this tab is
        // going yet, and a tab with no url in the file is one a restart
        // cannot come back to. Its first extraction changes the url, which
        // is a save on its own terms.
    }

    /// Give up a tab's target while keeping the tab.
    ///
    /// The one entry point, so eviction, a dead browser and a session
    /// restored from disk all leave a tab in the same state.
    fn detach(&mut self, id: TabId, effects: &mut Vec<Effect>) {
        let Some(tab) = self.tab_mut(id) else { return };
        if !tab.attached() {
            // Nothing to give up, and `Opening` must not be overwritten: its
            // `Job::Opened` is still coming and would arrive as a surprise.
            return;
        }
        tab.detach();
        effects.push(Effect::Detach(id));
        // Deliberately no save. The URL, the title and the offset are
        // exactly what they were, and section 7 of the parent spec says a
        // write happens when one of those changed.
    }

    /// Ask for the target a detached tab does not have.
    ///
    /// Reuses `Effect::OpenTab` rather than adding a reattach of its own: it
    /// already carries the scroll offset, and its `Job::Opened` already
    /// activates the tab, restarts the screencast and triggers the first
    /// read. A reattach is an open, and inherits every rule that holds for
    /// one.
    fn reattach(&mut self, id: TabId, effects: &mut Vec<Effect>) {
        let Some(tab) = self.tab_mut(id) else { return };
        if tab.presence != Presence::Detached {
            return;
        }
        tab.presence = Presence::Opening;
        tab.navigating = true;
        tab.state = State::Loading;
        let (url, scroll_y) = (tab.url.clone(), tab.scroll_y);
        effects.push(Effect::OpenTab { id, url, scroll_y });
    }

    /// Close a tab, and go wherever that leaves you.
    fn close_tab(&mut self, id: TabId, effects: &mut Vec<Effect>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        effects.push(Effect::CloseTab(id));
        self.tabs.remove(index);

        if self.tabs.is_empty() {
            // A browser with no page in it is not a state worth having, and
            // it is the same rule `q` follows.
            effects.push(Effect::Quit);
            return;
        }
        self.save(effects);

        if index < self.focus {
            // Something to the left went. You are still looking at the same
            // page; only its index moved.
            self.focus -= 1;
            return;
        }
        if index > self.focus {
            return;
        }
        // The page you were looking at went, and its right-hand neighbour
        // has taken its index, which is where the eye already is.
        self.look_at(index.min(self.tabs.len() - 1));
        let id = self.focused_id();
        // No stop for the tab that went: it is being closed, and its target
        // goes with it.
        self.follow_focus(None, effects);
        effects.push(Effect::Activate(id));
        self.start_extract(id, effects);
    }

    /// Look at another tab.
    ///
    /// The cached runs are painted the moment this returns, so a switch is a
    /// repaint; the extraction only refreshes what is already on screen. That
    /// is what a background tab keeps its runs for.
    fn focus_tab(&mut self, index: usize, effects: &mut Vec<Effect>) {
        if index >= self.tabs.len() || index == self.focus {
            return;
        }
        let leaving = self.focused_id();
        self.look_at(index);
        let id = self.focused_id();
        // A tab that was evicted, or left behind by a browser that died,
        // asks for its target back on the way in. Its runs are painted
        // first, so this is a round trip behind a repaint rather than
        // instead of one.
        self.reattach(id, effects);
        self.follow_focus(Some(leaving), effects);
        if self.focused().attached() {
            // The browser's foreground and ours have to be the same target,
            // or input lands on the page you just left.
            effects.push(Effect::Activate(id));
            // Spends the dirty flag this tab has been accumulating in the
            // background, and does nothing if it has none.
            self.start_extract(id, effects);
        }
        self.save(effects);
        self.evict(effects);
    }

    /// Hold no more live targets than the limit, by letting go of the tab
    /// you looked at longest ago.
    ///
    /// Eligible means attached, not focused, and with nothing in flight. A
    /// background tab mid-navigation has a url that still names where it is
    /// leaving, so detaching it and reattaching later would take you back
    /// to the page you navigated away from.
    ///
    /// If nothing is eligible, nothing is evicted: the limit is a target
    /// and not a guarantee. The alternative is racing an answer that is
    /// already on its way in order to honour a number whose whole purpose
    /// is to bound memory.
    fn evict(&mut self, effects: &mut Vec<Effect>) {
        let focused = self.focused_id();
        loop {
            let attached = self.tabs.iter().filter(|tab| tab.attached()).count();
            if attached <= self.max_tabs {
                return;
            }
            let oldest = self
                .tabs
                .iter()
                .filter(|tab| {
                    tab.attached()
                        && tab.id != focused
                        && !tab.reading
                        && !tab.navigating
                        && !tab.hinting
                })
                .min_by_key(|tab| tab.focused_at)
                .map(|tab| tab.id);
            let Some(id) = oldest else { return };
            self.detach(id, effects);
        }
    }

    /// The tab `steps` along from the focused one, wrapping.
    fn neighbour(&self, steps: isize) -> usize {
        let count = self.tabs.len() as isize;
        if count == 0 {
            return 0;
        }
        (self.focus as isize + steps).rem_euclid(count) as usize
    }

    /// The first thing a browser does: ask for the pages it does not have,
    /// and read the ones it does.
    pub fn begin(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        // Only the tab in front. A restored tab is detached, and detached
        // tabs are opened when you reach them: the same machinery eviction
        // uses, pointed at startup. The tab bar is already complete, because
        // titles and urls came out of the session file.
        let id = self.focused_id();
        match self.focused().presence {
            Presence::Detached => self.reattach(id, &mut effects),
            // A tab the constructor already asked for: a command-line url,
            // or the `about:blank` a session with nothing in it gets.
            Presence::Opening => {
                let (url, scroll_y) = {
                    let tab = self.focused();
                    (tab.url.clone(), tab.scroll_y)
                };
                effects.push(Effect::OpenTab { id, url, scroll_y });
            }
            // `Session::new`, which tests use and nothing else does.
            Presence::Attached => self.start_extract(id, &mut effects),
        }
        effects
    }

    /// Tell the session whether the terminal can show a picture.
    ///
    /// Asked once at startup, before raw mode, and never again.
    pub fn set_graphics(&mut self, graphics: bool) {
        self.graphics = graphics;
    }

    /// Take what the config file said.
    ///
    /// Called once at startup, like `set_graphics`, and for the same reason:
    /// neither of these is a thing that changes while the browser is running.
    pub fn configure(&mut self, config: &crate::config::Config) {
        self.search = config.search.clone();
        self.max_tabs = config.max_tabs;
    }

    /// Enter or leave pixel mode.
    ///
    /// Never refused. Without a graphics protocol the picture is
    /// half-block rather than absent, which is what M5's notice said it
    /// was waiting for. Whether a picture is true pixels or coloured
    /// blocks is a property of the terminal and not a mode: there is one
    /// key and one tag.
    fn set_pixel(&mut self, on: bool, effects: &mut Vec<Effect>) {
        if on == self.pixel {
            return;
        }
        self.clear_hints();
        self.pixel = on;
        let id = self.focused_id();
        let reader_active = self.focused().reader.active;
        if on {
            if !reader_active {
                effects.push(Effect::StartScreencast(id, self.frame_size()));
            }
        } else {
            // The picture goes with the mode, so the next compose carries
            // none and the renderer deletes it from the terminal.
            self.picture = None;
            if !reader_active {
                effects.push(Effect::StopScreencast(id));
            }
            // Nobody's runs were being maintained while the picture was up,
            // so every tab's are suspect, not just the one in front. A
            // background tab only takes the flag and spends it when focus
            // arrives, which is M4's idling rule doing exactly its job: the
            // one in front costs a read now and the rest cost nothing until
            // you look at them. Marking only the focused tab left a tab you
            // had visited in pixel mode painting stale runs on the switch
            // back, because a switch spends a dirty flag and never sets one.
            for tab in &mut self.tabs {
                tab.dirty = true;
            }
            if !reader_active {
                self.start_extract(id, effects);
            }
        }
    }

    fn set_reader(&mut self, on: bool, effects: &mut Vec<Effect>) {
        self.clear_hints();
        let id = self.focused_id();
        let was_active = self.focused().reader.active;
        if !on {
            let pending = !was_active
                && matches!(
                    self.focused().state,
                    State::Notice(ref message) if message == "reading"
                );
            self.leave_reader(effects);
            if pending {
                self.focused_mut().state = State::Ready;
            }
            self.start_extract(id, effects);
            return;
        }

        let clean_cache = {
            let reader = &self.focused().reader;
            !reader.dirty && reader.document.is_some() && reader.layout.is_some()
        };
        let tab = self.focused_mut();
        tab.reader.wanted = true;
        if clean_cache {
            tab.reader.active = true;
        } else {
            tab.state = State::Notice("reading".to_string());
        }
        let active = self.focused().reader.active;
        if self.pixel && !was_active && active {
            effects.push(Effect::StopScreencast(id));
        }
        self.start_reader(id, effects);
    }

    /// Leave the semantic view without discarding its reusable cache.
    fn leave_reader(&mut self, effects: &mut Vec<Effect>) {
        self.clear_hints();
        let id = self.focused_id();
        let was_active = self.focused().reader.active;
        let tab = self.focused_mut();
        tab.reader.wanted = false;
        tab.reader.active = false;
        if self.pixel && was_active {
            effects.push(Effect::StartScreencast(id, self.frame_size()));
        }
    }

    /// Move the screencast to whatever tab is focused now.
    ///
    /// Called wherever the focus changes or the viewport does, and does
    /// nothing at all in text mode. The picture is deliberately not
    /// cleared: a switch in pixel mode is a round trip, and the previous
    /// picture under the new tab's chrome is what "never blank the frame
    /// you are looking at" means here.
    fn follow_focus(&mut self, leaving: Option<TabId>, effects: &mut Vec<Effect>) {
        if !self.pixel {
            return;
        }
        if let Some(leaving) = leaving {
            effects.push(Effect::StopScreencast(leaving));
        }
        effects.push(Effect::StartScreencast(self.focused_id(), self.frame_size()));
    }

    /// Stop the picture on the way to a tab that does not exist yet.
    ///
    /// `follow_focus` is for a tab already open and does the stop and the
    /// start together. It cannot be used here: `Core` drops any effect
    /// naming a page it does not hold, which is every effect between asking
    /// for a tab and being told it opened, so a start emitted now is a start
    /// nobody hears. The tab being left does exist, so its stop is emitted
    /// here and the start waits for `Job::Opened`.
    ///
    /// Without the stop, the tab you left goes on producing frames that
    /// `on_frame` acks and discards for naming a tab that is no longer in
    /// front, and the picture on screen stays the last one it sent.
    fn leave_for_a_new_tab(&mut self, effects: &mut Vec<Effect>) {
        if !self.pixel || self.focused().reader.active {
            return;
        }
        effects.push(Effect::StopScreencast(self.focused_id()));
    }

    /// How large a picture to ask for. See `FrameSize`.
    fn frame_size(&self) -> FrameSize {
        if self.graphics {
            return FrameSize { width: self.vp.css_width(), height: self.vp.css_height() };
        }
        let grid = self.vp.grid();
        FrameSize { width: u32::from(grid.cols) * 2, height: u32::from(grid.rows) * 4 }
    }

    /// Say something in the statusline.
    pub fn notice(&mut self, message: &str) {
        self.focused_mut().state = State::Notice(message.to_string());
    }

    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    pub fn state(&self) -> &State {
        &self.focused().state
    }

    pub fn viewport(&self) -> Viewport {
        self.vp
    }

    /// Paint the page and the chrome into one full-grid frame.
    pub fn compose(&self) -> Frame {
        let mut frame = Frame::new(self.grid);
        let tab = self.focused();

        // Each representation owns the page area outright. Reader comes
        // first because it is the per-tab view in front; pixel is the
        // global preference waiting underneath it.
        if tab.reader.active {
            if let Some(layout) = &tab.reader.layout {
                layout.paint(
                    &mut frame,
                    tab.reader.top_row,
                    self.vp.origin_row(),
                    self.vp.grid().rows,
                );
            }
        } else if self.pixel {
            match &self.picture {
                Some(Picture::Graphics(image)) => frame.set_image(Some(image.clone())),
                Some(Picture::Blocks(samples)) => frame
                    .paint_samples(CellRect::of(self.vp.grid(), self.vp.origin_row()), samples),
                None => {}
            }
        } else {
            frame.paint_runs(&self.vp, &tab.runs);
        }

        // After the page and before the chrome: labels cover the text they
        // point at, which is what makes them readable, and the chrome still
        // owns its rows.
        if let Mode::Hint(session) = &self.mode {
            session.paint(&mut frame);
        }

        let titles: Vec<String> = self.tabs.iter().map(|tab| tab.title.clone()).collect();
        let progress = if tab.reader.active {
            let max_top = tab
                .reader
                .layout
                .as_ref()
                .map_or(0, |layout| {
                    layout
                        .rows()
                        .saturating_sub(usize::from(self.vp.grid().rows))
                });
            if max_top == 0 {
                0.0
            } else {
                tab.reader.top_row as f64 / max_top as f64
            }
        } else {
            tab.progress
        };
        chrome::paint(
            &mut frame,
            &Chrome {
                mode: &self.mode,
                state: &tab.state,
                url: &tab.url,
                title: &tab.title,
                progress,
                titles: &titles,
                focus: self.focus,
                pixel: self.pixel,
                reader: tab.reader.active,
                degraded: tab.degraded,
            },
        );

        // One place decides where the cursor goes, though two modes have an
        // insertion point. Splitting that between here and the chrome would
        // leave the two exclusive only by accident of paint order.
        frame.set_cursor(match &self.mode {
            // In pixel mode the page drew its own caret into the picture,
            // so placing ours on top of it is two carets disagreeing about
            // where the insertion point is. The command line keeps its own:
            // it is painted into a chrome row, which no image ever covers.
            Mode::Insert if self.pixel => None,
            // A page can focus a field without your asking, and a caret
            // there would promise that your typing lands in it when in
            // normal mode it does not.
            Mode::Insert => tab.caret.and_then(|caret| caret.cell(&self.vp)),
            Mode::Command(buffer) => chrome::command_caret(buffer, self.grid),
            Mode::Normal | Mode::Hint(_) => None,
        });
        frame
    }

    /// Take one event and say what it should cost.
    pub fn on(&mut self, event: Event) -> Vec<Effect> {
        let mut effects = Vec::new();
        match event {
            Event::Key(key) => self.on_key(key, &mut effects),
            Event::Mouse(mouse) => self.on_mouse(mouse, &mut effects),
            Event::Resized(grid, cell) => self.on_resize(grid, cell, &mut effects),
            Event::Dirty(id) => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.mark_dirty();
                }
                self.start_current_read(id, &mut effects);
            }
            Event::Frame(id, frame) => self.on_frame(id, *frame, &mut effects),
            Event::TargetOpened(target) => self.adopt_tab(target, &mut effects),
            Event::BrowserLost => self.on_browser_lost(&mut effects),
            Event::BrowserBack => self.on_browser_back(&mut effects),
            Event::Done(job) => self.on_job(job, &mut effects),
        }
        effects
    }

    /// The browser died. Keep everything, ask for another.
    fn on_browser_lost(&mut self, effects: &mut Vec<Effect>) {
        self.browser_lost = true;
        // `Tab::detach` and not `Session::detach`: there is no target on the
        // other end to close, so emitting `Effect::Detach` would ask `Core`
        // to close pages whose websocket is already gone.
        for tab in &mut self.tabs {
            tab.detach();
        }
        self.focused_mut().state = State::Notice("browser gone, restarting".to_string());
        self.ask_for_a_browser(effects);
    }

    /// Ask for a browser, unless we already have.
    ///
    /// The fourth in-flight flag in this file, and it exists for the same
    /// reason as the other three: a held `j` after a failed relaunch would
    /// otherwise spawn a relaunch per repeat.
    fn ask_for_a_browser(&mut self, effects: &mut Vec<Effect>) {
        if self.relaunching {
            return;
        }
        self.relaunching = true;
        effects.push(Effect::Relaunch);
    }

    /// A browser arrived. Nothing has a target yet.
    fn on_browser_back(&mut self, effects: &mut Vec<Effect>) {
        self.browser_lost = false;
        self.relaunching = false;
        // Only the tab in front, because the restart path is lazy restore
        // arrived at from the other direction. A background tab pays for
        // its target when you reach it, which is M4's idling rule.
        let id = self.focused_id();
        self.reattach(id, effects);
    }

    /// A picture arrived.
    ///
    /// Acked whatever becomes of it: Chromium sends the next only once this
    /// one is answered, so a frame we drop still has to be answered or the
    /// screencast stops with nothing to say it did. A tab that is gone is
    /// the one exception, because there is nothing left to answer.
    fn on_frame(&mut self, id: TabId, frame: ScreencastFrame, effects: &mut Vec<Effect>) {
        if !self.tabs.iter().any(|tab| tab.id == id) {
            return;
        }
        effects.push(Effect::AckFrame(id, frame.ack));

        // A frame for a tab you have switched away from, or one that was in
        // flight when pixel mode was left, is answered and discarded.
        if !self.pixel || self.focused().reader.active || self.focused_id() != id {
            return;
        }
        if self.graphics {
            // M5's path: the bytes never leave base64.
            self.generations += 1;
            self.picture = Some(Picture::Graphics(Image {
                generation: self.generations,
                payload: std::sync::Arc::new(frame.data),
                area: CellRect::of(self.vp.grid(), self.vp.origin_row()),
            }));
            return;
        }

        // Half-block has to look inside. Decoded here rather than in
        // `compose`, which runs for every hint label, mode change and
        // statusline update, and here rather than in a spawned task,
        // because a frame arrives on the CDP arm of the loop and never as
        // a job. A few thousand pixels against a 33ms pacing interval.
        let grid = self.vp.grid();
        let decoded = wwt_png::decode_base64(&frame.data).ok().and_then(|png| {
            Samples::resampled(png.width, png.height, &png.pixels, grid.cols, grid.rows * 2)
        });
        match decoded {
            Some(samples) => self.picture = Some(Picture::Blocks(samples)),
            // The frame you are looking at stands. It has already been
            // acked above, which is what keeps the screencast running.
            None => self.notice("that picture could not be read"),
        }
    }

    fn on_key(&mut self, key: KeyEvent, effects: &mut Vec<Effect>) {
        let Some(action) = action_for(&self.mode, key, self.vp) else {
            return;
        };
        self.run_action(action, effects);
    }

    fn run_action(&mut self, action: Action, effects: &mut Vec<Effect>) {
        // With no browser there is nothing for most of these to act on, and
        // a keystroke is how you ask for one back. Deliberately not a timer:
        // an idle wwt costs ~zero CPU and that rule does not get an
        // exception for the state where there is nothing to be busy about.
        if self.browser_lost && self.action_touches_the_page(&action) {
            self.ask_for_a_browser(effects);
            return;
        }
        match action {
            Action::Quit => {
                // Stop before quitting, so the browser is not left painting
                // for a terminal that has gone.
                if self.pixel && !self.focused().reader.active {
                    effects.push(Effect::StopScreencast(self.focused_id()));
                }
                effects.push(Effect::Quit);
            }
            Action::TogglePixel if self.focused().reader.active => {
                self.leave_reader(effects);
                self.set_pixel(true, effects);
            }
            Action::TogglePixel => self.set_pixel(!self.pixel, effects),
            Action::ToggleReader => {
                let on = !(self.focused().reader.wanted || self.focused().reader.active);
                self.set_reader(on, effects);
            }
            Action::EnterCommand(prefill) => self.mode = Mode::Command(prefill),
            Action::Insert => {
                self.leave_reader(effects);
                self.mode = Mode::Insert;
            }
            Action::Hints if self.focused().reader.active => self.enter_reader_hints(),
            Action::Hints => match self.focused().hints.clone() {
                Some(targets) => self.enter_page_hints(targets),
                // `f` pressed twice before the first answer comes back is
                // one question, not two.
                //
                // And a tab with no page behind it is not asked at all.
                // `Core` drops an effect naming a page it does not hold,
                // which is every effect between asking for a tab and being
                // told it opened, and `Job::Hints` is the only thing that
                // clears the flag below. Setting it for a query nobody can
                // answer leaves `f` dead on that tab for the rest of the
                // run. `Tab::opened` is what names that window.
                None if !self.focused().hinting && self.focused().attached() => {
                    let id = self.focused_id();
                    self.focused_mut().hinting = true;
                    let source =
                        if self.focused().degraded { Source::Snapshot } else { Source::Script };
                    effects.push(Effect::Hints(id, source));
                }
                None => {}
            },

            // Scrolling does not settle the way a navigation does; the
            // page's own scroll listener reports when it has moved.
            Action::Scroll(amount) => {
                if self.focused().reader.active {
                    self.scroll_reader(amount);
                } else {
                    effects.push(Effect::Scroll(self.focused_id(), self.page_scroll(amount)));
                }
            }
            Action::Back => self.navigate(Navigation::Back, effects),
            Action::Forward => self.navigate(Navigation::Forward, effects),
            Action::Reload => self.navigate(Navigation::Reload, effects),

            Action::Leave => {
                // Leaving insert mode has already happened by the time the
                // blur runs. If it fails the statusline says so and the
                // keyboard is still yours: taking it back must never depend
                // on the page.
                if self.mode == Mode::Insert {
                    effects.push(Effect::Blur(self.focused_id()));
                }
                self.mode = Mode::Normal;
                self.hint_source = None;
            }

            Action::CommandPush(c) => {
                if let Mode::Command(buffer) = &mut self.mode {
                    buffer.push(c);
                }
            }
            Action::CommandPop => {
                if let Mode::Command(buffer) = &mut self.mode {
                    buffer.pop();
                }
            }
            Action::CommandRun => {
                let Mode::Command(buffer) = &self.mode else {
                    return;
                };
                let line = buffer.clone();
                self.mode = Mode::Normal;
                match command::parse(&line, &self.search) {
                    Ok(Command::Quit) => effects.push(Effect::Quit),
                    Ok(command) => self.run_command(command, effects),
                    Err(message) => self.focused_mut().state = State::Error(message),
                }
            }

            Action::HintPush(c) => {
                let Mode::Hint(session) = &mut self.mode else {
                    return;
                };
                let filtered = session.push(c);
                self.on_filtered(filtered, effects);
            }
            Action::HintPop => {
                let Mode::Hint(session) = &mut self.mode else {
                    return;
                };
                let filtered = session.pop();
                self.on_filtered(filtered, effects);
            }

            Action::TabClose => {
                let id = self.focused_id();
                self.close_tab(id, effects);
            }
            // Out of range does nothing rather than clamping to the last
            // tab: `$` with three open is a tab that is not there, and
            // landing somewhere you did not ask for is worse than landing
            // nowhere.
            Action::TabAt(index) => self.focus_tab(index, effects),

            Action::Send(key) => self.send_key(key, effects),
        }
    }

    fn page_scroll(&self, amount: ScrollAmount) -> Scroll {
        let rows = self.vp.grid().rows;
        let pixels = |rows: i32| f64::from(rows) * f64::from(self.cell.h);
        match amount {
            ScrollAmount::Lines(lines) => Scroll::By(pixels(lines)),
            ScrollAmount::HalfPages(pages) => {
                Scroll::By(pixels(i32::from((rows / 2).max(1)) * pages))
            }
            ScrollAmount::Pages(pages) => {
                Scroll::By(pixels(i32::from(rows.saturating_sub(2).max(1)) * pages))
            }
            ScrollAmount::Top => Scroll::Top,
            ScrollAmount::End => Scroll::End,
        }
    }

    fn scroll_reader(&mut self, amount: ScrollAmount) {
        let page_rows = usize::from(self.vp.grid().rows);
        let max_top = self
            .focused()
            .reader
            .layout
            .as_ref()
            .map_or(0, |layout| layout.rows().saturating_sub(page_rows));
        let top = self.focused().reader.top_row;
        let shifted = |rows: i32| {
            if rows >= 0 {
                top.saturating_add(rows as usize)
            } else {
                top.saturating_sub(rows.unsigned_abs() as usize)
            }
        };
        let next = match amount {
            ScrollAmount::Lines(lines) => shifted(lines),
            ScrollAmount::HalfPages(pages) => {
                shifted(i32::from((self.vp.grid().rows / 2).max(1)) * pages)
            }
            ScrollAmount::Pages(pages) => {
                shifted(i32::from(self.vp.grid().rows.saturating_sub(2).max(1)) * pages)
            }
            ScrollAmount::Top => 0,
            ScrollAmount::End => max_top,
        };
        self.focused_mut().reader.top_row = next.min(max_top);
    }

    /// Whether this action needs the hidden page in the current view.
    fn action_touches_the_page(&self, action: &Action) -> bool {
        match action {
            Action::Scroll(_) | Action::Hints if self.focused().reader.active => false,
            _ => matches!(
                action,
                Action::Scroll(_)
                    | Action::Back
                    | Action::Forward
                    | Action::Reload
                    | Action::Hints
                    | Action::Insert
                    | Action::Send(_)
            ),
        }
    }

    /// Forward one key to the page, if it is one we know how to describe.
    ///
    /// An unknown key is dropped rather than approximated: a wrong `code` is
    /// worse than a missing keystroke, because the page acts on it.
    fn send_key(&self, key: KeyEvent, effects: &mut Vec<Effect>) {
        if let Some(input) = keys::describe(key) {
            effects.push(Effect::Send(self.focused_id(), Input::Key(input)));
        }
    }

    fn run_command(&mut self, command: Command, effects: &mut Vec<Effect>) {
        match command {
            Command::Open(url) => {
                self.focused_mut().url = url.clone();
                self.navigate(Navigation::Open(url), effects);
            }
            Command::TabOpen(url) => self.open_tab(url, effects),
            Command::TabClose => {
                let id = self.focused_id();
                self.close_tab(id, effects);
            }
            Command::TabNext => {
                let index = self.neighbour(1);
                self.focus_tab(index, effects);
            }
            Command::TabPrev => {
                let index = self.neighbour(-1);
                self.focus_tab(index, effects);
            }
            Command::Back => self.navigate(Navigation::Back, effects),
            Command::Forward => self.navigate(Navigation::Forward, effects),
            Command::Reload => self.navigate(Navigation::Reload, effects),
            Command::Set(Setting::Mouse(on)) => {
                effects.push(Effect::MouseCapture(on));
                self.focused_mut().state =
                    State::Notice(if on { "mouse on" } else { "mouse off" }.to_string());
            }
            Command::Set(Setting::Pixel(on)) => self.set_pixel(on, effects),
            // Handled by the caller.
            Command::Quit => {}
        }
    }

    /// Change what page we are on.
    ///
    /// The previous page stays on screen, marked loading, until the new one
    /// has been extracted. Nothing a page does blanks the frame.
    fn navigate(&mut self, navigation: Navigation, effects: &mut Vec<Effect>) {
        if self.focused().navigating {
            return;
        }
        self.leave_reader(effects);
        let id = self.focused_id();
        let tab = self.focused_mut();
        tab.replace_document();
        tab.navigating = true;
        // A new document reinstalls bootstrap.js, so the next page has done
        // nothing to deserve the slow path. Cleared on asking rather than
        // on arriving, which makes a reload the way back from a tab that
        // degraded on something transient.
        tab.degraded = false;
        tab.state = State::Loading;
        effects.push(Effect::Navigate(id, navigation));
    }

    fn on_mouse(&mut self, event: MouseEvent, effects: &mut Vec<Effect>) {
        let Some(cell) = page_cell(&self.vp, event.column, event.row) else {
            return;
        };
        if self.focused().reader.active {
            match event.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    let link = self.focused().reader.layout.as_ref().and_then(|layout| {
                        layout.link_at(
                            cell,
                            self.focused().reader.top_row,
                            self.vp.origin_row(),
                            self.vp.grid().rows,
                        )
                    });
                    if let Some(link) = link {
                        self.activate_reader(link, effects);
                    }
                }
                MouseEventKind::ScrollDown => {
                    self.scroll_reader(ScrollAmount::Lines(WHEEL_ROWS));
                }
                MouseEventKind::ScrollUp => {
                    self.scroll_reader(ScrollAmount::Lines(-WHEEL_ROWS));
                }
                // A release completes no local operation. It is still ours:
                // sending it to the hidden page would give Chromium half a
                // click whose press it never saw.
                MouseEventKind::Up(MouseButton::Left) => {}
                _ => {}
            }
            return;
        }
        // `to_css` returns the cell's centre, so the click lands
        // unambiguously inside the cell you pointed at.
        let at = self.vp.to_css(cell);
        let notch = f64::from(WHEEL_ROWS) * f64::from(self.cell.h);

        let mouse = match event.kind {
            MouseEventKind::Down(MouseButton::Left) => MouseInput::press(at),
            MouseEventKind::Up(MouseButton::Left) => MouseInput::release(at),
            MouseEventKind::ScrollDown => MouseInput::wheel(at, notch),
            MouseEventKind::ScrollUp => MouseInput::wheel(at, -notch),
            // Motion would cost a round trip per reported frame, and there is
            // no context menu to open and no tab to middle-click into.
            _ => return,
        };
        effects.push(Effect::Send(self.focused_id(), Input::Mouse(mouse)));
    }

    fn start_extract(&mut self, id: TabId, effects: &mut Vec<Effect>) {
        let focused = self.focused_id() == id;
        // Only what is in front is painted as a picture. A background tab is
        // read for its runs, which is what makes the first switch to it a
        // repaint rather than a round trip.
        let pixel = self.pixel && focused;
        let Some(tab) = self.tab_mut(id) else { return };
        // A tab with no target has no page to read. The dirty flag is kept
        // rather than spent, and the reattach is what spends it.
        if !tab.attached() {
            return;
        }
        if tab.reading || !tab.dirty {
            return;
        }
        // A background tab keeps its flag and spends it when focus arrives.
        // Reading a page nobody is looking at is a round trip for a frame
        // nobody will see, and spec section 3 is explicit that an idle
        // background tab must cost what an idle foreground tab costs.
        //
        // The exception is a tab nobody has read yet: reading it once is what
        // puts a real title in the bar and makes the first switch to it a
        // repaint rather than a round trip. Idling means after that.
        if !focused && tab.read {
            return;
        }
        tab.reading = true;
        tab.dirty = false;
        // Pixel mode paints the picture and never the runs, so producing
        // them is a forced layout for an answer `compose` throws away. Only
        // the focused tab is a picture, and only a tab whose script works
        // can be asked our cheap question at all: a degraded one asks the
        // snapshot, which is the whole document either way.
        //
        // A tab nobody has read yet is the exception, whatever mode you are
        // in. Reading it once is what puts a real title in the bar and gives
        // it the runs that make the first switch to it a repaint, and a
        // status carries neither of the two.
        if pixel && tab.read && !tab.degraded {
            effects.push(Effect::ReadStatus(id));
            return;
        }
        let source = if tab.degraded { Source::Snapshot } else { Source::Script };
        effects.push(Effect::Extract(id, source));
    }

    fn start_reader(&mut self, id: TabId, effects: &mut Vec<Effect>) {
        let Some(tab) = self.tab_mut(id) else { return };
        if !tab.attached()
            || tab.reading
            || !tab.reader.dirty
            || !(tab.reader.wanted || tab.reader.active)
        {
            return;
        }
        tab.reading = true;
        tab.reader.dirty = false;
        effects.push(Effect::ReadReader(id));
    }

    /// Refresh the representation this tab currently wants in front.
    fn start_current_read(&mut self, id: TabId, effects: &mut Vec<Effect>) {
        let reader = self
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .is_some_and(|tab| tab.reader.wanted || tab.reader.active);
        if reader {
            self.start_reader(id, effects);
        } else {
            self.start_extract(id, effects);
        }
    }

    /// Everything the chrome learns from a page, applied the same way
    /// whichever read produced it.
    ///
    /// An extraction and a status read differ in whether runs came with it
    /// and in nothing else, so this is the only place that knows what a
    /// title, a URL and a scroll offset mean. Splitting it out is what lets
    /// pixel mode ask the cheap question without a second copy of the error
    /// detection and the save rule drifting away from this one.
    fn apply_status(&mut self, id: TabId, status: Status, effects: &mut Vec<Effect>) {
        let progress = status.scroll_progress();
        let Some(tab) = self.tab_mut(id) else { return };
        // What a restart would come back to, before this read touches it.
        // Compared against what is stored rather than against what arrived:
        // an error page's URL is deliberately not kept, so a comparison with
        // the read would differ every time and turn a page that cannot load
        // into a write per dirty signal.
        let was = (tab.url.clone(), tab.title.clone(), tab.scroll_y);
        tab.progress = progress;
        tab.scroll_y = status.scroll_y;
        tab.title = status.title;

        // Chromium answers a DNS or connection failure by navigating to its
        // own error page rather than failing the command, so a navigation can
        // "succeed" into one. Its error page is more use than a stale frame,
        // it says what went wrong, but the statusline must not go on claiming
        // the page is fine.
        if status.url.starts_with(CHROME_ERROR_SCHEME) {
            // The statusline prints the URL itself, so naming it here too
            // would print it twice.
            tab.state = State::Error("could not be reached".to_string());
        } else {
            tab.url = status.url;
            if !tab.navigating {
                tab.state = State::Ready;
            }
        }

        let tab = self.tab_mut(id).expect("resolved above");
        if was != (tab.url.clone(), tab.title.clone(), tab.scroll_y) {
            self.save(effects);
        }
    }

    fn enter_page_hints(&mut self, targets: Vec<HintTarget>) {
        let cells = targets
            .iter()
            .map(|target| target.label_cell(&self.vp))
            .collect();
        self.enter_hints(cells, HintSource::Page);
    }

    fn enter_reader_hints(&mut self) {
        let Some(layout) = self.focused().reader.layout.as_ref() else {
            self.focused_mut().state = State::Notice("no hints".to_string());
            return;
        };
        let visible = layout.visible_links(
            self.focused().reader.top_row,
            self.vp.origin_row(),
            self.vp.grid().rows,
        );
        let (links, cells): (Vec<_>, Vec<_>) = visible.into_iter().unzip();
        self.enter_hints(cells, HintSource::Reader(links));
    }

    fn enter_hints(&mut self, cells: Vec<CellPos>, source: HintSource) {
        let session = HintSession::new(cells);
        if session.is_empty() {
            // Entering a mode with nothing in it would only need escaping.
            self.focused_mut().state = State::Notice("no hints".to_string());
            self.hint_source = None;
            return;
        }
        self.mode = Mode::Hint(session);
        self.hint_source = Some(source);
    }

    fn clear_hints(&mut self) {
        if matches!(self.mode, Mode::Hint(_)) {
            self.mode = Mode::Normal;
        }
        self.hint_source = None;
    }

    /// Apply what filtering decided about the character just typed.
    fn on_filtered(&mut self, filtered: Filtered, effects: &mut Vec<Effect>) {
        match filtered {
            Filtered::Waiting(_) => {}
            Filtered::Activate(index) => {
                let source = self.hint_source.take();
                self.mode = Mode::Normal;
                match source {
                    Some(HintSource::Page) => {
                        let target = self
                            .focused()
                            .hints
                            .as_ref()
                            .and_then(|targets| targets.get(index))
                            .cloned();
                        if let Some(target) = target {
                            self.activate(target, effects);
                        }
                    }
                    Some(HintSource::Reader(links)) => {
                        if let Some(link) = links.get(index).copied() {
                            self.activate_reader(link, effects);
                        }
                    }
                    None => {}
                }
            }
            // Nothing matches, so there is nothing left to type. Leaving is
            // friendlier than sitting there waiting for an Esc.
            Filtered::None => {
                self.mode = Mode::Normal;
                self.hint_source = None;
            }
        }
    }

    fn activate_reader(&mut self, id: LinkId, effects: &mut Vec<Effect>) {
        let link = self
            .focused()
            .reader
            .document
            .as_ref()
            .and_then(|document| document.links.get(id.0))
            .cloned();
        let Some(link) = link else { return };
        if self.browser_lost {
            self.ask_for_a_browser(effects);
            return;
        }
        if link.new_tab {
            self.open_tab(link.url, effects);
        } else {
            self.navigate(Navigation::Open(link.url), effects);
        }
    }

    fn activate(&mut self, target: HintTarget, effects: &mut Vec<Effect>) {
        let at = target.center();
        let id = self.focused_id();
        effects.push(Effect::Send(id, Input::Mouse(MouseInput::press(at))));
        effects.push(Effect::Send(id, Input::Mouse(MouseInput::release(at))));
        // Clicking a text field is the beginning of typing into it, so that
        // is where the mode goes. Anything else is finished when the click
        // lands.
        self.mode = match target.kind {
            TargetKind::Editable => Mode::Insert,
            TargetKind::Clickable => Mode::Normal,
        };
    }

    fn on_resize(&mut self, grid: GridSize, cell: CellSize, effects: &mut Vec<Effect>) {
        if grid == self.grid && cell == self.cell {
            return;
        }
        self.grid = grid;
        self.cell = cell;
        self.vp = page_viewport(grid, cell);
        // Every tab, not just the one in front: a background tab laid out
        // for the terminal you used to have would be wrong the moment you
        // reached it, and reaching it is the one moment there is no time to
        // fix it in. The page genuinely reflows; extraction waits for
        // `Job::Resized`, because reading before the page has been resized
        // reads the old layout.
        for tab in &self.tabs {
            effects.push(Effect::SetViewport(tab.id, self.vp));
        }

        // Whatever picture is still up covers the page area it has now, or
        // the placeholders would address a placement of the wrong shape
        // until the next frame lands. A new generation with it, because the
        // renderer diffs on that and this image has to be placed again.
        // Half-block has no placement to correct: its cells are repainted
        // from whatever samples are in hand, and a grid the old picture is
        // too small for leaves the new edge blank until the next frame.
        if let Some(Picture::Graphics(image)) = &mut self.picture {
            self.generations += 1;
            image.generation = self.generations;
            image.area = CellRect::of(self.vp.grid(), self.vp.origin_row());
        }
        // The same tab, restarted at the new size: a screencast is started
        // with a viewport and does not learn about a later one.
        let focused = self.focused_id();
        self.follow_focus(Some(focused), effects);
    }

    fn on_job(&mut self, job: Job, effects: &mut Vec<Effect>) {
        let id = match &job {
            Job::Extracted(id, _, _)
            | Job::Reader(id, _)
            | Job::Status(id, _)
            | Job::Failed(id, _)
            | Job::Settled(id)
            | Job::Hints(id, _)
            | Job::Resized(id)
            | Job::Noted(id, _) => *id,
            // The one job with no tab: the session file is made of all of
            // them. It goes on the tab in front because that is the only
            // statusline there is.
            Job::Unsaved(message) => {
                self.focused_mut().state = State::Error(message.clone());
                return;
            }
            // The second job with no tab, and for the same reason: a browser
            // belongs to all of them.
            Job::Relaunched(result) => {
                self.relaunching = false;
                if let Err(message) = result {
                    // Stale frames and a label, never an exit. The tabs are
                    // already written down, so quitting is yours to choose.
                    self.focused_mut().state =
                        State::Error(format!("no browser: {message}. any key retries"));
                }
                return;
            }
            Job::Opened(id, _) => *id,
        };
        if self.tab_mut(id).is_none() {
            // The tab was closed while this was in flight. Its id is never
            // reused, so there is no page this could belong to instead.
            return;
        }

        match job {
            Job::Extracted(_, source, result) => {
                let extraction = match result {
                    Ok(extraction) => extraction,
                    Err(failure) => {
                        let tab = self.tab_mut(id).expect("resolved above");
                        tab.reading = false;
                        match (source, &failure) {
                            // A deadline is not a broken script. The page is
                            // not running, and `DOMSnapshot` needs the same
                            // main thread our script does, so asking it costs
                            // a second deadline to learn the same thing and
                            // leaves the tab degraded for good over a wedge
                            // that may last a second.
                            //
                            // Nothing is scheduled to try again either: a
                            // page wedged in a loop cannot run its own
                            // MutationObserver, so it sends no dirty signal,
                            // and one that recovers sends one and is read.
                            (_, Failure::TimedOut) => {
                                tab.state = State::Stalled;
                                return;
                            }
                            // The script broke. Read it the other way, once,
                            // and go on reading it that way until it
                            // navigates.
                            (Source::Script, _) => {
                                tab.degraded = true;
                                tab.dirty = true;
                            }
                            // There is no third source. The frame you are
                            // looking at stands and only the statusline
                            // changes, which is section 8 of the parent.
                            (Source::Snapshot, _) => tab.state = State::Error(failure.message()),
                        }
                        self.start_current_read(id, effects);
                        return;
                    }
                };
                let extraction = *extraction;
                let tab = self.tab_mut(id).expect("resolved above");
                tab.reading = false;
                tab.read = true;
                tab.runs = extraction.runs;
                tab.caret = extraction.caret;
                // Everything else an extraction carries is what a status read
                // carries, and is applied by the one place that knows how.
                self.apply_status(id, extraction.status, effects);
                // The page may have changed again while we were extracting.
                self.start_current_read(id, effects);
            }
            Job::Reader(_, result) => {
                self.tab_mut(id).expect("resolved above").reading = false;
                let extraction = match result {
                    Ok(extraction) => *extraction,
                    Err(failure) => {
                        let tab = self.tab_mut(id).expect("resolved above");
                        tab.state = match failure {
                            Failure::TimedOut => State::Stalled,
                            Failure::Failed(message) => State::Error(message),
                        };
                        if !tab.reader.active {
                            tab.reader.wanted = false;
                        }
                        self.start_current_read(id, effects);
                        return;
                    }
                };

                let was_active = self
                    .tab_mut(id)
                    .expect("resolved above")
                    .reader
                    .active;
                if extraction.document.blocks.is_empty() {
                    let tab = self.tab_mut(id).expect("resolved above");
                    tab.state = State::Notice("nothing to read".to_string());
                    if !was_active {
                        tab.reader.wanted = false;
                    }
                    self.start_current_read(id, effects);
                    return;
                }
                let document = extraction.document;
                let layout = Layout::new(&document, self.grid.cols);
                let max_top = layout
                    .rows()
                    .saturating_sub(usize::from(self.vp.grid().rows));
                let tab = self.tab_mut(id).expect("resolved above");
                tab.reader.top_row = tab.reader.top_row.min(max_top);
                tab.reader.document = Some(document);
                tab.reader.layout = Some(layout);
                tab.reader.active = tab.reader.wanted;
                let became_active = !was_active && tab.reader.active;
                self.apply_status(id, extraction.status, effects);
                if became_active && self.pixel && self.focused_id() == id {
                    effects.push(Effect::StopScreencast(id));
                }
                self.start_current_read(id, effects);
            }
            Job::Status(_, result) => {
                let tab = self.tab_mut(id).expect("resolved above");
                tab.reading = false;
                let status = match result {
                    Ok(status) => status,
                    // `status()` is the same injected script `extract()` is,
                    // so it breaks the same way and earns the same answer:
                    // degrade, and read the other way from now on. There is
                    // no `Source` to branch on because a status is only ever
                    // asked of a tab whose script works.
                    // A deadline is the exception, for the reason
                    // `Extracted` gives: the snapshot needs the main thread
                    // that did not answer.
                    Err(Failure::TimedOut) => {
                        tab.state = State::Stalled;
                        return;
                    }
                    Err(_) => {
                        tab.degraded = true;
                        tab.dirty = true;
                        self.start_current_read(id, effects);
                        return;
                    }
                };
                // Deliberately not `read`: a status carries no runs, and
                // `read` is what says the first switch to this tab can be a
                // repaint. Leaving pixel mode is what fills them in.
                self.apply_status(id, status, effects);
                self.start_current_read(id, effects);
            }
            Job::Hints(_, result) => {
                // However it went, that tab's query is over and `f` must
                // work on it again.
                if let Some(tab) = self.tab_mut(id) {
                    tab.hinting = false;
                }
                match result {
                    Ok(targets) => {
                        if let Some(tab) = self.tab_mut(id) {
                            tab.hints = Some(targets.clone());
                        }
                        // A query is a round trip, and the keystroke that
                        // asked for it was normal mode's, on a tab that was
                        // in front. Landing the answer in whatever mode you
                        // have since entered would take the command line out
                        // from under you mid-word, and landing it on another
                        // tab would paint one page's labels over another's
                        // text.
                        if self.mode == Mode::Normal
                            && self.focused_id() == id
                            && !self.focused().reader.active
                        {
                            self.enter_page_hints(targets);
                        }
                    }
                    Err(failure) => {
                        if let Some(tab) = self.tab_mut(id) {
                            // A hint query that was never answered says the
                            // page is not running, which is what `[stalled]`
                            // is for.
                            tab.state = match failure {
                                Failure::TimedOut => State::Stalled,
                                Failure::Failed(message) => State::Error(message),
                            };
                        }
                    }
                }
            }
            Job::Settled(_) => {
                let tab = self.tab_mut(id).expect("resolved above");
                tab.navigating = false;
                tab.state = State::Ready;
                tab.mark_dirty();
                self.start_current_read(id, effects);
            }
            Job::Resized(_) => {
                self.tab_mut(id).expect("resolved above").mark_dirty();
                self.start_current_read(id, effects);
            }
            Job::Opened(_, Ok(())) => {
                let tab = self.tab_mut(id).expect("resolved above");
                tab.presence = Presence::Attached;
                tab.navigating = false;
                tab.state = State::Ready;
                tab.mark_dirty();
                if self.focused_id() == id {
                    effects.push(Effect::Activate(id));
                    // The moment the picture can follow the focus here. It
                    // could not when the tab was made, because there was no
                    // page for `Core` to ask; nothing was left running to
                    // stop, because `leave_for_a_new_tab` did that then.
                    self.follow_focus(None, effects);
                }
                // The page is already at the offset it was restored to;
                // `Effect::OpenTab` carried it, so this reads what is there.
                self.start_current_read(id, effects);
            }
            Job::Opened(_, Err(message)) => {
                // A tab with no page behind it is not a tab. Drop it and say
                // why, without disturbing the one you were on.
                //
                // Through the caller's own effects, because closing decides
                // more than that a tab is gone: it asks to quit when that was
                // the last one, and hands the browser to the tab taking its
                // place. Those decisions were being made into a vector nobody
                // read, which left the browser in front of a page the session
                // had let go of, and left `focused_mut` below indexing a tab
                // list that closing had just emptied.
                //
                // `Effect::CloseTab` names a tab `Core` holds no page for and
                // is a no-op there. A target the browser did manage to create
                // is closed by `Core`, which is the only side that knows one
                // exists.
                self.close_tab(id, effects);
                // Closing the last tab asks to quit, and there is then no
                // statusline left to put the message on.
                if !self.tabs.is_empty() {
                    self.focused_mut().state = State::Error(message);
                }
            }
            Job::Failed(_, failure) => {
                let tab = self.tab_mut(id).expect("resolved above");
                tab.reading = false;
                tab.navigating = false;
                // The frame stays exactly as it was; only the statusline
                // changes. Section 8: never blank the frame you are looking at.
                //
                // A scroll or a navigation that was never answered says the
                // same thing about the page as a read that was not, so it
                // earns the same label.
                tab.state = match failure {
                    Failure::TimedOut => State::Stalled,
                    Failure::Failed(message) => State::Error(message),
                };
            }
            // The frame stays exactly as it was; only the statusline changes.
            // Spec section 8. Deliberately not `Job::Failed`: that one clears
            // the extraction and navigation flags, and a keystroke that
            // failed has finished neither of those.
            Job::Noted(_, message) => {
                self.tab_mut(id).expect("resolved above").state = State::Error(message);
            }
            Job::Unsaved(_) | Job::Relaunched(_) => unreachable!("answered above"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use wwt_frame::{Caret, CssRect, Rgb, Style, TextRun};
    use wwt_page::{Extraction, ReaderExtraction};
    use wwt_reader::{Block, BlockKind, Document, Layout, Link, LinkId, Span};

    const GRID: GridSize = GridSize { cols: 80, rows: 24 };
    const CELL: CellSize = CellSize { w: 9, h: 20 };

    fn session() -> Session {
        Session::new(GRID, CELL)
    }

    /// The id of the tab a fresh session starts with.
    fn tab0() -> TabId {
        TabId(0)
    }

    /// A session past its first extraction, the state most keys are pressed in.
    fn ready() -> Session {
        let mut session = session();
        session.begin();
        session.on(Event::Done(Job::Extracted(tab0(), Source::Script, Ok(Box::new(extraction("https://example.com"))))));
        session
    }

    fn extraction(url: &str) -> Extraction {
        Extraction { runs: Vec::new(), caret: None, status: status(url) }
    }

    fn status(url: &str) -> Status {
        Status {
            title: "Example".to_string(),
            url: url.to_string(),
            scroll_y: 0.0,
            scroll_height: 1000.0,
            viewport_height: 460.0,
        }
    }

    fn reader_extraction(text: &str) -> ReaderExtraction {
        ReaderExtraction {
            document: Document {
                blocks: vec![Block {
                    kind: BlockKind::Paragraph,
                    spans: vec![Span {
                        text: text.to_string(),
                        link: None,
                    }],
                }],
                links: Vec::new(),
            },
            status: status("https://example.com"),
        }
    }

    fn request_reader(session: &mut Session) -> Vec<Effect> {
        session.focused_mut().reader.wanted = true;
        let mut effects = Vec::new();
        session.start_reader(tab0(), &mut effects);
        effects
    }

    fn cache_reader(session: &mut Session, text: &str) {
        let document = reader_extraction(text).document;
        session.focused_mut().reader.layout = Some(Layout::new(&document, GRID.cols));
        session.focused_mut().reader.document = Some(document);
        session.focused_mut().reader.dirty = false;
    }

    fn enter_long_reader(session: &mut Session) -> usize {
        cache_reader(session, &"reader words ".repeat(1_000));
        assert_eq!(session.on(key('r')), vec![]);
        let max_top = session
            .focused()
            .reader
            .layout
            .as_ref()
            .expect("cached layout")
            .rows()
            .saturating_sub(usize::from(session.vp.grid().rows));
        assert!(max_top > usize::from(session.vp.grid().rows));
        max_top
    }

    fn enter_link_reader(session: &mut Session, links: Vec<Link>, spans: Vec<Span>) {
        let document = Document {
            blocks: vec![Block {
                kind: BlockKind::Paragraph,
                spans,
            }],
            links,
        };
        session.focused_mut().reader.layout = Some(Layout::new(&document, GRID.cols));
        session.focused_mut().reader.document = Some(document);
        session.focused_mut().reader.dirty = false;
        assert_eq!(session.on(key('r')), vec![]);
    }

    fn reader_link(url: &str, new_tab: bool) -> Link {
        Link {
            url: url.to_string(),
            new_tab,
        }
    }

    fn key(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    fn code(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    /// Alt and a digit, which is how a tab is reached.
    fn alt(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT))
    }

    fn target(kind: TargetKind) -> HintTarget {
        HintTarget { rect: CssRect { x: 90.0, y: 40.0, w: 90.0, h: 20.0 }, kind }
    }

    /// The page answering a hint query.
    fn hinted(targets: Vec<HintTarget>) -> Event {
        Event::Done(Job::Hints(tab0(), Ok(targets)))
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE })
    }

    fn typed(session: &mut Session, text: &str) {
        for c in text.chars() {
            session.on(key(c));
        }
    }

    // The extraction handshake: two flags, and the rule that a page which
    // changed while we were reading it gets read again — once.

    #[test]
    fn the_first_thing_a_session_does_is_read_the_page() {
        assert_eq!(session().begin(), vec![Effect::Extract(tab0(), Source::Script)]);
    }

    #[test]
    fn a_dirty_signal_during_an_extraction_re_runs_it_once_not_twice() {
        let mut session = session();
        assert_eq!(session.begin(), vec![Effect::Extract(tab0(), Source::Script)]);

        // Three signals arrive while that extraction is still in flight.
        for _ in 0..3 {
            assert_eq!(session.on(Event::Dirty(tab0())), vec![], "a second extraction would race it");
        }

        // Finishing it starts exactly one more, covering all three. The tab
        // had no url until now, so this is also the first thing worth
        // writing down.
        let effects = session.on(Event::Done(Job::Extracted(tab0(), Source::Script, Ok(Box::new(extraction("about:blank"))))));
        assert_eq!(
            effects,
            vec![Effect::Save(session.snapshot()), Effect::Extract(tab0(), Source::Script)]
        );
    }

    #[test]
    fn a_page_that_stopped_changing_stops_being_read() {
        let mut session = ready();
        // The same page `ready` left it on: an idle page is one that did not
        // move, so extracting it again must cost neither a read nor a write.
        assert_eq!(
            session.on(Event::Done(Job::Extracted(tab0(), Source::Script, Ok(Box::new(extraction("https://example.com")))))),
            vec![],
            "an idle page must cost nothing"
        );
    }

    // Reader extraction shares the ordinary read slot and caches its answer.

    #[test]
    fn one_reader_request_uses_the_shared_read_slot() {
        let mut session = ready();

        assert_eq!(request_reader(&mut session), vec![Effect::ReadReader(tab0())]);
        assert!(session.focused().reading);
        assert!(!session.focused().reader.dirty);

        let mut effects = Vec::new();
        session.start_reader(tab0(), &mut effects);
        session.focused_mut().dirty = true;
        session.start_extract(tab0(), &mut effects);
        assert_eq!(effects, Vec::<Effect>::new(), "a second page read would race the first");
    }

    #[test]
    fn an_ordinary_answer_hands_the_shared_slot_to_a_pending_reader() {
        let mut session = session();
        assert_eq!(session.begin(), vec![Effect::Extract(tab0(), Source::Script)]);

        assert_eq!(request_reader(&mut session), vec![]);

        let effects = session.on(Event::Done(Job::Extracted(
            tab0(),
            Source::Script,
            Ok(Box::new(extraction("https://example.com"))),
        )));
        assert!(effects.contains(&Effect::ReadReader(tab0())));
        assert!(session.focused().reading);
    }

    #[test]
    fn a_reader_answer_is_cached_and_laid_out_at_the_current_width() {
        let mut session = ready();
        request_reader(&mut session);
        session.focused_mut().reader.top_row = usize::MAX;
        let extraction = reader_extraction("reader text");
        let expected_document = extraction.document.clone();
        let expected_layout = Layout::new(&expected_document, GRID.cols);

        let effects = session.on(Event::Done(Job::Reader(tab0(), Ok(Box::new(extraction)))));

        assert_eq!(effects, Vec::<Effect>::new());
        assert_eq!(session.focused().reader.document, Some(expected_document));
        assert_eq!(session.focused().reader.layout, Some(expected_layout));
        assert_eq!(session.focused().reader.top_row, 0);
        assert!(session.focused().reader.active);
        assert!(!session.focused().reading);
    }

    #[test]
    fn cancelling_reader_entry_before_the_answer_only_warms_the_cache() {
        let mut session = ready();
        request_reader(&mut session);
        session.focused_mut().reader.wanted = false;

        session.on(Event::Done(Job::Reader(
            tab0(),
            Ok(Box::new(reader_extraction("cached text"))),
        )));

        assert!(session.focused().reader.document.is_some());
        assert!(session.focused().reader.layout.is_some());
        assert!(!session.focused().reader.active);
    }

    #[test]
    fn a_reader_refresh_timeout_keeps_the_old_active_layout() {
        let mut session = ready();
        let old_document = reader_extraction("old text").document;
        let old_layout = Layout::new(&old_document, GRID.cols);
        session.focused_mut().reader.document = Some(old_document);
        session.focused_mut().reader.layout = Some(old_layout.clone());
        session.focused_mut().reader.active = true;
        session.focused_mut().reader.wanted = true;
        request_reader(&mut session);

        session.on(Event::Done(Job::Reader(tab0(), Err(Failure::TimedOut))));

        assert_eq!(*session.state(), State::Stalled);
        assert_eq!(session.focused().reader.layout, Some(old_layout));
        assert!(session.focused().reader.active);
        assert!(session.focused().reader.wanted);
        assert!(!session.focused().degraded);
        assert!(!session.focused().reading);
    }

    #[test]
    fn a_first_reader_failure_keeps_page_view_and_does_not_degrade_the_tab() {
        let mut session = ready();
        request_reader(&mut session);

        session.on(Event::Done(Job::Reader(
            tab0(),
            Err(Failure::Failed("reader failed".to_string())),
        )));

        assert_eq!(*session.state(), State::Error("reader failed".to_string()));
        assert!(!session.focused().reader.active);
        assert!(!session.focused().reader.wanted);
        assert!(session.focused().reader.layout.is_none());
        assert!(!session.focused().degraded);
        assert!(!session.focused().reading);
    }

    #[test]
    fn a_first_empty_reader_answer_keeps_the_live_page() {
        let mut session = ready();
        session.focused_mut().runs = vec![run("live page")];
        session.on(key('r'));
        let mut extraction = reader_extraction("");
        extraction.document.blocks.clear();

        assert_eq!(
            session.on(Event::Done(Job::Reader(tab0(), Ok(Box::new(extraction))))),
            vec![]
        );

        assert!(!session.focused().reader.active);
        assert!(!session.focused().reader.wanted);
        assert!(session.focused().reader.document.is_none());
        assert!(session.focused().reader.layout.is_none());
        assert!(session.compose().row_text(1).contains("live page"));
    }

    #[test]
    fn a_refused_reader_refresh_keeps_layout_and_waits_for_another_signal() {
        let mut session = ready();
        cache_reader(&mut session, "old reader layout");
        session.on(key('r'));
        let old = session.focused().reader.layout.clone();
        session.on(Event::Dirty(tab0()));

        assert_eq!(
            session.on(Event::Done(Job::Reader(
                tab0(),
                Err(Failure::Failed("reader refused".to_string()))
            ))),
            vec![]
        );
        assert_eq!(session.focused().reader.layout, old);
        assert!(session.focused().reader.active);
        assert!(session.focused().reader.wanted);
        assert_eq!(session.state(), &State::Error("reader refused".to_string()));
        assert!(!session.focused().degraded);

        assert_eq!(session.on(Event::Dirty(tab0())), vec![Effect::ReadReader(tab0())]);
    }

    #[test]
    fn a_dirty_signal_during_a_reader_query_produces_one_follow_up() {
        let mut session = ready();
        assert_eq!(request_reader(&mut session), vec![Effect::ReadReader(tab0())]);

        for _ in 0..3 {
            assert_eq!(session.on(Event::Dirty(tab0())), vec![]);
        }

        let effects = session.on(Event::Done(Job::Reader(
            tab0(),
            Ok(Box::new(reader_extraction("first answer"))),
        )));
        assert_eq!(effects, vec![Effect::ReadReader(tab0())]);
    }

    #[test]
    fn dirty_active_reader_refreshes_only_the_document_in_front() {
        let mut session = ready();
        enter_long_reader(&mut session);
        session.focused_mut().reader.top_row = 20;
        let before = session.compose();

        assert_eq!(session.on(Event::Dirty(tab0())), vec![Effect::ReadReader(tab0())]);
        assert!(session.focused().dirty, "ordinary runs became stale too");
        assert_eq!(session.focused().reader.top_row, 20);
        assert_eq!(session.compose(), before, "the old layout stands while its refresh is away");

        assert_eq!(session.on(Event::Dirty(tab0())), vec![], "one shared read is enough");
        assert!(session.focused().reader.dirty, "the second signal is remembered");

        let effects = session.on(Event::Done(Job::Reader(
            tab0(),
            Ok(Box::new(reader_extraction("short replacement"))),
        )));
        assert_eq!(effects, vec![Effect::ReadReader(tab0())]);
        assert_eq!(session.focused().reader.top_row, 0, "replacement clamps the numeric row");
    }

    #[test]
    fn dirty_inactive_reader_refreshes_only_the_live_page() {
        let mut session = ready();
        cache_reader(&mut session, "cached reader");

        assert_eq!(session.on(Event::Dirty(tab0())), vec![Effect::Extract(tab0(), Source::Script)]);
        assert!(session.focused().reader.dirty);
        assert!(!session.focused().reader.active);
        assert!(!session.focused().reader.wanted);
    }

    #[test]
    fn first_reader_entry_keeps_the_live_page_while_the_query_is_away() {
        let mut session = ready();
        session.focused_mut().runs = vec![run("live page")];

        let effects = session.on(key('r'));

        assert_eq!(effects, vec![Effect::ReadReader(tab0())]);
        assert!(session.focused().reader.wanted);
        assert!(!session.focused().reader.active);
        assert!(session.compose().row_text(1).contains("live page"));
        assert_eq!(*session.state(), State::Notice("reading".to_string()));
    }

    #[test]
    fn a_clean_reader_cache_enters_without_a_page_effect() {
        let mut session = ready();
        session.focused_mut().runs = vec![run("live page")];
        cache_reader(&mut session, "reader page");

        assert_eq!(session.on(key('r')), vec![]);

        assert!(session.focused().reader.active);
        assert!(session.focused().reader.wanted);
        assert!(session.compose().row_text(1).contains("reader page"));
        assert!(!session.compose().row_text(1).contains("live page"));
    }

    #[test]
    fn a_second_r_cancels_pending_entry_and_a_late_answer_only_warms_the_cache() {
        let mut session = ready();
        assert_eq!(session.on(key('r')), vec![Effect::ReadReader(tab0())]);

        assert_eq!(session.on(key('r')), vec![]);
        assert!(!session.focused().reader.wanted);
        assert!(!session.focused().reader.active);

        session.on(Event::Done(Job::Reader(
            tab0(),
            Ok(Box::new(reader_extraction("late reader"))),
        )));
        assert!(session.focused().reader.document.is_some());
        assert!(session.focused().reader.layout.is_some());
        assert!(!session.focused().reader.active);
    }

    #[test]
    fn leaving_reader_repaints_live_runs_without_moving_the_page() {
        let mut session = ready();
        session.focused_mut().runs = vec![run("live page")];
        session.focused_mut().scroll_y = 360.0;
        cache_reader(&mut session, "reader page");
        session.on(key('r'));
        session.focused_mut().dirty = true;

        let effects = session.on(key('r'));

        assert_eq!(effects, vec![Effect::Extract(tab0(), Source::Script)]);
        assert!(!session.focused().reader.active);
        assert!(!session.focused().reader.wanted);
        assert_eq!(session.focused().scroll_y, 360.0);
        assert!(session.compose().row_text(1).contains("live page"));
        assert!(!effects.iter().any(|effect| matches!(effect, Effect::Scroll(..) | Effect::Save(_))));
    }

    #[test]
    fn the_statusline_uses_reader_progress_only_while_reader_is_active() {
        let mut session = ready();
        cache_reader(&mut session, &"reader words ".repeat(500));
        session.focused_mut().progress = 0.75;
        session.focused_mut().reader.active = true;
        session.focused_mut().reader.wanted = true;
        let max_top = session
            .focused()
            .reader
            .layout
            .as_ref()
            .expect("cached layout")
            .rows()
            .saturating_sub(usize::from(session.vp.grid().rows));
        session.focused_mut().reader.top_row = max_top / 2;

        let reader_line = session.compose().row_text(GRID.rows - 1);
        let reader_percent = ((max_top / 2) as f64 / max_top as f64 * 100.0).round() as i64;
        assert!(reader_line.ends_with(&format!(" {reader_percent:>3}%")));

        session.focused_mut().reader.active = false;
        let page_line = session.compose().row_text(GRID.rows - 1);
        assert!(page_line.ends_with(" 75%"));
    }

    #[test]
    fn reader_keys_scroll_rows_locally_and_clamp_at_both_ends() {
        let mut session = ready();
        let max_top = enter_long_reader(&mut session);

        let cases = [
            (key('j'), 1),
            (key('k'), 0),
            (key('d'), 11),
            (key('u'), 0),
            (key(' '), 20),
            (key('b'), 0),
            (key('G'), max_top),
            (key('j'), max_top),
            (key('g'), 0),
            (key('k'), 0),
        ];

        for (event, top_row) in cases {
            assert_eq!(session.on(event), vec![]);
            assert_eq!(session.focused().reader.top_row, top_row);
        }
    }

    #[test]
    fn reader_scrolling_changes_reader_progress_without_saving_the_page() {
        let mut session = ready();
        let max_top = enter_long_reader(&mut session);
        let before = session.compose().row_text(GRID.rows - 1);

        let effects = session.on(key('d'));
        let after = session.compose().row_text(GRID.rows - 1);

        assert_eq!(effects, vec![]);
        assert_ne!(before, after);
        let percent = (11.0 / max_top as f64 * 100.0).round() as i64;
        assert!(after.ends_with(&format!(" {percent:>3}%")));
    }

    #[test]
    fn reader_scrolling_stays_local_while_the_browser_is_gone() {
        let mut session = ready();
        enter_long_reader(&mut session);
        session.on(Event::BrowserLost);

        assert_eq!(session.on(key('j')), vec![]);
        assert_eq!(session.focused().reader.top_row, 1);
    }

    #[test]
    fn reader_hints_label_visible_distinct_links_without_querying_the_page() {
        let mut session = ready();
        enter_link_reader(
            &mut session,
            vec![reader_link("https://one.example", false), reader_link("https://two.example", false)],
            vec![
                Span { text: "one".to_string(), link: Some(LinkId(0)) },
                Span { text: " and ".to_string(), link: None },
                Span { text: "one again".to_string(), link: Some(LinkId(0)) },
                Span { text: " then two".to_string(), link: Some(LinkId(1)) },
            ],
        );

        assert_eq!(session.on(key('f')), vec![]);
        assert!(matches!(session.mode(), Mode::Hint(_)));

        let frame = session.compose();
        assert_eq!(frame.cell(CellPos { col: 0, row: 1 }).expect("first link cell").ch, 's');
        assert_eq!(frame.cell(CellPos { col: 17, row: 1 }).expect("second link cell").ch, 'a');
    }

    #[test]
    fn reader_with_no_visible_links_reports_no_hints_locally() {
        let mut session = ready();
        cache_reader(&mut session, "plain reader text");
        session.on(key('r'));

        assert_eq!(session.on(key('f')), vec![]);
        assert_eq!(session.mode(), &Mode::Normal);
        assert_eq!(session.state(), &State::Notice("no hints".to_string()));
    }

    #[test]
    fn selecting_a_same_tab_reader_hint_leaves_reader_and_navigates() {
        let mut session = ready();
        enter_link_reader(
            &mut session,
            vec![reader_link("https://next.example", false)],
            vec![Span { text: "next".to_string(), link: Some(LinkId(0)) }],
        );
        session.on(key('f'));

        assert_eq!(
            session.on(key('s')),
            vec![Effect::Navigate(
                tab0(),
                Navigation::Open("https://next.example".to_string())
            )]
        );
        assert!(!session.focused().reader.active);
        assert!(!session.focused().reader.wanted);
        assert!(session.focused().reader.document.is_none());
        assert!(session.focused().reader.layout.is_none());
        assert_eq!(session.focused().reader.top_row, 0);
        assert_eq!(session.mode(), &Mode::Normal);
    }

    #[test]
    fn selecting_a_new_tab_reader_hint_keeps_the_source_reader_intact() {
        let mut session = ready();
        enter_link_reader(
            &mut session,
            vec![reader_link("https://new.example", true)],
            vec![Span { text: "new".to_string(), link: Some(LinkId(0)) }],
        );
        session.on(key('f'));

        let effects = session.on(key('s'));

        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::OpenTab { url, .. } if url == "https://new.example"
        )));
        assert!(session.tabs[0].reader.active);
        assert!(session.tabs[0].reader.wanted);
        assert_eq!(session.focus, 1);
        assert_eq!(session.mode(), &Mode::Normal);
    }

    #[test]
    fn reader_wheel_scrolls_three_rows_locally_and_clamps() {
        let mut session = ready();
        let max_top = enter_long_reader(&mut session);

        assert_eq!(session.on(mouse(MouseEventKind::ScrollDown, 0, 1)), vec![]);
        assert_eq!(session.focused().reader.top_row, 3);
        assert_eq!(session.on(mouse(MouseEventKind::ScrollUp, 0, 1)), vec![]);
        assert_eq!(session.focused().reader.top_row, 0);

        session.focused_mut().reader.top_row = max_top - 1;
        assert_eq!(session.on(mouse(MouseEventKind::ScrollDown, 0, 1)), vec![]);
        assert_eq!(session.focused().reader.top_row, max_top);
    }

    #[test]
    fn reader_mouse_press_follows_a_link_without_sending_page_input() {
        let mut session = ready();
        enter_link_reader(
            &mut session,
            vec![reader_link("https://mouse.example", false)],
            vec![Span { text: "mouse link".to_string(), link: Some(LinkId(0)) }],
        );

        assert_eq!(
            session.on(mouse(MouseEventKind::Down(MouseButton::Left), 0, 1)),
            vec![Effect::Navigate(
                tab0(),
                Navigation::Open("https://mouse.example".to_string())
            )]
        );
        assert!(!session.focused().reader.active);
    }

    #[test]
    fn reader_mouse_ignores_empty_cells_and_consumes_releases() {
        let mut session = ready();
        enter_link_reader(
            &mut session,
            vec![reader_link("https://mouse.example", false)],
            vec![Span { text: "mouse link".to_string(), link: Some(LinkId(0)) }],
        );

        assert_eq!(session.on(mouse(MouseEventKind::Down(MouseButton::Left), 40, 1)), vec![]);
        assert_eq!(session.on(mouse(MouseEventKind::Up(MouseButton::Left), 0, 1)), vec![]);
        assert!(session.focused().reader.active);
    }

    #[test]
    fn following_a_reader_link_relaunches_a_missing_browser() {
        let mut session = ready();
        enter_link_reader(
            &mut session,
            vec![reader_link("https://gone.example", false)],
            vec![Span { text: "gone".to_string(), link: Some(LinkId(0)) }],
        );
        session.browser_lost = true;
        session.relaunching = false;

        assert_eq!(session.on(key('f')), vec![]);
        assert!(matches!(session.mode(), Mode::Hint(_)));
        assert_eq!(session.on(key('s')), vec![Effect::Relaunch]);
        assert!(session.focused().reader.active);
    }

    #[test]
    fn reader_actions_that_need_the_page_relaunch_a_missing_browser() {
        for event in [key('i'), key('H')] {
            let mut session = ready();
            cache_reader(&mut session, "reader text");
            session.on(key('r'));
            session.browser_lost = true;
            session.relaunching = false;

            assert_eq!(session.on(event), vec![Effect::Relaunch]);
            assert!(session.focused().reader.active);
            assert_eq!(session.mode(), &Mode::Normal);
        }
    }

    #[test]
    fn every_navigation_entry_leaves_reader_and_forgets_its_old_document() {
        let key_cases = [
            (key('H'), Navigation::Back),
            (key('L'), Navigation::Forward),
            (ctrl('r'), Navigation::Reload),
        ];
        for (event, navigation) in key_cases {
            let mut session = ready();
            enter_long_reader(&mut session);
            session.focused_mut().reader.top_row = 7;

            assert_eq!(session.on(event), vec![Effect::Navigate(tab0(), navigation)]);
            assert_reader_was_replaced(&session);
        }

        let command_cases = [
            (":back", Navigation::Back),
            (":forward", Navigation::Forward),
            (":reload", Navigation::Reload),
            (
                ":open https://next.example",
                Navigation::Open("https://next.example".to_string()),
            ),
        ];
        for (command, navigation) in command_cases {
            let mut session = ready();
            enter_long_reader(&mut session);
            session.focused_mut().reader.top_row = 7;
            typed(&mut session, command);

            assert_eq!(session.on(code(KeyCode::Enter)), vec![Effect::Navigate(tab0(), navigation)]);
            assert_reader_was_replaced(&session);
        }
    }

    fn assert_reader_was_replaced(session: &Session) {
        assert!(!session.focused().reader.active);
        assert!(!session.focused().reader.wanted);
        assert!(session.focused().reader.document.is_none());
        assert!(session.focused().reader.layout.is_none());
        assert_eq!(session.focused().reader.top_row, 0);
    }

    #[test]
    fn insert_leaves_reader_but_keeps_its_reusable_cache() {
        let mut session = ready();
        cache_reader(&mut session, "reader cache");
        session.on(key('r'));
        let document = session.focused().reader.document.clone();
        let layout = session.focused().reader.layout.clone();

        assert_eq!(session.on(key('i')), vec![]);
        assert_eq!(session.mode(), &Mode::Insert);
        assert!(!session.focused().reader.active);
        assert!(!session.focused().reader.wanted);
        assert_eq!(session.focused().reader.document, document);
        assert_eq!(session.focused().reader.layout, layout);
        assert!(!session.focused().reader.dirty);
    }

    #[test]
    fn pixel_key_leaves_reader_for_pixels_without_discarding_the_cache() {
        let mut session = ready_with_graphics();
        cache_reader(&mut session, "reader cache");
        session.on(key('r'));
        let document = session.focused().reader.document.clone();
        let layout = session.focused().reader.layout.clone();

        assert_eq!(
            session.on(key('p')),
            vec![Effect::StartScreencast(tab0(), session.frame_size())]
        );
        assert!(session.pixel);
        assert!(!session.focused().reader.active);
        assert!(!session.focused().reader.wanted);
        assert_eq!(session.focused().reader.document, document);
        assert_eq!(session.focused().reader.layout, layout);
    }

    #[test]
    fn opening_a_reader_link_does_not_stop_an_already_stopped_screencast() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let document = Document {
            blocks: vec![Block {
                kind: BlockKind::Paragraph,
                spans: vec![Span { text: "new".to_string(), link: Some(LinkId(0)) }],
            }],
            links: vec![reader_link("https://new.example", true)],
        };
        session.focused_mut().reader.layout = Some(Layout::new(&document, GRID.cols));
        session.focused_mut().reader.document = Some(document);
        session.focused_mut().reader.dirty = false;
        assert_eq!(session.on(key('r')), vec![Effect::StopScreencast(tab0())]);
        session.on(key('f'));

        let effects = session.on(key('s'));

        assert!(!effects.contains(&Effect::StopScreencast(tab0())));
    }

    #[test]
    fn a_failed_extraction_lets_the_next_one_start() {
        let mut session = session();
        session.begin();
        session.on(Event::Done(Job::Failed(tab0(), Failure::Failed("boom".to_string()))));
        assert_eq!(session.on(Event::Dirty(tab0())), vec![Effect::Extract(tab0(), Source::Script)]);
    }

    // What a finished job says about the page.

    #[test]
    fn a_chrome_error_url_is_an_error_without_becoming_the_url() {
        let mut session = ready();
        session.on(Event::Dirty(tab0()));
        session.on(Event::Done(Job::Extracted(tab0(), Source::Script, Ok(Box::new(extraction(
            "chrome-error://chromewebdata/",
        ))))));

        assert_eq!(session.state(), &State::Error("could not be reached".to_string()));
        assert!(
            !session.compose().row_text(23).contains("chrome-error"),
            "the statusline prints the URL itself, so naming it twice reads as a bug"
        );
    }

    #[test]
    fn a_keystroke_that_failed_leaves_the_page_alone() {
        let mut session = ready();
        session.on(Event::Dirty(tab0()));
        let mid_extraction = session.on(Event::Done(Job::Noted(tab0(), "no".to_string())));

        assert_eq!(session.state(), &State::Error("no".to_string()));
        assert_eq!(mid_extraction, vec![], "an extraction was in flight and still is");
        // The one already running still finishes, and finds nothing to do.
        assert_eq!(
            session.on(Event::Done(Job::Extracted(tab0(), Source::Script, Ok(Box::new(extraction("https://example.com")))))),
            vec![]
        );
    }

    #[test]
    fn a_failure_never_blanks_the_frame() {
        let mut session = ready();
        let before = session.compose();
        session.on(Event::Done(Job::Failed(tab0(), Failure::Failed("the page went away".to_string()))));
        let after = session.compose();

        let rows = |f: &Frame| (0..23).map(|r| f.row_text(r)).collect::<Vec<_>>();
        assert_eq!(rows(&before), rows(&after), "only the statusline may change");
        assert!(after.row_text(23).contains("the page went away"));
    }

    // Modes: entering them, leaving them, and what leaving costs.

    #[test]
    fn esc_leaves_insert_mode_and_blurs() {
        let mut session = ready();
        session.on(key('i'));
        assert_eq!(session.mode(), &Mode::Insert);

        assert_eq!(session.on(code(KeyCode::Esc)), vec![Effect::Blur(tab0())]);
        assert_eq!(
            session.mode(),
            &Mode::Normal,
            "taking the keyboard back must never depend on the page"
        );
    }

    #[test]
    fn esc_out_of_the_other_modes_costs_nothing() {
        let mut session = ready();
        session.on(key(':'));
        assert_eq!(session.on(code(KeyCode::Esc)), vec![]);
        assert_eq!(session.mode(), &Mode::Normal);
    }

    #[test]
    fn the_page_never_sees_an_escape_it_was_not_sent_on_purpose() {
        let mut session = ready();
        session.on(key('i'));
        assert_eq!(session.on(code(KeyCode::Esc)), vec![Effect::Blur(tab0())], "ours, always");

        session.on(key('i'));
        let sent = session.on(ctrl(']'));
        let [Effect::Send(_, Input::Key(sent))] = sent.as_slice() else {
            panic!("Ctrl-] should reach the page, got {sent:?}");
        };
        assert_eq!(sent.key, "Escape");
        assert_eq!(session.mode(), &Mode::Insert, "sending one does not leave the mode");
    }

    #[test]
    fn typing_in_insert_mode_reaches_the_page_in_order() {
        let mut session = ready();
        session.on(key('i'));

        let mut sent = Vec::new();
        for c in "abc".chars() {
            for effect in session.on(key(c)) {
                if let Effect::Send(_, Input::Key(key)) = effect {
                    sent.push(key.text);
                }
            }
        }
        assert_eq!(sent, vec!["a", "b", "c"]);
    }

    #[test]
    fn the_command_line_collects_what_you_type_and_runs_it() {
        let mut session = ready();
        typed(&mut session, ":open example.com");
        assert!(matches!(session.mode(), Mode::Command(b) if b == "open example.com"));

        session.on(code(KeyCode::Backspace));
        let effects = session.on(code(KeyCode::Enter));
        assert_eq!(
            effects,
            vec![Effect::Navigate(tab0(), Navigation::Open("https://example.co".to_string()))],
            "backspace rubbed out the m"
        );
    }

    #[test]
    fn a_command_that_does_not_parse_says_so_and_changes_nothing_else() {
        let mut session = ready();
        typed(&mut session, ":wat");
        assert_eq!(session.on(code(KeyCode::Enter)), vec![]);
        assert!(matches!(session.state(), State::Error(_)), "state was {:?}", session.state());
        assert_eq!(session.mode(), &Mode::Normal);
    }

    #[test]
    fn setting_the_mouse_is_an_effect_rather_than_something_to_remember() {
        let mut session = ready();
        typed(&mut session, ":set mouse off");
        assert_eq!(session.on(code(KeyCode::Enter)), vec![Effect::MouseCapture(false)]);
        assert_eq!(session.state(), &State::Notice("mouse off".to_string()));
    }

    // Navigation: one at a time, and the frame you are looking at stays.

    #[test]
    fn a_second_navigation_while_one_is_in_flight_is_dropped() {
        let mut session = ready();
        assert_eq!(session.on(key('H')), vec![Effect::Navigate(tab0(), Navigation::Back)]);
        assert_eq!(session.on(key('L')), vec![], "one navigation at a time");

        session.on(Event::Done(Job::Settled(tab0())));
        assert_eq!(session.on(key('L')), vec![Effect::Navigate(tab0(), Navigation::Forward)]);
    }

    #[test]
    fn a_settled_navigation_reads_the_new_page() {
        let mut session = ready();
        session.on(key('H'));
        assert_eq!(session.on(Event::Done(Job::Settled(tab0()))), vec![Effect::Extract(tab0(), Source::Script)]);
        assert_eq!(session.state(), &State::Ready);
    }

    #[test]
    fn every_scroll_key_keeps_its_live_page_distance() {
        let mut session = ready();
        let cases = [
            (key('j'), Scroll::By(20.0)),
            (code(KeyCode::Down), Scroll::By(20.0)),
            (key('k'), Scroll::By(-20.0)),
            (code(KeyCode::Up), Scroll::By(-20.0)),
            (key('d'), Scroll::By(220.0)),
            (key('u'), Scroll::By(-220.0)),
            (key(' '), Scroll::By(400.0)),
            (code(KeyCode::PageDown), Scroll::By(400.0)),
            (key('b'), Scroll::By(-400.0)),
            (code(KeyCode::PageUp), Scroll::By(-400.0)),
            (key('g'), Scroll::Top),
            (code(KeyCode::Home), Scroll::Top),
            (key('G'), Scroll::End),
            (code(KeyCode::End), Scroll::End),
        ];

        for (event, scroll) in cases {
            assert_eq!(session.on(event), vec![Effect::Scroll(tab0(), scroll)]);
        }
    }

    // Hints: cached until the page moves, and a text field lands in insert.

    #[test]
    fn f_queries_the_page_once_and_then_uses_what_it_said() {
        let mut session = ready();
        assert_eq!(session.on(key('f')), vec![Effect::Hints(tab0(), Source::Script)]);

        session.on(hinted(vec![target(TargetKind::Clickable)]));
        assert!(matches!(session.mode(), Mode::Hint(_)));

        session.on(code(KeyCode::Esc));
        assert_eq!(session.on(key('f')), vec![], "a page that has not moved is not asked twice");
        assert!(matches!(session.mode(), Mode::Hint(_)));
    }

    #[test]
    fn a_page_that_moved_has_no_hints_left_to_reuse() {
        let mut session = ready();
        session.on(key('f'));
        session.on(hinted(vec![target(TargetKind::Clickable)]));
        session.on(code(KeyCode::Esc));

        session.on(Event::Dirty(tab0()));
        assert_eq!(
            session.on(key('f')),
            vec![Effect::Hints(tab0(), Source::Script)],
            "hints are geometry, so a page that moved has invalidated them"
        );
    }

    #[test]
    fn a_page_with_nothing_to_hint_says_so_rather_than_opening_an_empty_mode() {
        let mut session = ready();
        session.on(key('f'));
        session.on(hinted(Vec::new()));

        assert_eq!(session.mode(), &Mode::Normal, "a mode with nothing in it only needs escaping");
        assert_eq!(session.state(), &State::Notice("no hints".to_string()));
    }

    #[test]
    fn hinting_a_text_field_clicks_it_and_enters_insert() {
        let mut session = ready();
        session.on(key('f'));
        session.on(hinted(vec![target(TargetKind::Editable)]));

        // One target, so its label is one character: the first of the
        // alphabet, and typing it activates.
        let effects = session.on(key('s'));
        let at = target(TargetKind::Editable).center();
        assert_eq!(
            effects,
            vec![
                Effect::Send(tab0(), Input::Mouse(MouseInput::press(at))),
                Effect::Send(tab0(), Input::Mouse(MouseInput::release(at))),
            ]
        );
        assert_eq!(session.mode(), &Mode::Insert, "clicking a field is the start of typing in it");
    }

    #[test]
    fn hinting_a_link_leaves_you_where_you_were() {
        let mut session = ready();
        session.on(key('f'));
        session.on(hinted(vec![target(TargetKind::Clickable)]));

        let effects = session.on(key('s'));
        let at = target(TargetKind::Clickable).center();
        assert_eq!(
            effects,
            vec![
                Effect::Send(tab0(), Input::Mouse(MouseInput::press(at))),
                Effect::Send(tab0(), Input::Mouse(MouseInput::release(at))),
            ]
        );
        assert_eq!(session.mode(), &Mode::Normal, "a link is finished when the click lands");
    }

    #[test]
    fn a_query_still_in_flight_is_not_asked_again() {
        let mut session = ready();
        assert_eq!(session.on(key('f')), vec![Effect::Hints(tab0(), Source::Script)]);
        assert_eq!(session.on(key('f')), vec![], "one question, not two");
    }

    #[test]
    fn a_late_answer_does_not_take_the_mode_you_have_since_entered() {
        let mut session = ready();
        session.on(key('f'));

        // A query is a round trip, and you have not stopped typing.
        session.on(key(':'));
        session.on(key('o'));
        session.on(hinted(vec![target(TargetKind::Clickable)]));

        assert_eq!(
            session.mode(),
            &Mode::Command("o".to_string()),
            "the answer to `f` took the command line out from under you"
        );
        // The targets are still good geometry, so the next `f` is free.
        session.on(code(KeyCode::Esc));
        assert_eq!(session.on(key('f')), vec![]);
        assert!(matches!(session.mode(), Mode::Hint(_)));
    }

    #[test]
    fn a_query_that_failed_leaves_f_working() {
        let mut session = ready();
        session.on(key('f'));
        session.on(Event::Done(Job::Hints(tab0(), Err(Failure::Failed("the page went away".to_string())))));

        assert_eq!(session.state(), &State::Error("the page went away".to_string()));
        assert_eq!(
            session.on(key('f')),
            vec![Effect::Hints(tab0(), Source::Script)],
            "a failed query that never cleared its flag would kill `f` for the session"
        );
    }

    #[test]
    fn a_label_nothing_matches_leaves_hint_mode() {
        let mut session = ready();
        session.on(key('f'));
        session.on(hinted(vec![
            target(TargetKind::Clickable),
            target(TargetKind::Clickable),
        ]));
        session.on(key('z'));
        assert_eq!(session.mode(), &Mode::Normal, "nothing left to type is nothing to wait for");
    }

    #[test]
    fn a_stale_hint_index_leaves_hint_mode_without_clicking() {
        let mut session = ready();
        session.on(key('f'));
        session.on(hinted(vec![target(TargetKind::Clickable)]));
        session.on(Event::Dirty(tab0()));

        let effects = session.on(key('s'));

        assert_eq!(effects, Vec::new());
        assert_eq!(session.mode(), &Mode::Normal);
    }

    #[test]
    fn the_cursor_follows_the_command_line_and_nothing_else() {
        let mut session = ready();
        assert_eq!(session.compose().cursor(), None, "normal mode has no insertion point");

        typed(&mut session, ":open");
        assert_eq!(
            session.compose().cursor(),
            // The `:` plus five characters, on the chrome row.
            Some(CellPos { col: 5, row: GRID.rows - 1 })
        );

        session.on(code(KeyCode::Esc));
        assert_eq!(session.compose().cursor(), None, "leaving takes the caret with it");
    }

    // The mouse, and the row the page does not know about.

    #[test]
    fn a_click_on_the_page_lands_in_the_cell_you_pointed_at() {
        let mut session = ready();
        let effects = session.on(mouse(MouseEventKind::Down(MouseButton::Left), 4, 2));
        let at = session.viewport().to_css(CellPos { col: 4, row: 2 });
        assert_eq!(effects, vec![Effect::Send(tab0(), Input::Mouse(MouseInput::press(at)))]);
    }

    #[test]
    fn a_click_on_the_statusline_is_not_the_pages_to_see() {
        let mut session = ready();
        let effects = session.on(mouse(MouseEventKind::Down(MouseButton::Left), 4, 23));
        assert_eq!(effects, vec![], "row 23 is ours");
    }

    #[test]
    fn the_wheel_scrolls_three_rows_a_notch() {
        let mut session = ready();
        let effects = session.on(mouse(MouseEventKind::ScrollDown, 0, 1));
        let at = session.viewport().to_css(CellPos { col: 0, row: 1 });
        assert_eq!(effects, vec![Effect::Send(tab0(), Input::Mouse(MouseInput::wheel(at, 60.0)))]);
    }

    #[test]
    fn a_mouse_move_is_not_worth_a_round_trip() {
        let mut session = ready();
        assert_eq!(session.on(mouse(MouseEventKind::Moved, 4, 2)), vec![]);
    }

    // Resize.

    #[test]
    fn a_resize_tells_every_tab_before_reading_any_of_them() {
        let mut session = ready();
        open_two_more(&mut session);
        let grid = GridSize { cols: 100, rows: 30 };
        let vp = page_viewport(grid, CELL);

        let effects = session.on(Event::Resized(grid, CELL));
        assert_eq!(
            effects,
            vec![
                Effect::SetViewport(tab0(), vp),
                Effect::SetViewport(TabId(1), vp),
                Effect::SetViewport(TabId(2), vp),
            ],
            "a tab you switch to must already be the size of the terminal you have"
        );

        assert_eq!(
            session.on(Event::Done(Job::Resized(TabId(2)))),
            vec![Effect::Extract(TabId(2), Source::Script)],
            "reading before the page has reflowed reads the old layout"
        );
        assert_eq!(
            session.on(Event::Done(Job::Resized(tab0()))),
            vec![],
            "a background tab keeps the flag until you look at it"
        );
    }

    #[test]
    fn a_resize_to_the_same_size_costs_nothing() {
        let mut session = ready();
        assert_eq!(session.on(Event::Resized(GRID, CELL)), vec![]);
    }

    // Composition.

    #[test]
    fn the_caret_shows_in_insert_mode_only() {
        let mut with_caret = extraction("https://example.com");
        with_caret.caret = Some(Caret { x: 90.0, baseline: 56.0, offset: 2 });

        let mut session = session();
        session.begin();
        session.on(Event::Done(Job::Extracted(tab0(), Source::Script, Ok(Box::new(with_caret)))));
        assert_eq!(
            session.compose().cursor(),
            None,
            "a page can focus a field unasked; a caret would promise typing lands there"
        );

        session.on(key('i'));
        assert_eq!(session.compose().cursor(), Some(CellPos { col: 12, row: 3 }));
    }

    #[test]
    fn the_chrome_owns_a_row_at_each_end_and_the_page_knows_of_neither() {
        let session = ready();
        assert_eq!(session.viewport().grid().rows, 22);
        assert_eq!(session.viewport().origin_row(), 1);
        assert_eq!(session.compose().grid().rows, 24);
    }

    #[test]
    fn the_page_viewport_is_two_rows_shorter_than_the_terminal() {
        let vp = page_viewport(GRID, CELL);
        assert_eq!(vp.grid(), GridSize { cols: 80, rows: 22 });
        assert_eq!(vp.css_height(), 22 * 20);
    }

    #[test]
    fn a_one_row_terminal_still_leaves_a_page_row() {
        let vp = page_viewport(GridSize { cols: 80, rows: 1 }, CELL);
        assert_eq!(vp.grid().rows, 1, "never zero, or Chromium gets a zero-height window");
    }

    #[test]
    fn a_click_on_the_page_keeps_its_cell() {
        let vp = page_viewport(GRID, CELL);
        assert_eq!(page_cell(&vp, 5, 7), Some(CellPos { col: 5, row: 7 }));
    }

    #[test]
    fn a_click_on_the_tab_bar_belongs_to_no_page_cell() {
        // Row 0 is the tab bar and row 23 is the statusline. The page does
        // not know either exists, so there is nothing to convert a click
        // there into.
        let vp = page_viewport(GRID, CELL);
        assert_eq!(page_cell(&vp, 5, 0), None);
        assert_eq!(page_cell(&vp, 5, 23), None);
    }

    #[test]
    fn every_effect_says_which_page_it_is_for() {
        let mut session = session();
        assert_eq!(session.begin(), vec![Effect::Extract(tab0(), Source::Script)]);

        let mut session = ready();
        assert_eq!(
            session.on(key('j')),
            vec![Effect::Scroll(tab0(), Scroll::By(20.0))]
        );
    }

    #[test]
    fn a_job_for_a_tab_that_is_gone_is_dropped_rather_than_painted() {
        // Nothing can close a tab yet, but the guard is what makes Task 8
        // safe, and a job carrying an unknown id must never be looked up.
        let mut session = ready();
        let stale = Job::Extracted(TabId(999), Source::Script, Ok(Box::new(extraction("https://elsewhere.test"))));
        assert_eq!(session.on(Event::Done(stale)), vec![]);
        assert_eq!(session.focused().url, "https://example.com", "the frame is untouched");
    }

    /// Two more tabs, both opened and settled, focus left on the last.
    fn open_two_more(session: &mut Session) {
        for (n, url) in [(1u32, "one.test"), (2, "two.test")] {
            typed(session, &format!(":tabopen {url}"));
            session.on(code(KeyCode::Enter));
            session.on(Event::Done(Job::Opened(TabId(n), Ok(()))));
            session.on(Event::Done(Job::Extracted(TabId(n), Source::Script, Ok(Box::new(extraction(&format!("https://{url}")))))));
        }
    }

    // Restore.

    fn snapshot_of(urls: &[&str], focus: usize) -> Snapshot {
        Snapshot {
            version: crate::store::VERSION,
            focus,
            tabs: urls
                .iter()
                .map(|url| SavedTab {
                    url: (*url).to_string(),
                    title: "saved".to_string(),
                    scroll_y: 120.0,
                })
                .collect(),
        }
    }

    #[test]
    fn restoring_brings_back_every_tab_and_a_page_for_one_of_them() {
        let mut session = Session::restore(
            GRID,
            CELL,
            Some(snapshot_of(&["https://one.test", "https://two.test"], 1)),
            None,
        );

        assert_eq!(session.tabs().len(), 2);
        assert_eq!(session.focused().url, "https://two.test", "you come back where you were");
        assert_eq!(
            session.begin(),
            vec![Effect::OpenTab {
                id: TabId(1),
                url: "https://two.test".to_string(),
                scroll_y: 120.0
            }],
            "the tab you were looking at, and no page for the ones you were not"
        );
        assert_eq!(session.tabs[0].presence, Presence::Detached);
    }

    #[test]
    fn a_url_on_the_command_line_is_a_new_tab_beside_the_restored_ones() {
        // Nothing you had is lost by typing `wwt example.com` out of habit,
        // which is the failure mode that actually costs something.
        let session = Session::restore(
            GRID,
            CELL,
            Some(snapshot_of(&["https://one.test"], 0)),
            Some("https://asked.test".to_string()),
        );

        assert_eq!(session.tabs().len(), 2);
        assert_eq!(session.focused().url, "https://asked.test");
    }

    #[test]
    fn no_snapshot_and_no_url_is_one_blank_tab() {
        let session = Session::restore(GRID, CELL, None, None);

        assert_eq!(session.tabs().len(), 1);
        assert_eq!(session.focused().url, "about:blank");
    }

    #[test]
    fn a_snapshot_with_no_tabs_in_it_still_leaves_you_a_browser() {
        let empty = Snapshot { version: crate::store::VERSION, focus: 0, tabs: Vec::new() };

        let session = Session::restore(GRID, CELL, Some(empty), None);

        assert_eq!(session.tabs().len(), 1, "a browser with no page in it is not a state");
    }

    #[test]
    fn a_focus_index_past_the_end_of_the_snapshot_lands_on_a_real_tab() {
        // The file is data from disk and is not trusted.
        let session = Session::restore(
            GRID,
            CELL,
            Some(snapshot_of(&["https://one.test", "https://two.test"], 99)),
            None,
        );

        assert_eq!(session.focused().url, "https://two.test");
    }

    #[test]
    fn a_restored_tab_is_asked_for_at_the_offset_it_was_left_at() {
        // Asked for as part of opening, not scrolled to afterwards: the two
        // as separate effects are two spawned tasks, and an extraction that
        // wins that race reads offset zero and writes it down.
        let mut session =
            Session::restore(GRID, CELL, Some(snapshot_of(&["https://one.test"], 0)), None);

        let effects = session.begin();

        assert_eq!(
            effects,
            vec![Effect::OpenTab {
                id: tab0(),
                url: "https://one.test".to_string(),
                scroll_y: 120.0
            }]
        );
    }

    #[test]
    fn a_restored_tab_is_opened_and_read_when_you_reach_it_and_not_before() {
        // The bar has real titles from the file, so nothing is read to fill
        // it in. M4 read every restored tab once for that; the file has
        // carried the titles since, and a page is what costs something.
        let mut session = Session::restore(
            GRID,
            CELL,
            Some(snapshot_of(&["https://one.test", "https://two.test"], 1)),
            None,
        );
        session.begin();
        assert_eq!(session.tabs[0].title, "saved", "the bar reads as the tabs you left");

        // Nothing is asked of tab 0 until you look at it.
        assert_eq!(session.on(Event::Dirty(tab0())), vec![]);

        let effects = session.on(alt('1'));
        assert!(effects.contains(&Effect::OpenTab {
            id: tab0(),
            url: "https://one.test".to_string(),
            scroll_y: 120.0
        }));

        let effects = session.on(Event::Done(Job::Opened(tab0(), Ok(()))));
        assert!(effects.contains(&Effect::Extract(tab0(), Source::Script)));

        session.on(Event::Done(Job::Extracted(tab0(), Source::Script, Ok(Box::new(extraction("https://one.test"))))));
        assert!(session.tabs[0].read, "reaching a restored tab is what reads it");
    }

    /// One tab as the session file holds it.
    ///
    /// `snapshot_of` gives every tab the same title and offset, which is
    /// enough for most of these and not for the ones about what a restored
    /// tab remembers.
    fn saved_tab(url: &str, title: &str, scroll_y: f64) -> SavedTab {
        SavedTab { url: url.to_string(), title: title.to_string(), scroll_y }
    }

    #[test]
    fn a_restored_session_opens_the_tab_you_were_looking_at_and_no_others() {
        let snapshot = Snapshot {
            version: crate::store::VERSION,
            focus: 1,
            tabs: vec![
                saved_tab("https://a.example", "A", 0.0),
                saved_tab("https://b.example", "B", 250.0),
                saved_tab("https://c.example", "C", 0.0),
            ],
        };
        let mut session = Session::restore(GRID, CELL, Some(snapshot), None);
        let effects = session.begin();

        let opens: Vec<_> = effects
            .iter()
            .filter_map(|e| match e {
                Effect::OpenTab { id, url, scroll_y } => Some((*id, url.clone(), *scroll_y)),
                _ => None,
            })
            .collect();
        assert_eq!(opens.len(), 1, "one page, however many tabs were open");
        assert_eq!(opens[0].1, "https://b.example");
        assert_eq!(opens[0].2, 250.0);
        assert_eq!(session.tabs[0].presence, Presence::Detached);
        assert_eq!(session.tabs[2].presence, Presence::Detached);
    }

    #[test]
    fn a_restored_tab_reads_as_itself_in_the_bar_before_it_has_a_page() {
        // Titles come from the file, so the bar is complete on the first
        // frame rather than a row of blanks that fills in.
        let snapshot = Snapshot {
            version: crate::store::VERSION,
            focus: 0,
            tabs: vec![saved_tab("https://a.example", "Anemone", 0.0)],
        };
        let session = Session::restore(GRID, CELL, Some(snapshot), None);
        assert_eq!(session.tabs[0].title, "Anemone");
    }

    #[test]
    fn a_url_on_the_command_line_is_the_tab_that_opens() {
        let snapshot = Snapshot {
            version: crate::store::VERSION,
            focus: 0,
            tabs: vec![saved_tab("https://a.example", "A", 0.0)],
        };
        let mut session = Session::restore(
            GRID,
            CELL,
            Some(snapshot),
            Some("https://new.example".to_string()),
        );
        let effects = session.begin();
        let opens: Vec<_> = effects
            .iter()
            .filter(|e| matches!(e, Effect::OpenTab { .. }))
            .collect();
        assert_eq!(opens.len(), 1);
        assert!(matches!(
            opens[0],
            Effect::OpenTab { url, .. } if url == "https://new.example"
        ));
    }

    // The session file.

    fn saved(effects: &[Effect]) -> Option<&Snapshot> {
        effects.iter().find_map(|effect| match effect {
            Effect::Save(snapshot) => Some(snapshot),
            _ => None,
        })
    }

    #[test]
    fn a_snapshot_is_the_tabs_you_have_and_the_one_you_are_looking_at() {
        let mut session = ready();
        open_two_more(&mut session);

        let snapshot = session.snapshot();

        assert_eq!(snapshot.version, crate::store::VERSION);
        assert_eq!(snapshot.focus, 2);
        assert_eq!(snapshot.tabs.len(), 3);
        assert_eq!(snapshot.tabs[0].url, "https://example.com");
        assert_eq!(snapshot.tabs[0].title, "Example");
    }

    #[test]
    fn opening_a_tab_is_worth_writing_down() {
        let mut session = ready();
        typed(&mut session, ":tabopen example.org");

        let effects = session.on(code(KeyCode::Enter));

        assert!(saved(&effects).is_some(), "the tab set changed");
    }

    #[test]
    fn closing_a_tab_is_worth_writing_down() {
        let mut session = ready();
        open_two_more(&mut session);

        let effects = session.on(key('x'));

        assert_eq!(saved(&effects).map(|s| s.tabs.len()), Some(2));
    }

    #[test]
    fn switching_tabs_is_worth_writing_down() {
        let mut session = ready();
        open_two_more(&mut session);

        let effects = session.on(alt('1'));

        assert_eq!(saved(&effects).map(|s| s.focus), Some(0));
    }

    #[test]
    fn an_extraction_that_moved_the_page_is_worth_writing_down() {
        let mut session = ready();
        let mut moved = extraction("https://example.com");
        moved.status.scroll_y = 240.0;

        let effects = session.on(Event::Done(Job::Extracted(tab0(), Source::Script, Ok(Box::new(moved)))));

        assert_eq!(saved(&effects).map(|s| s.tabs[0].scroll_y), Some(240.0));
    }

    #[test]
    fn an_extraction_that_changed_nothing_is_not_worth_a_write() {
        let mut session = ready();
        session.focused_mut().dirty = true;

        let effects = session.on(Event::Done(Job::Extracted(tab0(), Source::Script, Ok(Box::new(extraction("https://example.com"))))));

        assert!(
            saved(&effects).is_none(),
            "an idle page must not turn into a write per extraction"
        );
    }

    #[test]
    fn an_error_page_extracted_again_is_not_worth_a_write() {
        // Its URL is deliberately not the one the tab keeps, so a save
        // decided on the extraction rather than on what was stored would
        // write on every dirty signal a page that cannot even load.
        let mut session = ready();
        let error = || {
            Event::Done(Job::Extracted(tab0(), Source::Script, Ok(Box::new(extraction("chrome-error://chromewebdata/")))))
        };
        session.on(error());

        let effects = session.on(error());

        assert!(saved(&effects).is_none(), "nothing a restart would notice moved");
    }

    // Opening and closing.

    #[test]
    fn opening_a_tab_asks_for_a_page_and_moves_you_to_it() {
        let mut session = ready();
        let effects = session.on(key('t'));
        assert!(matches!(session.mode(), Mode::Command(buffer) if buffer == "tabopen "));
        assert_eq!(effects, vec![], "`t` only opens the : line");

        typed(&mut session, "example.org");
        let effects = session.on(code(KeyCode::Enter));

        assert_eq!(session.tabs().len(), 2);
        assert_eq!(session.focused().id, TabId(1), "a new tab is the one you are looking at");
        assert_eq!(
            effects,
            vec![
                Effect::OpenTab {
                    id: TabId(1),
                    url: "https://example.org".to_string(),
                    scroll_y: 0.0
                },
                Effect::Save(session.snapshot()),
            ]
        );
    }

    #[test]
    fn a_tab_that_finished_opening_is_activated_and_read() {
        let mut session = ready();
        typed(&mut session, ":tabopen example.org");
        session.on(code(KeyCode::Enter));

        let effects = session.on(Event::Done(Job::Opened(TabId(1), Ok(()))));
        assert_eq!(
            effects,
            vec![Effect::Activate(TabId(1)), Effect::Extract(TabId(1), Source::Script)]
        );
    }

    #[test]
    fn a_tab_the_page_opened_for_itself_arrives_focused_and_asks_to_be_adopted() {
        let mut session = ready();
        let target = Attached {
            target: wwt_cdp::TargetId("T2".to_string()),
            session: "S2".to_string(),
        };

        let effects = session.on(Event::TargetOpened(target.clone()));

        assert_eq!(session.tabs().len(), 2);
        assert_eq!(session.focused().id, TabId(1), "a link that opens a tab takes you to it");
        assert_eq!(effects, vec![Effect::AdoptTab { id: TabId(1), target }]);
    }

    #[test]
    fn an_adopted_tab_is_activated_and_read_like_any_other() {
        // Adoption differs from opening only in where the target came from,
        // so everything after `Job::Opened` has to be the one path.
        let mut session = ready();
        session.on(Event::TargetOpened(Attached {
            target: wwt_cdp::TargetId("T2".to_string()),
            session: "S2".to_string(),
        }));

        let effects = session.on(Event::Done(Job::Opened(TabId(1), Ok(()))));

        assert_eq!(
            effects,
            vec![Effect::Activate(TabId(1)), Effect::Extract(TabId(1), Source::Script)]
        );
    }

    #[test]
    fn a_tab_that_could_not_be_opened_leaves_you_where_you_were() {
        let mut session = ready();
        typed(&mut session, ":tabopen example.org");
        session.on(code(KeyCode::Enter));

        session.on(Event::Done(Job::Opened(TabId(1), Err("no target".to_string()))));
        assert_eq!(session.tabs().len(), 1, "a tab with no page is not a tab");
        assert_eq!(session.focused().id, tab0());
        assert!(matches!(session.state(), State::Error(_)));
    }

    #[test]
    fn a_failure_on_a_background_tab_does_not_land_on_the_one_in_front() {
        let mut session = ready();
        typed(&mut session, ":tabopen example.org");
        session.on(code(KeyCode::Enter));
        session.on(Event::Done(Job::Opened(TabId(1), Ok(()))));
        session.on(alt('1'));
        assert_eq!(session.focused().id, tab0(), "back on the first tab");

        // A target that would not come to the front, reported while you are
        // looking at something else. It is the second tab's failure and it
        // says so there.
        session.on(Event::Done(Job::Noted(
            TabId(1),
            "would not activate".to_string(),
        )));

        assert!(
            !matches!(session.state(), State::Error(_)),
            "the tab in front did not fail: {:?}",
            session.state()
        );
        let background = session
            .tabs()
            .iter()
            .find(|tab| tab.id == TabId(1))
            .expect("the tab is still open");
        assert!(
            matches!(background.state, State::Error(_)),
            "the tab that failed says nothing about it: {:?}",
            background.state
        );
    }

    #[test]
    fn a_tab_with_no_page_yet_is_not_asked_for_hints() {
        let mut session = ready();
        typed(&mut session, ":tabopen example.org");
        session.on(code(KeyCode::Enter));
        // Focus is on the new tab, which has been asked for and not yet
        // opened. `Core` holds no page for it and would drop the query.
        let effects = session.on(key('f'));
        assert!(
            !effects.iter().any(|effect| matches!(effect, Effect::Hints(..))),
            "asked a tab with no page behind it: {effects:?}"
        );

        // And the asking is still there to be done once there is a page.
        // `Job::Hints` is the only thing that clears the in-flight flag, so
        // a query nobody could answer would have left `f` dead on this tab
        // for the rest of the run.
        session.on(Event::Done(Job::Opened(TabId(1), Ok(()))));
        let effects = session.on(key('f'));
        assert!(
            effects.contains(&Effect::Hints(TabId(1), Source::Script)),
            "f stopped working on the tab: {effects:?}"
        );
    }

    #[test]
    fn the_only_tab_failing_to_open_asks_to_quit() {
        // A browser with no page in it is not a state worth having, and one
        // page that will not open is that state. The tab it would have been
        // is gone, so there is nowhere left to say so and nothing to do but
        // leave, which is the same rule closing the last tab follows.
        let mut session = session();
        session.begin();
        let effects = session.on(Event::Done(Job::Opened(tab0(), Err("no target".to_string()))));

        assert!(session.tabs().is_empty(), "the tab had no page behind it");
        assert!(
            effects.contains(&Effect::Quit),
            "closing the last tab asks to quit, and asking is the whole \
             point of handing closing the caller's effects: {effects:?}"
        );
    }

    #[test]
    fn a_tab_that_could_not_be_opened_hands_its_neighbour_the_browser() {
        // Closing decides more than that a tab is gone: the tab taking its
        // place has to be brought to the front, because input dispatch is
        // answered by whichever target the browser has in front.
        let mut session = ready();
        typed(&mut session, ":tabopen example.org");
        session.on(code(KeyCode::Enter));

        let effects = session.on(Event::Done(Job::Opened(TabId(1), Err("no target".to_string()))));
        assert!(
            effects.contains(&Effect::Activate(tab0())),
            "the tab you are left looking at is the one the browser must \
             have in front: {effects:?}"
        );
    }

    #[test]
    fn closing_the_focused_tab_lands_you_on_its_right_hand_neighbour() {
        let mut session = ready();
        open_two_more(&mut session);
        // Three tabs, focused on the middle one.
        session.on(alt('2'));
        assert_eq!(session.focused().id, TabId(1));

        let effects = session.on(key('x'));
        assert!(effects.contains(&Effect::CloseTab(TabId(1))));
        assert_eq!(session.tabs().len(), 2);
        assert_eq!(session.focused().id, TabId(2), "the right-hand neighbour took its place");
    }

    #[test]
    fn closing_a_tab_to_your_left_leaves_you_looking_at_the_same_page() {
        let mut session = ready();
        open_two_more(&mut session);
        assert_eq!(session.focused().id, TabId(2));

        session.close_tab(tab0(), &mut Vec::new());
        assert_eq!(session.focused().id, TabId(2), "you did not move");
    }

    #[test]
    fn closing_the_last_tab_quits() {
        let mut session = ready();
        let effects = session.on(key('x'));
        assert!(effects.contains(&Effect::CloseTab(tab0())));
        assert!(effects.contains(&Effect::Quit), "a browser with no page in it is not a state");
    }

    #[test]
    fn a_closed_tabs_late_answer_lands_nowhere() {
        let mut session = ready();
        open_two_more(&mut session);
        session.on(key('x')); // closes TabId(2), leaving 0 and 1

        let late = Job::Extracted(TabId(2), Source::Script, Ok(Box::new(extraction("https://gone.test"))));
        assert_eq!(session.on(Event::Done(late)), vec![]);
        assert_eq!(session.tabs().len(), 2);
    }

    fn failed(id: TabId) -> Job {
        Job::Extracted(
            id,
            Source::Script,
            Err(Failure::Failed("__wwt is not defined".to_string())),
        )
    }

    #[test]
    fn a_read_that_timed_out_stalls_the_tab_and_does_not_degrade_it() {
        // A script that threw is a page our extractor cannot read, and the
        // snapshot is a different extractor that might. A page that did not
        // answer in five seconds has no main thread running, and the
        // snapshot needs the same one: asking would cost a second deadline
        // to learn the same thing, and would mark the tab degraded for the
        // rest of its life over a wedge that may last a second.
        let mut session = ready();
        let id = session.focused_id();
        let effects = session.on(Event::Done(Job::Extracted(
            id,
            Source::Script,
            Err(Failure::TimedOut),
        )));

        assert_eq!(*session.state(), State::Stalled);
        assert!(!session.focused().degraded, "a deadline is not a broken script");
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::Extract(_, Source::Snapshot))),
            "there is nothing to ask a page that is not running"
        );
        assert!(!session.focused().reading, "the read is over either way");
    }

    #[test]
    fn a_script_that_threw_still_reaches_for_the_snapshot() {
        // M6's rule, unchanged. This is what proves the exemption above is
        // an exemption and not a replacement.
        let mut session = ready();
        let id = session.focused_id();
        let effects = session.on(Event::Done(failed(id)));

        assert!(session.focused().degraded);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Extract(_, Source::Snapshot)))
        );
    }

    #[test]
    fn a_page_that_comes_back_clears_the_stall_by_itself() {
        // Nothing schedules a retry: a page wedged in a loop cannot run its
        // own MutationObserver, so it sends no dirty signal and nothing
        // re-asks. A page that recovers sends one and is read normally.
        let mut session = ready();
        let id = session.focused_id();
        session.on(Event::Done(Job::Extracted(id, Source::Script, Err(Failure::TimedOut))));
        assert_eq!(*session.state(), State::Stalled);

        let effects = session.on(Event::Dirty(id));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Extract(_, Source::Script))),
            "the fast path, because a timeout never degraded it"
        );
        session.on(read(id, Source::Script, "https://example.com"));
        assert_eq!(*session.state(), State::Ready);
    }

    fn read(id: TabId, source: Source, url: &str) -> Event {
        Event::Done(Job::Extracted(id, source, Ok(Box::new(extraction(url)))))
    }

    // What a dirty signal costs while a picture is what you are looking at.

    fn attached(target: &str) -> Attached {
        Attached { target: wwt_cdp::TargetId(target.to_string()), session: format!("s-{target}") }
    }

    /// A ready session in pixel mode, which is where the cheap read applies.
    fn ready_in_pixel() -> Session {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        session
    }

    #[test]
    fn opening_a_tab_in_pixel_mode_moves_the_picture_to_it() {
        // The tab you left stops at once, because it exists. The new one
        // cannot start until it does: `Core` drops any effect naming a page
        // it holds none for, which is every effect between asking for a tab
        // and being told it opened.
        let mut session = ready_in_pixel();
        typed(&mut session, ":tabopen one.test");

        let effects = session.on(code(KeyCode::Enter));
        assert!(
            effects.contains(&Effect::StopScreencast(tab0())),
            "the tab you left has to stop, or its frames go on arriving: {effects:?}"
        );
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::StartScreencast(..))),
            "and nothing can start on a tab with no page yet: {effects:?}"
        );

        let effects = session.on(Event::Done(Job::Opened(TabId(1), Ok(()))));
        assert!(
            effects.iter().any(|e| matches!(e, Effect::StartScreencast(id, _) if *id == TabId(1))),
            "the picture follows the focus once there is a page to ask: {effects:?}"
        );
    }

    #[test]
    fn adopting_a_tab_in_pixel_mode_moves_the_picture_to_it() {
        // A `target=_blank` link arrives focused like any other new tab, and
        // has the same window with no page behind it.
        let mut session = ready_in_pixel();

        let effects = session.on(Event::TargetOpened(attached("t-1")));
        assert!(
            effects.contains(&Effect::StopScreencast(tab0())),
            "the tab you left has to stop: {effects:?}"
        );

        let effects = session.on(Event::Done(Job::Opened(TabId(1), Ok(()))));
        assert!(
            effects.iter().any(|e| matches!(e, Effect::StartScreencast(id, _) if *id == TabId(1))),
            "the picture follows the focus: {effects:?}"
        );
    }

    #[test]
    fn opening_a_tab_in_text_mode_asks_for_no_screencast() {
        let mut session = ready();
        typed(&mut session, ":tabopen one.test");
        session.on(code(KeyCode::Enter));

        let effects = session.on(Event::Done(Job::Opened(TabId(1), Ok(()))));

        assert!(
            !effects.iter().any(|e| matches!(
                e,
                Effect::StartScreencast(..) | Effect::StopScreencast(..)
            )),
            "text mode has no picture to move: {effects:?}"
        );
    }

    #[test]
    fn a_dirty_signal_in_pixel_mode_asks_only_what_the_chrome_needs() {
        // The runs an extraction would return are not painted in pixel mode:
        // `compose` paints the picture instead. Asking for them is a forced
        // layout on the same main thread that has to paint the next frame,
        // for an answer that is thrown away.
        let mut session = ready_in_pixel();

        assert_eq!(session.on(Event::Dirty(tab0())), vec![Effect::ReadStatus(tab0())]);
    }

    #[test]
    fn a_dirty_signal_in_text_mode_still_asks_for_the_runs() {
        let mut session = ready();

        assert_eq!(
            session.on(Event::Dirty(tab0())),
            vec![Effect::Extract(tab0(), Source::Script)],
            "text mode paints the runs, so text mode has to have them"
        );
    }

    #[test]
    fn a_status_read_moves_the_statusline_and_is_worth_writing_down() {
        // Everything a scroll in pixel mode has to keep true: where the
        // statusline says you are, and where a restart would put you back.
        let mut session = ready_in_pixel();
        session.on(Event::Dirty(tab0()));
        let mut moved = status("https://example.com");
        moved.scroll_y = 240.0;

        let effects = session.on(Event::Done(Job::Status(tab0(), Ok(moved))));

        assert_eq!(session.focused().scroll_y, 240.0);
        assert!(session.focused().progress > 0.0, "the statusline has to move with it");
        assert_eq!(saved(&effects).map(|s| s.tabs[0].scroll_y), Some(240.0));
    }

    #[test]
    fn a_second_dirty_signal_does_not_stack_a_second_status_read() {
        let mut session = ready_in_pixel();
        session.on(Event::Dirty(tab0()));

        assert_eq!(
            session.on(Event::Dirty(tab0())),
            vec![],
            "one read in flight at a time, whichever kind it is"
        );
    }

    #[test]
    fn leaving_pixel_mode_asks_for_the_runs_again() {
        // The runs in hand are whatever they were when pixel mode was
        // entered, and the page has been free to change since. Without this
        // the first frame of text is stale, and a tab opened while the
        // picture was up has no runs at all.
        let mut session = ready_in_pixel();
        session.on(Event::Dirty(tab0()));
        session.on(Event::Done(Job::Status(tab0(), Ok(status("https://example.com")))));

        let effects = session.on(key('p'));

        assert!(
            effects.contains(&Effect::Extract(tab0(), Source::Script)),
            "leaving pixel mode has to read the runs back: {effects:?}"
        );
    }

    #[test]
    fn a_degraded_tab_in_pixel_mode_still_asks_the_snapshot() {
        // `status()` is our script, and a degraded tab is one whose script
        // throws. Asking it the cheap question asks the broken thing.
        let mut session = ready_with_graphics();
        session.on(Event::Done(failed(tab0())));
        session.on(read(tab0(), Source::Snapshot, "https://example.com"));
        session.on(key('p'));

        assert_eq!(
            session.on(Event::Dirty(tab0())),
            vec![Effect::Extract(tab0(), Source::Snapshot)]
        );
    }

    #[test]
    fn a_tab_nobody_has_read_yet_is_read_for_its_runs_even_in_pixel_mode() {
        // Reading a tab once when it opens is what puts a real title in the
        // bar and makes the first switch to it a repaint. A status carries
        // no runs, so it cannot be what does that.
        let mut session = ready_in_pixel();
        typed(&mut session, ":tabopen one.test");
        session.on(code(KeyCode::Enter));

        let effects = session.on(Event::Done(Job::Opened(TabId(1), Ok(()))));

        assert!(
            effects.contains(&Effect::Extract(TabId(1), Source::Script)),
            "a tab with no runs needs the read that produces them: {effects:?}"
        );
    }

    #[test]
    fn leaving_pixel_mode_leaves_every_tab_wanting_a_read() {
        // A switch spends a dirty flag and never sets one, so a tab visited
        // in pixel mode would paint the runs it had before the picture went
        // up, for as long as nothing else changed it.
        let mut session = ready_in_pixel();
        open_two_more(&mut session);

        session.on(key('p'));

        assert!(
            session.tabs.iter().all(|tab| tab.dirty || tab.reading),
            "every tab has to be re-read, now or when you reach it"
        );
    }

    #[test]
    fn a_status_read_that_throws_degrades_the_tab_like_an_extraction_does() {
        let mut session = ready_in_pixel();
        session.on(Event::Dirty(tab0()));

        let effects = session.on(Event::Done(Job::Status(tab0(), Err(Failure::Failed("no".to_string())))));

        assert_eq!(
            effects,
            vec![Effect::Extract(tab0(), Source::Snapshot)],
            "the same rule as a failed script extraction, and the same one retry"
        );
        assert!(session.focused().degraded);
    }

    #[test]
    fn a_script_that_throws_is_read_the_other_way_instead() {
        let mut session = session();
        assert_eq!(session.begin(), vec![Effect::Extract(tab0(), Source::Script)]);

        let effects = session.on(Event::Done(failed(tab0())));
        assert_eq!(
            effects,
            vec![Effect::Extract(tab0(), Source::Snapshot)],
            "a failed script extraction asks the other source, once"
        );
    }

    #[test]
    fn a_tab_that_has_degraded_asks_the_snapshot_first_from_then_on() {
        // Otherwise a page whose script is permanently broken pays a failed
        // round trip before every good one, on every scroll frame.
        let mut session = session();
        session.begin();
        session.on(Event::Done(failed(tab0())));
        session.on(read(tab0(), Source::Snapshot, "https://example.com"));

        assert_eq!(
            session.on(Event::Dirty(tab0())),
            vec![Effect::Extract(tab0(), Source::Snapshot)]
        );
    }

    #[test]
    fn a_snapshot_that_also_fails_is_the_end_of_the_line() {
        let mut session = session();
        session.begin();
        session.on(Event::Done(failed(tab0())));

        let effects = session.on(Event::Done(Job::Extracted(
            tab0(),
            Source::Snapshot,
            Err(Failure::Failed("no document".to_string())),
        )));

        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Extract(..))),
            "there is no third source: {effects:?}"
        );
        assert!(matches!(session.state(), State::Error(_)), "the statusline must say so");
    }

    #[test]
    fn a_failed_extraction_leaves_the_frame_you_are_looking_at_alone() {
        let mut session = session();
        session.begin();
        session.on(read(tab0(), Source::Script, "https://example.com"));
        let before = session.compose().row_text(1);

        session.on(Event::Done(failed(tab0())));
        session.on(Event::Done(Job::Extracted(
            tab0(),
            Source::Snapshot,
            Err(Failure::Failed("no document".to_string())),
        )));

        assert_eq!(session.compose().row_text(1), before, "spec section 8");
    }

    #[test]
    fn navigating_gives_a_degraded_tab_the_good_path_back() {
        // A new document reinstalls bootstrap.js, so the next page has done
        // nothing to deserve the slow path. It is also the way back:
        // reloading a tab that degraded on a transient failure clears it.
        let mut session = session();
        session.begin();
        session.on(Event::Done(failed(tab0())));
        session.on(read(tab0(), Source::Snapshot, "https://example.com"));

        // Reload is Ctrl-r here, not r: `keymap.rs` is the table to check
        // rather than to guess at.
        session.on(ctrl('r'));

        // Settling is what asks for the read, so it is what to assert on:
        // a `Dirty` after it would find an extraction already in flight and
        // correctly do nothing.
        assert_eq!(
            session.on(Event::Done(Job::Settled(tab0()))),
            vec![Effect::Extract(tab0(), Source::Script)]
        );
    }

    #[test]
    fn hints_follow_the_flag_rather_than_deciding_anything() {
        let mut healthy = session();
        healthy.begin();
        healthy.on(read(tab0(), Source::Script, "https://example.com"));
        assert!(healthy.on(key('f')).contains(&Effect::Hints(tab0(), Source::Script)));

        let mut degraded = session();
        degraded.begin();
        degraded.on(Event::Done(failed(tab0())));
        degraded.on(read(tab0(), Source::Snapshot, "https://example.com"));
        assert!(degraded.on(key('f')).contains(&Effect::Hints(tab0(), Source::Snapshot)));
    }

    #[test]
    fn a_degraded_tab_says_so_and_goes_on_saying_it() {
        // Not a State::Notice: a notice is cleared by the next successful
        // extraction, and on a degraded tab the next extraction succeeds
        // every time, so it would say this once and never again.
        let mut session = session();
        session.begin();
        session.on(Event::Done(failed(tab0())));
        session.on(read(tab0(), Source::Snapshot, "https://example.com"));

        let rows = session.compose().grid().rows;
        let status = session.compose().row_text(rows - 1);
        assert!(status.contains("[degraded]"), "statusline was {status:?}");

        session.on(read(tab0(), Source::Snapshot, "https://example.com"));
        let status = session.compose().row_text(rows - 1);
        assert!(status.contains("[degraded]"), "and still says it: {status:?}");
    }

    #[test]
    fn the_tag_belongs_to_the_tab_and_not_to_the_browser() {
        let mut session = session();
        session.begin();
        session.on(Event::Done(failed(tab0())));
        // `t` opens the command line with "tabopen " prefilled rather than
        // opening a tab, so this drives it the way the tab tests do.
        typed(&mut session, ":tabopen https://other.test");
        session.on(Event::Done(Job::Opened(TabId(1), Ok(()))));

        let rows = session.compose().grid().rows;
        let status = session.compose().row_text(rows - 1);
        assert!(!status.contains("[degraded]"), "the new tab is fine: {status:?}");
    }

    fn hinted_for(id: TabId, targets: Vec<HintTarget>) -> Event {
        Event::Done(Job::Hints(id, Ok(targets)))
    }

    fn run(text: &str) -> TextRun {
        TextRun {
            text: text.to_string(),
            rect: CssRect { x: 0.0, y: 0.0, w: 400.0, h: 20.0 },
            baseline: 16.0,
            style: Style { fg: Rgb { r: 0xd0, g: 0xd0, b: 0xd0 }, bg: None, bold: false, reverse: false },
            z: 0,
        }
    }

    // Pixel mode.

    /// A ready session on a terminal that can show pictures.
    fn ready_with_graphics() -> Session {
        let mut session = ready();
        session.set_graphics(true);
        session
    }

    fn frame_data(payload: &str) -> Event {
        frame_for(tab0(), payload)
    }

    fn frame_for(id: TabId, payload: &str) -> Event {
        Event::Frame(
            id,
            Box::new(ScreencastFrame {
                data: payload.to_string(),
                ack: 7,
            }),
        )
    }

    /// A real screencast frame, as base64, from the M6 probe.
    fn fixture_frame() -> ScreencastFrame {
        ScreencastFrame {
            data: include_str!("../../wwt-png/tests/fixtures/screencast.txt").trim().to_string(),
            ack: 1,
        }
    }

    #[test]
    fn pixel_mode_without_graphics_is_offered_rather_than_refused() {
        // M5 answered this with a notice and said so until M6.
        let mut session = session();
        session.set_graphics(false);
        session.on(key('p'));

        let frame = session.compose();
        assert!(
            !matches!(session.state(), State::Notice(_)),
            "pixel mode said something instead of entering"
        );
        assert_eq!(frame.image(), None, "no graphics means no image on the frame");
    }

    #[test]
    fn a_frame_without_graphics_composes_to_half_block_cells() {
        let mut session = session();
        session.set_graphics(false);
        session.on(key('p'));
        session.on(Event::Frame(tab0(), Box::new(fixture_frame())));

        let frame = session.compose();
        // Row 1 is the first page row: row 0 is the tab bar.
        let cell = frame.cell(CellPos { col: 0, row: 1 }).expect("a page cell");
        assert_eq!(cell.ch, '\u{2580}');
        assert_eq!(cell.style.fg, Rgb { r: 255, g: 0, b: 0 }, "the fixture page was red");
        assert_eq!(cell.style.bg, Some(Rgb { r: 255, g: 0, b: 0 }));
        assert_eq!(frame.image(), None, "half-block is cells and never an image");
    }

    #[test]
    fn a_frame_with_graphics_still_composes_to_an_image() {
        // M5's path, unchanged, and the test that says so.
        let mut session = session();
        session.set_graphics(true);
        session.on(key('p'));
        session.on(Event::Frame(tab0(), Box::new(fixture_frame())));

        let frame = session.compose();
        assert!(frame.image().is_some(), "graphics means the payload goes out whole");
    }

    #[test]
    fn a_picture_that_cannot_be_decoded_leaves_the_last_one_up_and_is_still_acked() {
        let mut session = session();
        session.set_graphics(false);
        session.on(key('p'));
        session.on(Event::Frame(tab0(), Box::new(fixture_frame())));

        let effects = session.on(Event::Frame(
            tab0(),
            Box::new(ScreencastFrame { data: "not a picture".to_string(), ack: 7 }),
        ));

        assert!(
            effects.contains(&Effect::AckFrame(tab0(), 7)),
            "Chromium counts acks and not paints, so a dropped frame still owes one"
        );
        let frame = session.compose();
        let cell = frame.cell(CellPos { col: 0, row: 1 }).expect("a page cell");
        assert_eq!(cell.ch, '\u{2580}', "the picture you were looking at must stand");
    }

    #[test]
    fn leaving_pixel_mode_takes_the_half_block_picture_with_it() {
        let mut session = session();
        session.set_graphics(false);
        session.on(key('p'));
        session.on(Event::Frame(tab0(), Box::new(fixture_frame())));
        session.on(key('p'));

        let frame = session.compose();
        let cell = frame.cell(CellPos { col: 0, row: 1 }).expect("a page cell");
        assert_ne!(cell.ch, '\u{2580}', "text mode must not keep painting the picture");
    }

    #[test]
    fn a_terminal_with_graphics_is_asked_for_the_page_at_full_size() {
        let mut session = ready_with_graphics();
        let effects = session.on(key('p'));

        let vp = page_viewport(GRID, CELL);
        assert!(
            effects.contains(&Effect::StartScreencast(
                tab0(),
                FrameSize { width: vp.css_width(), height: vp.css_height() }
            )),
            "effects were {effects:?}"
        );
    }

    #[test]
    fn a_terminal_without_graphics_is_asked_for_twice_the_sample_grid() {
        // Half-block wants cols by 2*rows samples. Twice that, because
        // Chromium fits the frame inside both bounds while preserving the
        // source aspect ratio, and the sample grid's aspect is a half
        // cell, which is not square: asking for exactly the grid returns a
        // frame that is short on one axis, which is a letterboxed page.
        // Asked of the decision rather than through `p`, which still
        // refuses a terminal with no graphics protocol. Half-block is what
        // gives it something to do there, and that is the next task.
        let mut session = ready();
        session.set_graphics(false);

        let grid = page_viewport(GRID, CELL).grid();
        assert_eq!(
            session.frame_size(),
            FrameSize { width: u32::from(grid.cols) * 2, height: u32::from(grid.rows) * 4 }
        );
    }

    #[test]
    fn p_turns_pixel_mode_on_and_asks_for_pictures() {
        let mut session = ready_with_graphics();
        let effects = session.on(key('p'));
        assert!(session.pixel);
        assert!(matches!(effects.as_slice(), [Effect::StartScreencast(..)]));
    }

    #[test]
    fn p_again_turns_it_off_and_stops_them() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let effects = session.on(key('p'));
        assert!(!session.pixel);
        assert!(
            effects.contains(&Effect::StopScreencast(tab0())),
            "the pictures stop: {effects:?}"
        );
        assert!(
            effects.contains(&Effect::Extract(tab0(), Source::Script)),
            "and the runs come back, because nobody was keeping them: {effects:?}"
        );
    }

    #[test]
    fn asking_for_reader_keeps_the_picture_until_the_document_arrives() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        session.on(frame_data("AAAA"));

        let effects = session.on(key('r'));

        assert_eq!(effects, vec![Effect::ReadReader(tab0())]);
        assert!(session.compose().image().is_some());
        assert!(!effects.iter().any(|effect| matches!(effect, Effect::StopScreencast(_))));

        let effects = session.on(Event::Done(Job::Reader(
            tab0(),
            Ok(Box::new(reader_extraction("reader page"))),
        )));
        assert!(effects.contains(&Effect::StopScreencast(tab0())));
        assert!(session.focused().reader.active);
        assert!(session.compose().image().is_none());
        assert!(session.compose().row_text(1).contains("reader page"));
    }

    #[test]
    fn cached_reader_entry_stops_pixels_and_exit_starts_them_again() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        session.on(frame_data("AAAA"));
        cache_reader(&mut session, "reader page");

        let entering = session.on(key('r'));
        assert_eq!(entering, vec![Effect::StopScreencast(tab0())]);
        assert!(session.compose().image().is_none());
        assert!(session.compose().row_text(1).contains("reader page"));

        let leaving = session.on(key('r'));
        assert!(matches!(leaving.as_slice(), [Effect::StartScreencast(..)]));
        assert!(!session.focused().reader.active);
    }

    #[test]
    fn p_from_reader_starts_pixels_and_quit_stops_them() {
        let mut session = ready_with_graphics();
        cache_reader(&mut session, "reader page");
        session.on(key('r'));

        assert_eq!(
            session.on(key('p')),
            vec![Effect::StartScreencast(tab0(), session.frame_size())]
        );
        assert!(session.pixel);
        assert!(!session.focused().reader.active);
        assert_eq!(
            session.on(key('q')),
            vec![Effect::StopScreencast(tab0()), Effect::Quit]
        );
    }

    #[test]
    fn a_frame_becomes_the_image_on_the_next_compose() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        session.on(frame_data("AAAA"));

        let frame = session.compose();
        let image = frame.image().expect("pixel mode composes an image");
        assert_eq!(image.payload.as_str(), "AAAA");
        assert_eq!(image.area.row, 1, "the page starts below the tab bar");
    }

    #[test]
    fn every_frame_is_acked_so_the_next_one_comes() {
        // Chromium sends the next only once the last is acked.
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let effects = session.on(frame_data("AAAA"));
        assert!(effects.iter().any(|e| matches!(e, Effect::AckFrame(_, 7))));
    }

    #[test]
    fn a_frame_for_a_tab_that_is_not_focused_is_acked_and_dropped() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        open_two_more(&mut session);
        let background = tab0();

        let effects = session.on(frame_for(background, "BBBB"));
        assert!(effects.iter().any(|e| matches!(e, Effect::AckFrame(_, 7))));
        assert!(
            session.compose().image().is_none_or(|i| i.payload.as_str() != "BBBB"),
            "the tab you are not looking at does not paint"
        );
    }

    #[test]
    fn a_frame_is_acked_even_when_pixel_mode_has_already_been_left() {
        // Stopping is not instant: the frame in flight when p was pressed
        // still arrives. Failing to answer it leaves the screencast stopped
        // in a way that only shows up as a picture that never moves.
        let mut session = ready_with_graphics();
        session.on(key('p'));
        session.on(key('p'));

        let effects = session.on(frame_data("LATE"));
        assert!(
            effects.iter().any(|e| matches!(e, Effect::AckFrame(_, 7))),
            "a frame arriving after the mode is off is still answered"
        );
        assert!(session.compose().image().is_none(), "and is not shown");
    }

    #[test]
    fn a_frame_for_a_tab_that_is_gone_is_dropped_without_a_word() {
        // Every other job naming a closed tab is dropped, and an ack has
        // nowhere to go: Core drops an effect naming a page it does not
        // hold, so asking would be asking nobody.
        let mut session = ready_with_graphics();
        open_two_more(&mut session);
        session.on(key('p'));
        let doomed = session.focused_id();
        session.on(key('x'));

        let effects = session.on(frame_for(doomed, "GONE"));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::AckFrame(id, _) if *id == doomed)),
            "nothing is asked of a tab that is gone"
        );
    }

    #[test]
    fn each_frame_composes_a_new_generation() {
        // The renderer diffs on this. Two frames that encode identically
        // are still two frames and both must reach the terminal.
        let mut session = ready_with_graphics();
        session.on(key('p'));
        session.on(frame_data("SAME"));
        let first = session.compose().image().expect("an image").generation;
        session.on(frame_data("SAME"));
        let second = session.compose().image().expect("an image").generation;
        assert_ne!(first, second);
    }

    #[test]
    fn pixel_mode_paints_no_runs() {
        // The picture is the page. Painting runs underneath would show text
        // through every cell the image does not cover.
        let mut session = ready_with_graphics();
        session.focused_mut().runs = vec![run("hello")];
        session.on(key('p'));
        session.on(frame_data("AAAA"));

        let frame = session.compose();
        assert_eq!(
            frame.cell(CellPos { col: 0, row: 1 }).map(|c| c.ch),
            Some(' ')
        );
    }

    #[test]
    fn a_hint_label_is_painted_over_the_picture() {
        // Unicode placeholders make placement cell content, so a glyph in
        // the page area wins. This is why f keeps working in pixel mode.
        let mut session = ready_with_graphics();
        session.on(key('p'));
        session.on(frame_data("AAAA"));
        session.on(key('f'));
        session.on(hinted_for(tab0(), vec![target(TargetKind::Clickable)]));

        let frame = session.compose();
        assert!(frame.image().is_some(), "the picture is still there");
        // The target sits at x=90, y=40 in a 9x20 cell, so its label lands
        // at column 10 of the page's third row, which is frame row 3 once
        // the tab bar has its own.
        assert_ne!(
            frame.cell(CellPos { col: 10, row: 3 }).map(|c| c.ch),
            Some(' '),
            "and the label is on top of it"
        );
    }

    #[test]
    fn text_mode_composes_no_image_at_all() {
        let session = ready_with_graphics();
        assert!(session.compose().image().is_none());
    }

    #[test]
    fn insert_mode_over_a_picture_shows_the_pages_caret_and_not_ours() {
        // Section 6 of the M5 spec. The page drew its own caret into the
        // picture; a second one placed by us would disagree with it.
        let mut session = ready_with_graphics();
        session.on(key('p'));
        session.on(frame_data("AAAA"));
        session.focused_mut().caret = Some(Caret {
            x: 0.0,
            baseline: 16.0,
            offset: 0,
        });
        session.on(key('i'));
        assert_eq!(session.compose().cursor(), None);
    }

    #[test]
    fn the_command_line_keeps_its_caret_in_pixel_mode() {
        // It is painted into a chrome row, which no image ever covers.
        let mut session = ready_with_graphics();
        session.on(key('p'));
        session.on(key(':'));
        assert!(session.compose().cursor().is_some());
    }

    #[test]
    fn set_pixel_off_leaves_pixel_mode() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        typed(&mut session, ":set pixel off");
        session.on(code(KeyCode::Enter));
        assert!(!session.pixel);
        assert!(
            session.compose().image().is_none(),
            "and drops the picture"
        );
    }

    #[test]
    fn set_pixel_on_without_graphics_enters_it_like_the_key_does() {
        // M5 refused both ways. M6 enters both ways, into half-block: what
        // matters here is still that the command and the key agree.
        let mut session = ready();
        typed(&mut session, ":set pixel on");
        session.on(code(KeyCode::Enter));
        assert!(session.pixel);
    }

    #[test]
    fn switching_tabs_moves_the_screencast_with_the_focus() {
        let mut session = ready_with_graphics();
        open_two_more(&mut session);
        session.on(key('p'));
        let leaving = session.focused_id();

        // Alt and a digit, which is how a tab is reached: the bare number
        // row is kept for the count prefix a vim-like puts on it.
        let effects = session.on(alt('1'));
        let arriving = session.focused_id();
        assert_ne!(leaving, arriving);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::StopScreencast(id) if *id == leaving))
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::StartScreencast(id, _) if *id == arriving))
        );
    }

    #[test]
    fn switching_tabs_in_text_mode_asks_for_no_screencast() {
        // The whole helper is a no-op in text mode, or every switch would
        // start a screencast nobody asked for.
        let mut session = ready_with_graphics();
        open_two_more(&mut session);

        let effects = session.on(alt('1'));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::StartScreencast(..) | Effect::StopScreencast(_)))
        );
    }

    #[test]
    fn the_previous_picture_stays_up_until_the_new_tabs_first_frame() {
        // Never blank the frame you are looking at. A switch in pixel mode
        // is a round trip, and until it lands the old picture under the new
        // tab's chrome is better than nothing at all.
        let mut session = ready_with_graphics();
        open_two_more(&mut session);
        session.on(key('p'));
        session.on(frame_for(session.focused_id(), "OLD"));

        session.on(alt('1'));
        assert_eq!(
            session.compose().image().map(|i| i.payload.as_str()),
            Some("OLD"),
            "the picture stands until a new one arrives"
        );
    }

    #[test]
    fn a_resize_restarts_the_screencast_at_the_new_size() {
        // A screencast is started with a viewport and does not learn about a
        // later one, so the same tab has to be stopped and started again.
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let effects = session.on(Event::Resized(
            GridSize { cols: 100, rows: 30 },
            CellSize { w: 9, h: 20 },
        ));
        assert!(effects.iter().any(|e| matches!(e, Effect::StopScreencast(_))));
        assert!(effects.iter().any(|e| matches!(e, Effect::StartScreencast(..))));
    }

    #[test]
    fn a_resize_moves_the_picture_to_the_area_it_now_covers() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        session.on(frame_data("AAAA"));
        session.on(Event::Resized(
            GridSize { cols: 100, rows: 30 },
            CellSize { w: 9, h: 20 },
        ));

        let composed = session.compose();
        let image = composed.image().expect("the picture is still up");
        assert_eq!(image.area.cols, 100);
        assert_eq!(image.area.rows, 30 - CHROME_ROWS);
    }

    #[test]
    fn quitting_from_pixel_mode_stops_the_screencast_first() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let effects = session.on(key('q'));
        let stopped = effects
            .iter()
            .position(|e| matches!(e, Effect::StopScreencast(_)));
        let quit = effects.iter().position(|e| matches!(e, Effect::Quit));
        assert!(stopped.is_some(), "the browser is not left painting");
        assert!(stopped < quit, "and it is stopped before the loop ends");
    }

    #[test]
    fn closing_the_focused_tab_moves_the_screencast_to_the_next_one() {
        let mut session = ready_with_graphics();
        open_two_more(&mut session);
        session.on(key('p'));

        let effects = session.on(key('x'));
        assert!(effects.iter().any(|e| matches!(e, Effect::StartScreencast(..))));
        // Nothing is stopped: the tab is being closed and its target goes
        // with it.
        assert!(!effects.iter().any(|e| matches!(e, Effect::StopScreencast(_))));
    }

    /// What composing a pixel frame costs. Run with:
    ///
    ///     cargo test -p wwt --lib measure_pixel_compose -- --nocapture
    /// What composing a pixel frame costs. Run with:
    ///
    ///     cargo test -p wwt --lib measure_pixel_compose -- --nocapture
    ///
    /// The payload is shared rather than copied, so this is the cost of the
    /// cell grid and the chrome and not of the picture: a few hundred
    /// kilobytes of base64 moving through here would dominate everything.
    #[test]
    fn measure_pixel_compose() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let payload = "A".repeat(400 * 1024);
        session.on(frame_data(&payload));

        let mut worst = std::time::Duration::ZERO;
        for _ in 0..200 {
            let start = std::time::Instant::now();
            let frame = session.compose();
            worst = worst.max(start.elapsed());
            std::hint::black_box(frame);
        }
        eprintln!("pixel compose, worst of 200: {worst:?}");
    }

    /// What a degraded frame costs, from base64 to cells. Run with:
    ///
    ///     cargo test -p wwt --lib measure_halfblock_frame -- --nocapture
    ///
    /// The claim is that half-block is the cheap path: the payload is a few
    /// kilobytes rather than a few hundred, and the decode is a few thousand
    /// pixels. It runs on the loop's thread, so the number that matters is
    /// this one against `FRAME_INTERVAL`, which is 33ms.
    ///
    /// It prints rather than asserting a budget, the way every other
    /// measurement here does: inflate is an order of magnitude slower
    /// unoptimised, so a wall-clock assertion would say more about the
    /// profile than about the decoder. What is asserted is that the frame
    /// really went through the decode, since a picture that failed to
    /// decode would otherwise be the fastest run of all.
    #[test]
    fn measure_halfblock_frame() {
        let mut session = session();
        session.set_graphics(false);
        session.on(key('p'));
        let fixture = fixture_frame();

        let mut worst = std::time::Duration::ZERO;
        for _ in 0..50 {
            // Cloned outside the timer: the copy is the test's cost and not
            // the loop's, which is handed the frame the websocket read.
            let event = Event::Frame(tab0(), Box::new(fixture.clone()));
            let start = std::time::Instant::now();
            session.on(event);
            let frame = session.compose();
            worst = worst.max(start.elapsed());
            std::hint::black_box(frame);
        }
        eprintln!("half-block frame and compose, worst of 50: {worst:?}");
        let frame = session.compose();
        assert_eq!(frame.image(), None, "half-block is cells and never an image");
        assert_eq!(
            frame.cell(CellPos { col: 0, row: 1 }).expect("a page cell").ch,
            '\u{2580}',
            "the picture was decoded rather than dropped"
        );
    }

    // Switching.

    #[test]
    fn alt_and_a_digit_looks_at_the_tab_in_that_position() {
        let mut session = ready();
        open_two_more(&mut session);
        assert_eq!(session.focused().id, TabId(2));

        session.on(alt('1'));
        assert_eq!(session.focused().id, tab0(), "the first tab, whichever you were on");

        session.on(alt('3'));
        assert_eq!(session.focused().id, TabId(2), "and back, without passing the second");
    }

    #[test]
    fn a_digit_past_the_last_tab_leaves_you_where_you_are() {
        // Three tabs and a key for nine of them. The four hundredth is what
        // `:tabnext` is still for.
        let mut session = ready();
        open_two_more(&mut session);

        let effects = session.on(key('$'));

        assert_eq!(session.focused().id, TabId(2), "there is no fourth tab to go to");
        assert_eq!(effects, vec![], "and nothing was asked of the browser");
    }

    #[test]
    fn switching_activates_the_tab_you_switched_to() {
        // Input dispatch is answered by whichever target the browser has in
        // front, so a switch that does not activate leaves clicks landing on
        // the page you just left.
        let mut session = ready();
        open_two_more(&mut session);
        let effects = session.on(alt('1'));
        assert!(effects.contains(&Effect::Activate(tab0())));
    }

    #[test]
    fn a_switch_paints_the_page_you_switched_to_before_anyone_asks_the_browser() {
        let mut session = ready();
        session.focused_mut().runs = vec![run("first tab")];
        open_two_more(&mut session);

        session.on(alt('1'));
        let frame = session.compose();
        assert!(
            (0..frame.grid().rows).any(|r| frame.row_text(r).contains("first tab")),
            "the cached frame is what makes a switch a repaint rather than a round trip"
        );
    }

    #[test]
    fn a_background_tab_that_changed_is_not_read_until_you_look_at_it() {
        let mut session = ready();
        open_two_more(&mut session);

        // Tab 0 is in the background and its page says it moved.
        assert_eq!(
            session.on(Event::Dirty(tab0())),
            vec![],
            "an idle background tab must cost what an idle foreground tab costs"
        );

        // Switching to it spends the flag.
        let effects = session.on(alt('1'));
        assert!(effects.contains(&Effect::Extract(tab0(), Source::Script)));
    }

    #[test]
    fn switching_to_a_tab_that_did_not_change_costs_no_round_trip() {
        let mut session = ready();
        open_two_more(&mut session);
        session.on(alt('1')); // to tab 0, spending its flag
        session.on(Event::Done(Job::Extracted(tab0(), Source::Script, Ok(Box::new(extraction("https://example.com"))))));

        let effects = session.on(alt('3'));
        assert_eq!(
            effects,
            vec![Effect::Activate(TabId(2)), Effect::Save(session.snapshot())],
            "nothing to re-read, though which tab you are on is worth keeping"
        );
    }

    /// What a tab switch costs. Run with:
    ///
    ///     cargo test -p wwt --lib measure_switch -- --nocapture
    ///
    /// The claim in spec section 3 is that a switch is a repaint and no round
    /// trip, so this asserts the absence of an extraction as well as printing
    /// the time. A page's worth of runs is composed and the frame is built,
    /// which is everything between pressing `J` and having the text.
    ///
    /// It needs no browser, which is the point: nothing leaves the process.
    #[test]
    fn measure_switch() {
        let mut session = ready();
        open_two_more(&mut session);
        for tab in 0..3 {
            // Down the page rather than all on one row, or the frame this
            // times is one row of text and the diff has nothing to do.
            session.tabs[tab].runs = (0..300)
                .map(|i| {
                    let mut run = run(&format!("line {i}"));
                    run.rect.y = f64::from(i) * 20.0;
                    run.baseline = run.rect.y + 16.0;
                    run
                })
                .collect();
            session.tabs[tab].read = true;
            session.tabs[tab].dirty = false;
        }

        let mut worst = std::time::Duration::ZERO;
        // Alternating, because going to the tab you are already on is not a
        // switch and would measure nothing.
        for step in 0..200 {
            let to = if step % 2 == 0 { alt('1') } else { alt('3') };
            let start = std::time::Instant::now();
            let effects = session.on(to);
            let frame = session.compose();
            worst = worst.max(start.elapsed());

            assert!(
                !effects.iter().any(|e| matches!(e, Effect::Extract(..))),
                "a clean tab must not be re-read: a switch is a repaint"
            );
            std::hint::black_box(frame);
        }
        eprintln!("switch, worst of 200: {worst:?}");
        // Loose on purpose: it runs on whatever machine CI has.
        assert!(worst < std::time::Duration::from_millis(5), "switch took {worst:?}");

        // A switch to an evicted tab is still a repaint: the runs are cached
        // and the round trip happens behind them. The number that matters is
        // that it is the same order as an attached switch and not an
        // extraction, because that is M4's guarantee surviving M7.
        let mut effects = Vec::new();
        session.detach(TabId(0), &mut effects);
        session.detach(TabId(2), &mut effects);
        let mut detached = std::time::Duration::ZERO;
        for step in 0..200 {
            let to = if step % 2 == 0 { alt('1') } else { alt('3') };
            let start = std::time::Instant::now();
            let effects = session.on(to);
            let frame = session.compose();
            detached = detached.max(start.elapsed());

            assert!(
                !effects.iter().any(|e| matches!(e, Effect::Extract(..))),
                "an evicted tab is repainted from its runs, not re-read"
            );
            assert!(
                effects.iter().any(|e| matches!(e, Effect::OpenTab { .. })),
                "and asked for again behind the frame you are already looking at"
            );
            // The reattach leaves it `Opening`, and the answer never comes
            // in a test, so put it back where the next round expects it.
            session.tab_mut(session.focused_id()).expect("focused").detach();
            std::hint::black_box(frame);
        }
        eprintln!("switch to an evicted tab, worst of 200: {detached:?}");
        assert!(
            detached < std::time::Duration::from_millis(5),
            "detached switch took {detached:?}"
        );
    }

    // Detach and reattach.

    /// Three tabs, all opened and read, focus on the last. `open_two_more`
    /// with the fixture's own tab already read, which is what makes a
    /// switch back to it a repaint.
    fn ready_tabs() -> Session {
        let mut session = ready();
        session.focused_mut().runs = vec![run("first tab")];
        open_two_more(&mut session);
        session
    }

    #[test]
    fn a_detached_tab_is_asked_for_again_when_you_switch_to_it() {
        let mut session = ready_tabs();
        let away = session.tabs[0].id;
        let mut effects = Vec::new();
        session.detach(away, &mut effects);
        assert!(effects.contains(&Effect::Detach(away)));

        session.tabs[0].scroll_y = 900.0;
        let effects = session.on(alt('1'));

        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::OpenTab { id, scroll_y, .. } if *id == away && *scroll_y == 900.0
            )),
            "a reattach is an open, and an open carries the offset: as two \
             effects they are two tasks, and an extraction that wins that \
             race reads offset zero and saves it"
        );
        assert_eq!(session.focused().presence, Presence::Opening);
    }

    #[test]
    fn nothing_is_asked_of_a_detached_tab_while_it_is_away() {
        // `Core` would drop the effect and the flag would never be cleared.
        // This is the rule `Tab::opened` named in M4, under its new name.
        let mut session = ready_tabs();
        let away = session.tabs[0].id;
        let mut effects = Vec::new();
        session.detach(away, &mut effects);

        let effects = session.on(Event::Dirty(away));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Extract(id, _) if *id == away)),
            "a detached tab has no page to read"
        );
        assert!(session.tabs[0].dirty, "the flag is kept and spent on reattach");
        assert!(!session.tabs[0].reading);
    }

    #[test]
    fn switching_to_a_detached_tab_paints_what_it_looked_like_first() {
        // M4's repaint guarantee survives eviction: the runs are still
        // here, so the switch is a repaint and the round trip happens
        // behind it.
        let mut session = ready_tabs();
        let away = session.tabs[0].id;
        let runs = session.tabs[0].runs.len();
        assert!(runs > 0, "the fixture must have read this tab");
        let mut effects = Vec::new();
        session.detach(away, &mut effects);

        session.on(alt('1'));
        assert_eq!(session.focused().runs.len(), runs);
        let frame = session.compose();
        assert!(
            (0..frame.grid().rows).any(|r| frame.row_text(r).contains("first tab")),
            "the frame you switch to is the one the tab last looked like"
        );
    }

    // Eviction.

    /// Four tabs, all attached and read, focus on the last.
    fn four_ready_tabs() -> Session {
        let mut session = ready();
        for (n, url) in [(1u32, "one.test"), (2, "two.test"), (3, "three.test")] {
            typed(&mut session, &format!(":tabopen {url}"));
            session.on(code(KeyCode::Enter));
            session.on(Event::Done(Job::Opened(TabId(n), Ok(()))));
            session.on(Event::Done(Job::Extracted(
                TabId(n),
                Source::Script,
                Ok(Box::new(extraction(&format!("https://{url}")))),
            )));
        }
        session
    }

    #[test]
    fn the_tab_you_looked_at_longest_ago_is_the_one_that_goes() {
        let mut session = four_ready_tabs();
        let oldest = session.tabs[0].id;

        // Visit 1, 2, 3 in order, leaving tab 0 the least recently seen.
        session.on(alt('2'));
        session.on(alt('3'));
        session.on(alt('4'));

        session.max_tabs = 3;
        let effects = session.on(alt('2'));

        assert!(effects.contains(&Effect::Detach(oldest)));
        assert_eq!(session.tabs[0].presence, Presence::Detached);
        assert_eq!(session.tabs.len(), 4, "an evicted tab is still a tab");
    }

    #[test]
    fn the_tab_you_are_looking_at_is_never_the_one_that_goes() {
        let mut session = four_ready_tabs();
        session.max_tabs = 1;
        let effects = session.on(alt('2'));
        let focused = session.focused_id();
        assert!(!effects.contains(&Effect::Detach(focused)));
        assert!(session.focused().attached() || session.focused().presence == Presence::Opening);
    }

    #[test]
    fn a_tab_with_an_answer_coming_is_left_alone() {
        // Its url still names where it is leaving, so reattaching later
        // would take you back to the page it navigated away from.
        let mut session = four_ready_tabs();
        session.max_tabs = 2;
        let busy = session.tabs[0].id;
        session.tab_mut(busy).expect("fixture").navigating = true;

        let effects = session.on(alt('4'));
        assert!(!effects.contains(&Effect::Detach(busy)));
    }

    #[test]
    fn nothing_eligible_means_nothing_evicted() {
        // The limit is a target and not a guarantee. The alternative is
        // racing an answer that is already on its way in order to honour a
        // number that exists to bound memory.
        let mut session = four_ready_tabs();
        session.max_tabs = 1;
        for tab in &mut session.tabs {
            tab.reading = true;
        }
        let effects = session.on(alt('2'));
        assert!(!effects.iter().any(|e| matches!(e, Effect::Detach(_))));
    }

    #[test]
    fn a_tab_already_away_does_not_count_against_the_limit() {
        // The limit counts live targets, which is what costs memory, and
        // not tabs, which are cheap and all of which the bar goes on
        // showing.
        let mut session = four_ready_tabs();
        session.max_tabs = 3;
        let mut effects = Vec::new();
        let away = session.tabs[0].id;
        session.detach(away, &mut effects);

        let effects = session.on(alt('3'));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Detach(_))),
            "three attached tabs is not over a limit of three"
        );
    }

    // The supervisor.

    #[test]
    fn a_dead_browser_leaves_every_tab_where_it_was_and_asks_for_another() {
        let mut session = four_ready_tabs();
        session.tabs[0].scroll_y = 500.0;
        session.focused_mut().runs = vec![run("still here")];
        let effects = session.on(Event::BrowserLost);

        assert!(effects.contains(&Effect::Relaunch));
        assert!(
            session.tabs.iter().all(|tab| tab.presence == Presence::Detached),
            "there is no browser, so no tab has a target"
        );
        assert_eq!(session.tabs.len(), 4, "the tabs are what a restart comes back to");
        assert_eq!(session.tabs[0].scroll_y, 500.0);
        assert!(
            !session.focused().runs.is_empty(),
            "never blank the frame you are looking at"
        );
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Detach(_))),
            "there is nothing on the other end to close"
        );
    }

    #[test]
    fn a_browser_that_came_back_is_asked_for_one_page() {
        let mut session = four_ready_tabs();
        session.on(Event::BrowserLost);
        let effects = session.on(Event::BrowserBack);

        let opens: Vec<_> = effects
            .iter()
            .filter(|e| matches!(e, Effect::OpenTab { .. }))
            .collect();
        assert_eq!(opens.len(), 1, "the restart path is lazy restore");
        assert_eq!(session.focused().presence, Presence::Opening);
    }

    #[test]
    fn a_held_key_asks_for_one_relaunch_and_not_thirty() {
        let mut session = four_ready_tabs();
        session.on(Event::BrowserLost);
        session.on(Event::Done(Job::Relaunched(Err("no chromium".to_string()))));

        let first = session.on(key('j'));
        assert!(first.contains(&Effect::Relaunch), "a keystroke is how you ask again");
        let second = session.on(key('j'));
        assert!(
            !second.contains(&Effect::Relaunch),
            "one in flight is enough; the flag is cleared by its answer"
        );
        assert!(
            !first.iter().any(|e| matches!(e, Effect::Scroll(..))),
            "there is no page to scroll"
        );
    }

    #[test]
    fn a_relaunch_that_failed_leaves_the_frame_you_were_reading() {
        let mut session = four_ready_tabs();
        session.focused_mut().runs = vec![run("still here")];
        let runs = session.focused().runs.len();
        session.on(Event::BrowserLost);
        session.on(Event::Done(Job::Relaunched(Err("no chromium".to_string()))));

        assert_eq!(session.focused().runs.len(), runs);
        assert!(matches!(session.state(), State::Error(_) | State::Notice(_)));
    }

    #[test]
    fn a_hint_answer_for_a_tab_you_have_left_does_not_put_labels_over_another_page() {
        let mut session = ready();
        open_two_more(&mut session);

        session.on(key('f'));
        session.on(alt('1'));
        session.on(hinted_for(TabId(2), vec![target(TargetKind::Clickable)]));

        assert_eq!(
            session.mode(),
            &Mode::Normal,
            "labels measured against one page must not be painted over another"
        );
    }
}
