//! The event loop. It owns all state and is the only thing that mutates it.

use std::io::Write;
use std::sync::Arc;

use anyhow::{Context, Result};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event as TermEvent, EventStream, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, sleep_until};
use wwt_cdp::Client;
use wwt_frame::{
    CellPos, CellSize, CssPoint, CssRect, Frame, GridSize, HintTarget, Style, TargetKind, TextRun,
    Viewport,
};
use wwt_page::{DIRTY_BINDING, Extraction, MouseInput, Page};
use wwt_term::Renderer;
use wwt_ui::Mode;
use wwt_ui::chrome::{self, State};
use wwt_ui::command::{self, Command, Setting};
use wwt_ui::hint::{Filtered, HintSession};

use crate::input::{Input, InputPump};
use crate::keymap::{Action, action_for};
use crate::keys;

/// A dragged window edge produces a resize event per frame, and each one
/// would otherwise cost a Chromium relayout and a full extraction.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(100);

/// How far one notch of the wheel scrolls, in rows. Three is what a desktop
/// browser does, and matching it is what makes the page feel normal.
const WHEEL_ROWS: f64 = 3.0;

/// What Chromium navigates to when it cannot reach a host.
const CHROME_ERROR_SCHEME: &str = "chrome-error://";

/// The result of something that ran off the loop's thread.
enum Job {
    Extracted(Box<Extraction>),
    Failed(String),
    /// A navigation, history move, or reload finished.
    Settled,
    /// A key, a click, or a blur failed after the loop had moved on.
    InputFailed(String),
    /// The page reported its interactive boxes.
    Hints(Vec<HintTarget>),
}

/// The page viewport: the terminal grid, less the row chrome occupies.
///
/// Chromium is told this is the whole window, so the page genuinely does not
/// know the statusline exists.
pub fn page_viewport(grid: GridSize, cell: CellSize) -> Viewport {
    let rows = grid.rows.saturating_sub(1).max(1);
    Viewport::new(GridSize { cols: grid.cols, rows }, cell)
}

/// The page cell a terminal cell refers to, or `None` when it is one of ours.
///
/// The last row is chrome. The page does not know it exists, so a click there
/// has no page coordinate to become.
pub fn page_cell(vp: &Viewport, column: u16, row: u16) -> Option<CellPos> {
    let grid = vp.grid();
    (column < grid.cols && row < grid.rows).then_some(CellPos { col: column, row })
}

/// The cell the insertion point sits in, or `None` when it is off the page.
///
/// Measured at the middle of the caret's line rather than its top edge, so a
/// caret straddling a row boundary lands on the row its text is on.
pub fn caret_cell(vp: &Viewport, caret: &CssRect) -> Option<CellPos> {
    vp.to_cell(CssPoint { x: caret.x, y: caret.y + caret.h / 2.0 })
}

pub struct Core {
    page: Arc<Page>,
    client: Arc<Client>,
    grid: GridSize,
    cell: CellSize,
    vp: Viewport,
    renderer: Renderer,

    mode: Mode,
    state: State,
    url: String,
    title: String,
    progress: f64,
    runs: Vec<TextRun>,
    /// Where typing would land, when the page has a field focused.
    caret: Option<CssRect>,

    /// The page says it changed and we have not caught up yet.
    dirty: bool,
    /// An extraction is in flight; a second would race it.
    extracting: bool,
    /// A navigation is in flight.
    navigating: bool,
    /// The last hint query's targets, held so that pressing `f` twice on a
    /// page that has not moved costs one round trip rather than two.
    hints: Option<Vec<HintTarget>>,
    /// A mouse capture change waiting for the next write to the terminal,
    /// because that is where we have something to write to.
    mouse_pending: Option<bool>,

    jobs_tx: mpsc::UnboundedSender<Job>,
    jobs_rx: mpsc::UnboundedReceiver<Job>,

    /// Ordered delivery of keys and clicks to the page.
    input: InputPump,
    /// Where things that failed after the loop moved on report themselves:
    /// a keystroke that did not land, a blur that did not take.
    errors_tx: mpsc::UnboundedSender<String>,
}

