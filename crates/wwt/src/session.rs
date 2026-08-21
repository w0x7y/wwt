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
use wwt_frame::{
    CellPos, CellSize, Frame, GridSize, HintTarget, TargetKind, Viewport,
};
use wwt_page::{Input, MouseInput};
use wwt_ui::Mode;
use wwt_ui::chrome::{self, State};
use wwt_ui::command::{self, Command, Setting};
use wwt_ui::hint::{Filtered, HintSession};

use crate::effect::{Effect, Navigation, Scroll};
use crate::event::{Event, Job};
use crate::keymap::{Action, action_for};
use crate::keys;
use crate::tab::{Tab, TabId};

/// How far one notch of the wheel scrolls, in rows. Three is what a desktop
/// browser does, and matching it is what makes the page feel normal.
const WHEEL_ROWS: f64 = 3.0;

/// What Chromium navigates to when it cannot reach a host.
const CHROME_ERROR_SCHEME: &str = "chrome-error://";

pub struct Session {
    grid: GridSize,
    cell: CellSize,
    vp: Viewport,

    mode: Mode,

    tabs: Vec<Tab>,
    focus: usize,
    /// Never reused, which is what makes a job from a closed tab safe to
    /// drop rather than plausible to paint.
    next_id: u32,
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
    pub fn new(grid: GridSize, cell: CellSize) -> Self {
        let mut session = Self {
            grid,
            cell,
            vp: page_viewport(grid, cell),
            mode: Mode::Normal,
            tabs: Vec::new(),
            focus: 0,
            next_id: 0,
        };
        let id = session.mint();
        session.tabs.push(Tab::new(id, String::new()));
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

    pub fn focused_id(&self) -> TabId {
        self.focused().id
    }

    /// The first read of a page nobody has looked at yet.
    pub fn begin(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.start_extract(&mut effects);
        effects
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
        frame.paint_runs(&self.vp, &tab.runs);

        // After the page and before the chrome: labels cover the text they
        // point at, which is what makes them readable, and the chrome still
        // owns its rows.
        if let Mode::Hint(session) = &self.mode {
            session.paint(&mut frame, &self.vp);
        }

        let titles: Vec<String> = self.tabs.iter().map(|tab| tab.title.clone()).collect();
        chrome::paint_tabs(&mut frame, &titles, self.focus);
        chrome::paint(
            &mut frame,
            &self.mode,
            &tab.state,
            &tab.url,
            &tab.title,
            tab.progress,
        );

        // One place decides where the cursor goes, though two modes have an
        // insertion point. Splitting that between here and the chrome would
        // leave the two exclusive only by accident of paint order.
        frame.set_cursor(match &self.mode {
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
            Event::Dirty => {
                self.focused_mut().mark_dirty();
                self.start_extract(&mut effects);
            }
            Event::Done(job) => self.on_job(job, &mut effects),
        }
        effects
    }

    fn on_key(&mut self, key: KeyEvent, effects: &mut Vec<Effect>) {
        let Some(action) = action_for(&self.mode, key, self.vp) else {
            return;
        };
        self.run_action(action, effects);
    }

    fn run_action(&mut self, action: Action, effects: &mut Vec<Effect>) {
        match action {
            Action::Quit => effects.push(Effect::Quit),
            Action::EnterCommand(prefill) => self.mode = Mode::Command(prefill),
            Action::Insert => self.mode = Mode::Insert,
            Action::Hints => match self.focused().hints.clone() {
                Some(targets) => self.enter_hints(targets),
                // `f` pressed twice before the first answer comes back is
                // one question, not two.
                None if !self.focused().hinting => {
                    self.focused_mut().hinting = true;
                    effects.push(Effect::Hints);
                }
                None => {}
            },

            // Scrolling does not settle the way a navigation does; the
            // page's own scroll listener reports when it has moved.
            Action::Scroll(dy) => effects.push(Effect::Scroll(Scroll::By(dy))),
            Action::ScrollTop => effects.push(Effect::Scroll(Scroll::Top)),
            Action::ScrollEnd => effects.push(Effect::Scroll(Scroll::End)),
            Action::Back => self.navigate(Navigation::Back, effects),
            Action::Forward => self.navigate(Navigation::Forward, effects),
            Action::Reload => self.navigate(Navigation::Reload, effects),

            Action::Leave => {
                // Leaving insert mode has already happened by the time the
                // blur runs. If it fails the statusline says so and the
                // keyboard is still yours: taking it back must never depend
                // on the page.
                if self.mode == Mode::Insert {
                    effects.push(Effect::Blur);
                }
                self.mode = Mode::Normal;
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
                match command::parse(&line) {
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

            Action::Send(key) => self.send_key(key, effects),
        }
    }

    /// Forward one key to the page, if it is one we know how to describe.
    ///
    /// An unknown key is dropped rather than approximated: a wrong `code` is
    /// worse than a missing keystroke, because the page acts on it.
    fn send_key(&self, key: KeyEvent, effects: &mut Vec<Effect>) {
        if let Some(input) = keys::describe(key) {
            effects.push(Effect::Send(Input::Key(input)));
        }
    }

    fn run_command(&mut self, command: Command, effects: &mut Vec<Effect>) {
        match command {
            Command::Open(url) => {
                self.focused_mut().url = url.clone();
                self.navigate(Navigation::Open(url), effects);
            }
            Command::Back => self.navigate(Navigation::Back, effects),
            Command::Forward => self.navigate(Navigation::Forward, effects),
            Command::Reload => self.navigate(Navigation::Reload, effects),
            Command::Set(Setting::Mouse(on)) => {
                effects.push(Effect::MouseCapture(on));
                self.focused_mut().state =
                    State::Notice(if on { "mouse on" } else { "mouse off" }.to_string());
            }
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
        let tab = self.focused_mut();
        tab.navigating = true;
        tab.state = State::Loading;
        effects.push(Effect::Navigate(navigation));
    }

    fn on_mouse(&mut self, event: MouseEvent, effects: &mut Vec<Effect>) {
        let Some(cell) = page_cell(&self.vp, event.column, event.row) else {
            return;
        };
        // `to_css` returns the cell's centre, so the click lands
        // unambiguously inside the cell you pointed at.
        let at = self.vp.to_css(cell);
        let notch = WHEEL_ROWS * f64::from(self.cell.h);

        let mouse = match event.kind {
            MouseEventKind::Down(MouseButton::Left) => MouseInput::press(at),
            MouseEventKind::Up(MouseButton::Left) => MouseInput::release(at),
            MouseEventKind::ScrollDown => MouseInput::wheel(at, notch),
            MouseEventKind::ScrollUp => MouseInput::wheel(at, -notch),
            // Motion would cost a round trip per reported frame, and there is
            // no context menu to open and no tab to middle-click into.
            _ => return,
        };
        effects.push(Effect::Send(Input::Mouse(mouse)));
    }

    fn start_extract(&mut self, effects: &mut Vec<Effect>) {
        let tab = self.focused_mut();
        if tab.extracting || !tab.dirty {
            return;
        }
        tab.extracting = true;
        tab.dirty = false;
        effects.push(Effect::Extract);
    }

    fn enter_hints(&mut self, targets: Vec<HintTarget>) {
        let session = HintSession::new(targets);
        if session.is_empty() {
            // Entering a mode with nothing in it would only need escaping.
            self.focused_mut().state = State::Notice("no hints".to_string());
            return;
        }
        self.mode = Mode::Hint(session);
    }

    /// Apply what filtering decided about the character just typed.
    fn on_filtered(&mut self, filtered: Filtered, effects: &mut Vec<Effect>) {
        match filtered {
            Filtered::Waiting(_) => {}
            Filtered::Activate(target) => self.activate(target, effects),
            // Nothing matches, so there is nothing left to type. Leaving is
            // friendlier than sitting there waiting for an Esc.
            Filtered::None => self.mode = Mode::Normal,
        }
    }

    fn activate(&mut self, target: HintTarget, effects: &mut Vec<Effect>) {
        let at = target.center();
        effects.push(Effect::Send(Input::Mouse(MouseInput::press(at))));
        effects.push(Effect::Send(Input::Mouse(MouseInput::release(at))));
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
        // The page genuinely reflows: it is being told the window changed
        // size. Extraction waits for `Job::Resized`, because reading the
        // page before it has been resized reads the old layout.
        effects.push(Effect::SetViewport(self.vp));
    }

    fn on_job(&mut self, job: Job, effects: &mut Vec<Effect>) {
        match job {
            Job::Extracted(extraction) => {
                let progress = extraction.scroll_progress();
                let tab = self.focused_mut();
                tab.extracting = false;
                tab.progress = progress;
                tab.scroll_y = extraction.scroll_y;
                tab.runs = extraction.runs;
                tab.caret = extraction.caret;
                tab.title = extraction.title;

                // Chromium answers a DNS or connection failure by navigating
                // to its own error page rather than failing the command, so a
                // navigation can "succeed" into one. Its error page is more
                // use than a stale frame — it says what went wrong — but the
                // statusline must not go on claiming the page is fine.
                if extraction.url.starts_with(CHROME_ERROR_SCHEME) {
                    // The statusline prints the URL itself, so naming it here
                    // too would print it twice.
                    tab.state = State::Error("could not be reached".to_string());
                } else {
                    tab.url = extraction.url;
                    if !tab.navigating {
                        tab.state = State::Ready;
                    }
                }
                // The page may have changed again while we were extracting.
                self.start_extract(effects);
            }
            Job::Hints(result) => {
                // However it went, the query is over and `f` must work again.
                self.focused_mut().hinting = false;
                match result {
                    Ok(targets) => {
                        self.focused_mut().hints = Some(targets.clone());
                        // A query is a round trip, and the keystroke that
                        // asked for it was normal mode's. Landing the answer
                        // in whatever mode you have since entered would take
                        // the command line out from under you mid-word.
                        if self.mode == Mode::Normal {
                            self.enter_hints(targets);
                        }
                    }
                    Err(message) => self.focused_mut().state = State::Error(message),
                }
            }
            Job::Settled => {
                let tab = self.focused_mut();
                tab.navigating = false;
                tab.state = State::Ready;
                tab.mark_dirty();
                self.start_extract(effects);
            }
            Job::Resized => {
                self.focused_mut().mark_dirty();
                self.start_extract(effects);
            }
            // The frame stays exactly as it was; only the statusline
            // changes. Spec section 8. Deliberately not `Job::Failed`: that
            // one clears the extraction and navigation flags, and a
            // keystroke that failed has finished neither of those.
            Job::InputFailed(message) => self.focused_mut().state = State::Error(message),
            Job::Failed(message) => {
                let tab = self.focused_mut();
                tab.extracting = false;
                tab.navigating = false;
                // The frame stays exactly as it was; only the statusline
                // changes. Section 8: never blank the frame you are looking at.
                tab.state = State::Error(message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use wwt_frame::{Caret, CssRect};
    use wwt_page::Extraction;

    const GRID: GridSize = GridSize { cols: 80, rows: 24 };
    const CELL: CellSize = CellSize { w: 9, h: 20 };

    fn session() -> Session {
        Session::new(GRID, CELL)
    }

    /// A session past its first extraction, the state most keys are pressed in.
    fn ready() -> Session {
        let mut session = session();
        session.begin();
        session.on(Event::Done(Job::Extracted(Box::new(extraction("https://example.com")))));
        session
    }

    fn extraction(url: &str) -> Extraction {
        Extraction {
            runs: Vec::new(),
            caret: None,
            title: "Example".to_string(),
            url: url.to_string(),
            scroll_y: 0.0,
            scroll_height: 1000.0,
            viewport_height: 460.0,
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

    fn target(kind: TargetKind) -> HintTarget {
        HintTarget { rect: CssRect { x: 90.0, y: 40.0, w: 90.0, h: 20.0 }, kind }
    }

    /// The page answering a hint query.
    fn hinted(targets: Vec<HintTarget>) -> Event {
        Event::Done(Job::Hints(Ok(targets)))
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
        assert_eq!(session().begin(), vec![Effect::Extract]);
    }

    #[test]
    fn a_dirty_signal_during_an_extraction_re_runs_it_once_not_twice() {
        let mut session = session();
        assert_eq!(session.begin(), vec![Effect::Extract]);

        // Three signals arrive while that extraction is still in flight.
        for _ in 0..3 {
            assert_eq!(session.on(Event::Dirty), vec![], "a second extraction would race it");
        }

        // Finishing it starts exactly one more, covering all three.
        let effects = session.on(Event::Done(Job::Extracted(Box::new(extraction("about:blank")))));
        assert_eq!(effects, vec![Effect::Extract]);
    }

    #[test]
    fn a_page_that_stopped_changing_stops_being_read() {
        let mut session = ready();
        assert_eq!(
            session.on(Event::Done(Job::Extracted(Box::new(extraction("about:blank"))))),
            vec![],
            "an idle page must cost nothing"
        );
    }

    #[test]
    fn a_failed_extraction_lets_the_next_one_start() {
        let mut session = session();
        session.begin();
        session.on(Event::Done(Job::Failed("boom".to_string())));
        assert_eq!(session.on(Event::Dirty), vec![Effect::Extract]);
    }

    // What a finished job says about the page.

    #[test]
    fn a_chrome_error_url_is_an_error_without_becoming_the_url() {
        let mut session = ready();
        session.on(Event::Dirty);
        session.on(Event::Done(Job::Extracted(Box::new(extraction(
            "chrome-error://chromewebdata/",
        )))));

        assert_eq!(session.state(), &State::Error("could not be reached".to_string()));
        assert!(
            !session.compose().row_text(23).contains("chrome-error"),
            "the statusline prints the URL itself, so naming it twice reads as a bug"
        );
    }

    #[test]
    fn a_keystroke_that_failed_leaves_the_page_alone() {
        let mut session = ready();
        session.on(Event::Dirty);
        let mid_extraction = session.on(Event::Done(Job::InputFailed("no".to_string())));

        assert_eq!(session.state(), &State::Error("no".to_string()));
        assert_eq!(mid_extraction, vec![], "an extraction was in flight and still is");
        // The one already running still finishes, and finds nothing to do.
        assert_eq!(
            session.on(Event::Done(Job::Extracted(Box::new(extraction("about:blank"))))),
            vec![]
        );
    }

    #[test]
    fn a_failure_never_blanks_the_frame() {
        let mut session = ready();
        let before = session.compose();
        session.on(Event::Done(Job::Failed("the page went away".to_string())));
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

        assert_eq!(session.on(code(KeyCode::Esc)), vec![Effect::Blur]);
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
        assert_eq!(session.on(code(KeyCode::Esc)), vec![Effect::Blur], "ours, always");

        session.on(key('i'));
        let sent = session.on(ctrl(']'));
        let [Effect::Send(Input::Key(sent))] = sent.as_slice() else {
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
                if let Effect::Send(Input::Key(key)) = effect {
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
            vec![Effect::Navigate(Navigation::Open("https://example.co".to_string()))],
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
        assert_eq!(session.on(key('H')), vec![Effect::Navigate(Navigation::Back)]);
        assert_eq!(session.on(key('L')), vec![], "one navigation at a time");

        session.on(Event::Done(Job::Settled));
        assert_eq!(session.on(key('L')), vec![Effect::Navigate(Navigation::Forward)]);
    }

    #[test]
    fn a_settled_navigation_reads_the_new_page() {
        let mut session = ready();
        session.on(key('H'));
        assert_eq!(session.on(Event::Done(Job::Settled)), vec![Effect::Extract]);
        assert_eq!(session.state(), &State::Ready);
    }

    #[test]
    fn scrolling_asks_the_page_to_move_and_waits_to_be_told_it_did() {
        let mut session = ready();
        // One row of 20 CSS pixels.
        assert_eq!(session.on(key('j')), vec![Effect::Scroll(Scroll::By(20.0))]);
        assert_eq!(session.on(key('g')), vec![Effect::Scroll(Scroll::Top)]);
        assert_eq!(session.on(key('G')), vec![Effect::Scroll(Scroll::End)]);
    }

    // Hints: cached until the page moves, and a text field lands in insert.

    #[test]
    fn f_queries_the_page_once_and_then_uses_what_it_said() {
        let mut session = ready();
        assert_eq!(session.on(key('f')), vec![Effect::Hints]);

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

        session.on(Event::Dirty);
        assert_eq!(
            session.on(key('f')),
            vec![Effect::Hints],
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
                Effect::Send(Input::Mouse(MouseInput::press(at))),
                Effect::Send(Input::Mouse(MouseInput::release(at))),
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
                Effect::Send(Input::Mouse(MouseInput::press(at))),
                Effect::Send(Input::Mouse(MouseInput::release(at))),
            ]
        );
        assert_eq!(session.mode(), &Mode::Normal, "a link is finished when the click lands");
    }

    #[test]
    fn a_query_still_in_flight_is_not_asked_again() {
        let mut session = ready();
        assert_eq!(session.on(key('f')), vec![Effect::Hints]);
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
        session.on(Event::Done(Job::Hints(Err("the page went away".to_string()))));

        assert_eq!(session.state(), &State::Error("the page went away".to_string()));
        assert_eq!(
            session.on(key('f')),
            vec![Effect::Hints],
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
        assert_eq!(effects, vec![Effect::Send(Input::Mouse(MouseInput::press(at)))]);
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
        assert_eq!(effects, vec![Effect::Send(Input::Mouse(MouseInput::wheel(at, 60.0)))]);
    }

    #[test]
    fn a_mouse_move_is_not_worth_a_round_trip() {
        let mut session = ready();
        assert_eq!(session.on(mouse(MouseEventKind::Moved, 4, 2)), vec![]);
    }

    // Resize.

    #[test]
    fn a_resize_tells_the_page_before_reading_it() {
        let mut session = ready();
        let grid = GridSize { cols: 100, rows: 30 };
        let effects = session.on(Event::Resized(grid, CELL));

        assert_eq!(effects, vec![Effect::SetViewport(page_viewport(grid, CELL))]);
        assert_eq!(
            session.on(Event::Done(Job::Resized)),
            vec![Effect::Extract],
            "reading before the page has reflowed reads the old layout"
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
        session.on(Event::Done(Job::Extracted(Box::new(with_caret))));
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
}