impl Core {
    pub fn new(page: Arc<Page>, client: Arc<Client>, grid: GridSize, cell: CellSize) -> Self {
        let (jobs_tx, jobs_rx) = mpsc::unbounded_channel();
        let (errors_tx, mut errors_rx) = mpsc::unbounded_channel::<String>();
        let input = InputPump::spawn(Arc::clone(&page), errors_tx.clone());

        // Input failures arrive on their own channel and are folded into the
        // one the loop already selects on. Two receivers would mean two
        // mutable borrows of `self` inside one `select!`, which does not
        // compile, and a second failure path the loop has to think about.
        let jobs_for_errors = jobs_tx.clone();
        tokio::spawn(async move {
            while let Some(message) = errors_rx.recv().await {
                let _ = jobs_for_errors.send(Job::InputFailed(message));
            }
        });

        Self {
            page,
            client,
            grid,
            cell,
            vp: page_viewport(grid, cell),
            renderer: Renderer::new(),
            mode: Mode::Normal,
            state: State::Loading,
            url: String::new(),
            title: String::new(),
            progress: 0.0,
            runs: Vec::new(),
            caret: None,
            dirty: true,
            extracting: false,
            navigating: false,
            hints: None,
            mouse_pending: None,
            jobs_tx,
            jobs_rx,
            input,
            errors_tx,
        }
    }

    /// Paint the page and the chrome into one full-grid frame.
    pub fn compose(&self) -> Frame {
        let mut frame = Frame::new(self.grid);
        for run in &self.runs {
            frame.paint_run(&self.vp, run);
        }
        // Only in insert mode. A page can focus a field without your asking,
        // and a caret there would promise that your typing lands in it when
        // in normal mode it does not.
        if self.mode == Mode::Insert {
            self.paint_caret(&mut frame);
        }

        // After the page and before the chrome: labels cover the text they
        // point at, which is what makes them readable, and the chrome still
        // owns its row.
        if let Mode::Hint(session) = &self.mode {
            session.paint(&mut frame, &self.vp);
        }
        chrome::paint(
            &mut frame,
            &self.mode,
            &self.state,
            &self.url,
            &self.title,
            self.progress,
        );
        frame
    }

    /// Invert the cell the insertion point is in.
    ///
    /// Inverting rather than overwriting keeps the character underneath
    /// readable: the caret shows the cell you are about to type over.
    fn paint_caret(&self, frame: &mut Frame) {
        let Some(caret) = &self.caret else { return };
        let Some(cell) = caret_cell(&self.vp, caret) else { return };
        let Some(under) = frame.cell(cell) else { return };

        let (ch, style) = (under.ch, under.style);
        frame.paint_text(
            cell,
            &ch.to_string(),
            Style { reverse: !style.reverse, ..style },
        );
    }

    pub async fn run(&mut self, out: &mut impl Write) -> Result<()> {
        let mut terminal = EventStream::new();
        let mut cdp = self.client.subscribe();
        let mut resize_at: Option<Instant> = None;

        self.start_extract();
        self.present(out)?;

        loop {
            tokio::select! {
                Some(Ok(event)) = terminal.next() => {
                    match event {
                        TermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                            if self.on_key(key) {
                                return Ok(());
                            }
                        }
                        TermEvent::Mouse(mouse) => self.on_mouse(mouse),
                        TermEvent::Resize(..) => {
                            resize_at = Some(Instant::now() + RESIZE_DEBOUNCE);
                        }
                        _ => {}
                    }
                }

                Some(event) = cdp.recv() => {
                    let ours = event.session_id.as_deref() == Some(self.page.session_id());
                    if ours
                        && event.method == "Runtime.bindingCalled"
                        && event.params["name"] == DIRTY_BINDING
                    {
                        self.mark_dirty();
                        self.start_extract();
                    }
                }

                Some(job) = self.jobs_rx.recv() => {
                    self.on_job(job);
                }

                () = async { sleep_until(resize_at.expect("guarded")).await },
                    if resize_at.is_some() =>
                {
                    resize_at = None;
                    self.on_resize().await?;
                }
            }

            self.present(out)?;
        }
    }

    fn present(&mut self, out: &mut impl Write) -> Result<()> {
        if let Some(on) = self.mouse_pending.take() {
            if on {
                execute!(out, EnableMouseCapture).context("enable mouse capture")?;
            } else {
                execute!(out, DisableMouseCapture).context("disable mouse capture")?;
            }
        }

        let frame = self.compose();
        self.renderer.render(&frame, out).context("write the frame")?;
        out.flush().context("flush the terminal")?;
        Ok(())
    }

    /// Handle one key. Returns `true` when it is time to quit.
    fn on_key(&mut self, key: KeyEvent) -> bool {
        match &self.mode {
            Mode::Command(buffer) => {
                let mut buffer = buffer.clone();
                match key.code {
                    KeyCode::Esc => self.mode = Mode::Normal,
                    KeyCode::Backspace => {
                        buffer.pop();
                        self.mode = Mode::Command(buffer);
                    }
                    KeyCode::Enter => {
                        self.mode = Mode::Normal;
                        match command::parse(&buffer) {
                            Ok(Command::Quit) => return true,
                            Ok(command) => self.run_command(command),
                            Err(message) => self.state = State::Error(message),
                        }
                    }
                    KeyCode::Char(c) => {
                        buffer.push(c);
                        self.mode = Mode::Command(buffer);
                    }
                    _ => {}
                }
                false
            }
            Mode::Insert => {
                match key.code {
                    // Never forwarded. A page cannot trap the keyboard,
                    // which is what makes handing it over safe.
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        self.blur();
                    }
                    // A terminal transmits `Ctrl-[` as 0x1B, which is
                    // Escape: the two are one keystroke on the wire. So the
                    // page's Escape lives on `Ctrl-]`, which is 0x1D.
                    KeyCode::Char(']') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.send_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                    }
                    _ => self.send_key(key),
                }
                false
            }
            Mode::Hint(session) => {
                let mut session = session.clone();
                match key.code {
                    KeyCode::Esc => self.mode = Mode::Normal,
                    KeyCode::Backspace => {
                        let filtered = session.pop();
                        self.on_filtered(session, filtered);
                    }
                    KeyCode::Char(c) => {
                        let filtered = session.push(c);
                        self.on_filtered(session, filtered);
                    }
                    _ => {}
                }
                false
            }
            Mode::Normal => match action_for(key, self.vp) {
                Some(Action::Quit) => true,
                Some(Action::EnterCommand(prefill)) => {
                    self.mode = Mode::Command(prefill);
                    false
                }
                Some(Action::Insert) => {
                    self.mode = Mode::Insert;
                    false
                }
                Some(Action::Hints) => {
                    match self.hints.clone() {
                        Some(targets) => self.enter_hints(targets),
                        None => self.start_hints(),
                    }
                    false
                }
                Some(action) => {
                    self.run_action(action);
                    false
                }
                None => false,
            },
        }
    }

    fn on_mouse(&mut self, event: MouseEvent) {
        let Some(cell) = page_cell(&self.vp, event.column, event.row) else {
            return;
        };
        // `to_css` returns the cell's centre, so the click lands
        // unambiguously inside the cell you pointed at.
        let at = self.vp.to_css(cell);
        let notch = WHEEL_ROWS * f64::from(self.cell.h);

        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.input.send(Input::Mouse(MouseInput::press(at)));
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.input.send(Input::Mouse(MouseInput::release(at)));
            }
            MouseEventKind::ScrollDown => {
                self.input.send(Input::Mouse(MouseInput::wheel(at, notch)));
            }
            MouseEventKind::ScrollUp => {
                self.input.send(Input::Mouse(MouseInput::wheel(at, -notch)));
            }
            // Motion would cost a round trip per reported frame, and there is
            // no context menu to open and no tab to middle-click into.
            _ => {}
        }
    }

    /// Say something in the statusline before the loop starts.
    pub fn notice(&mut self, message: &str) {
        self.state = State::Notice(message.to_string());
    }

    /// Forward one key to the page, if it is one we know how to describe.
    ///
    /// An unknown key is dropped rather than approximated: a wrong `code` is
    /// worse than a missing keystroke, because the page acts on it.
    fn send_key(&self, key: KeyEvent) {
        if let Some(input) = keys::describe(key) {
            self.input.send(Input::Key(input));
        }
    }

    /// Take focus off whatever has it, without waiting for it.
    ///
    /// Leaving insert mode has already happened by the time this runs. If it
    /// fails the statusline says so, and the keyboard is still yours: taking
    /// it back must never depend on the page.
    fn blur(&self) {
        let page = Arc::clone(&self.page);
        let errors = self.errors_tx.clone();
        tokio::spawn(async move {
            if let Err(error) = page.blur().await {
                let _ = errors.send(error.to_string());
            }
        });
    }

    fn run_action(&mut self, action: Action) {
        let page = Arc::clone(&self.page);
        let tx = self.jobs_tx.clone();
        let vp = self.vp;

        match action {
            Action::Scroll(dy) => {
                // Scrolling does not settle the way a navigation does; the
                // page's own scroll listener reports when it has moved.
                tokio::spawn(async move {
                    if let Err(error) = page.scroll_by(dy, vp).await {
                        let _ = tx.send(Job::Failed(error.to_string()));
                    }
                });
            }
            Action::ScrollTop => {
                tokio::spawn(async move {
                    if let Err(error) = page.scroll_to_top().await {
                        let _ = tx.send(Job::Failed(error.to_string()));
                    }
                });
            }
            Action::ScrollEnd => {
                tokio::spawn(async move {
                    if let Err(error) = page.scroll_to_end().await {
                        let _ = tx.send(Job::Failed(error.to_string()));
                    }
                });
            }
            Action::Back => {
                self.navigate_with(move |page| async move { page.back().await.map(|_| ()) })
            }
            Action::Forward => {
                self.navigate_with(move |page| async move { page.forward().await.map(|_| ()) })
            }
            Action::Reload => self.navigate_with(move |page| async move { page.reload().await }),
            // Handled by the caller.
            Action::Quit | Action::EnterCommand(_) | Action::Insert | Action::Hints => {}
        }
    }

    fn run_command(&mut self, command: Command) {
        match command {
            Command::Open(url) => {
                self.url = url.clone();
                self.navigate_with(move |page| async move { page.navigate(&url).await });
            }
            Command::Back => self.run_action(Action::Back),
            Command::Forward => self.run_action(Action::Forward),
            Command::Reload => self.run_action(Action::Reload),
            Command::Set(Setting::Mouse(on)) => {
                self.mouse_pending = Some(on);
                self.state =
                    State::Notice(if on { "mouse on" } else { "mouse off" }.to_string());
            }
            // Handled by the caller.
            Command::Quit => {}
        }
    }

    /// Run something that changes what page we are on, off the loop's thread.
    ///
    /// The previous page stays on screen, marked loading, until the new one
    /// has been extracted. Nothing a page does blanks the frame.
    fn navigate_with<F, Fut>(&mut self, make: F)
    where
        F: FnOnce(Arc<Page>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send,
    {
        if self.navigating {
            return;
        }
        self.navigating = true;
        self.state = State::Loading;

        let page = Arc::clone(&self.page);
        let tx = self.jobs_tx.clone();
        tokio::spawn(async move {
            let job = match make(page).await {
                Ok(()) => Job::Settled,
                Err(error) => Job::Failed(error.to_string()),
            };
            let _ = tx.send(job);
        });
    }

    /// Note that the page has changed under us.
    ///
    /// Hint targets are geometry, so a page that moved has invalidated them.
    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.hints = None;
    }

    fn start_hints(&mut self) {
        let page = Arc::clone(&self.page);
        let tx = self.jobs_tx.clone();
        let errors = self.errors_tx.clone();
        tokio::spawn(async move {
            match page.hints().await {
                Ok(targets) => {
                    let _ = tx.send(Job::Hints(targets));
                }
                // Not a `Job::Failed`: that one clears the extraction and
                // navigation flags, and a failed hint query has finished
                // neither of those.
                Err(error) => {
                    let _ = errors.send(error.to_string());
                }
            }
        });
    }

    fn enter_hints(&mut self, targets: Vec<HintTarget>) {
        let session = HintSession::new(targets);
        if session.is_empty() {
            // Entering a mode with nothing in it would only need escaping.
            self.state = State::Notice("no hints".to_string());
            return;
        }
        self.mode = Mode::Hint(session);
    }

    /// Apply what filtering decided about the character just typed.
    fn on_filtered(&mut self, session: HintSession, filtered: Filtered) {
        match filtered {
            Filtered::Waiting(_) => self.mode = Mode::Hint(session),
            Filtered::Activate(target) => self.activate(target),
            // Nothing matches, so there is nothing left to type. Leaving is
            // friendlier than sitting there waiting for an Esc.
            Filtered::None => self.mode = Mode::Normal,
        }
    }

    fn activate(&mut self, target: HintTarget) {
        let at = target.center();
        self.input.send(Input::Mouse(MouseInput::press(at)));
        self.input.send(Input::Mouse(MouseInput::release(at)));
        // Clicking a text field is the beginning of typing into it, so that
        // is where the mode goes. Anything else is finished when the click
        // lands.
        self.mode = match target.kind {
            TargetKind::Editable => Mode::Insert,
            TargetKind::Clickable => Mode::Normal,
        };
    }

    fn start_extract(&mut self) {
        if self.extracting || !self.dirty {
            return;
        }
        self.extracting = true;
        self.dirty = false;

        let page = Arc::clone(&self.page);
        let tx = self.jobs_tx.clone();
        tokio::spawn(async move {
            let job = match page.extract().await {
                Ok(extraction) => Job::Extracted(Box::new(extraction)),
                Err(error) => Job::Failed(error.to_string()),
            };
            let _ = tx.send(job);
        });
    }

    fn on_job(&mut self, job: Job) {
        match job {
            Job::Extracted(extraction) => {
                self.extracting = false;
                self.progress = extraction.scroll_progress();
                self.runs = extraction.runs;
                self.caret = extraction.caret;
                self.title = extraction.title;

                // Chromium answers a DNS or connection failure by navigating
                // to its own error page rather than failing the command, so a
                // navigation can "succeed" into one. Its error page is more
                // use than a stale frame — it says what went wrong — but the
                // statusline must not go on claiming the page is fine.
                if extraction.url.starts_with(CHROME_ERROR_SCHEME) {
                    // The statusline prints the URL itself, so naming it here
                    // too would print it twice.
                    self.state = State::Error("could not be reached".to_string());
                } else {
                    self.url = extraction.url;
                    if !self.navigating {
                        self.state = State::Ready;
                    }
                }
                // The page may have changed again while we were extracting.
                self.start_extract();
            }
            Job::Hints(targets) => {
                self.hints = Some(targets.clone());
                self.enter_hints(targets);
            }
            Job::Settled => {
                self.navigating = false;
                self.state = State::Ready;
                self.mark_dirty();
                self.start_extract();
            }
            // The frame stays exactly as it was; only the statusline
            // changes. Spec section 8. Deliberately not `Job::Failed`: that
            // one clears the extraction and navigation flags, and a
            // keystroke that failed has finished neither of those.
            Job::InputFailed(message) => self.state = State::Error(message),
            Job::Failed(message) => {
                self.extracting = false;
                self.navigating = false;
                // The frame stays exactly as it was; only the statusline
                // changes. Section 8: never blank the frame you are looking at.
                self.state = State::Error(message);
            }
        }
    }

    async fn on_resize(&mut self) -> Result<()> {
        let (grid, cell) = wwt_term::probe().context("re-measure the terminal")?;
        if grid == self.grid && cell == self.cell {
            return Ok(());
        }

        self.grid = grid;
        self.cell = cell;
        self.vp = page_viewport(grid, cell);

        // The page genuinely reflows: it is being told the window changed size.
        self.page
            .set_viewport(self.vp)
            .await
            .context("resize the page viewport")?;

        // A diff against a frame of different dimensions is meaningless.
        self.renderer.invalidate();
        self.mark_dirty();
        self.start_extract();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_viewport_is_one_row_shorter_than_the_terminal() {
        let vp = page_viewport(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
        assert_eq!(vp.grid(), GridSize { cols: 80, rows: 23 });
        assert_eq!(vp.css_height(), 23 * 20);
    }

    #[test]
    fn a_one_row_terminal_still_leaves_a_page_row() {
        let vp = page_viewport(GridSize { cols: 80, rows: 1 }, CellSize { w: 9, h: 20 });
        assert_eq!(vp.grid().rows, 1, "never zero, or Chromium gets a zero-height window");
    }
    #[test]
    fn a_click_on_the_page_keeps_its_cell() {
        let vp = page_viewport(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
        assert_eq!(page_cell(&vp, 5, 7), Some(CellPos { col: 5, row: 7 }));
    }

    #[test]
    fn a_click_on_the_chrome_row_belongs_to_no_page_cell() {
        // Row 23 is the statusline. The page does not know that row exists,
        // so there is nothing to convert a click there into.
        let vp = page_viewport(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
        assert_eq!(page_cell(&vp, 5, 23), None);
    }

    #[test]
    fn the_caret_lands_on_the_row_its_line_is_on() {
        let vp = page_viewport(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
        // A caret on the line starting at y 40, one line high: row 2.
        let caret = CssRect { x: 90.0, y: 40.0, w: 0.0, h: 20.0 };
        assert_eq!(caret_cell(&vp, &caret), Some(CellPos { col: 10, row: 2 }));
    }

    #[test]
    fn a_caret_scrolled_off_the_page_has_no_cell() {
        let vp = page_viewport(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
        let caret = CssRect { x: 90.0, y: -100.0, w: 0.0, h: 20.0 };
        assert_eq!(caret_cell(&vp, &caret), None, "nothing above the viewport has a cell");
    }

}
