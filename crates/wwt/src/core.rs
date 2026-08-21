//! The event loop: the adapter between tokio and the session.
//!
//! Nothing here decides anything. It turns what arrives on the `select!`
//! into a [`Event`], hands it to the [`Session`], and does what the session
//! asks for. Every rule about what a key means or when an extraction may
//! start lives on the other side of that seam, where it can be tested
//! without a browser.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use anyhow::{Context, Result};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event as TermEvent, EventStream, KeyEventKind,
};
use crossterm::execute;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, sleep_until};
use wwt_cdp::Client;
use wwt_frame::{CellSize, GridSize, Viewport};
use wwt_page::Page;
use wwt_term::Renderer;

use crate::effect::{Effect, Navigation, Scroll};
use crate::event::{Event, Job};
use crate::input::InputPump;
use crate::session::Session;
use crate::tab::TabId;

/// A dragged window edge produces a resize event per frame, and each one
/// would otherwise cost a Chromium relayout and a full extraction.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(100);

pub struct Core {
    pages: HashMap<TabId, Arc<Page>>,
    client: Arc<Client>,
    renderer: Renderer,
    session: Session,

    jobs_tx: mpsc::UnboundedSender<Job>,
    jobs_rx: mpsc::UnboundedReceiver<Job>,

    /// Ordered delivery of keys and clicks, across every page.
    input: InputPump,
}

impl Core {
    pub fn new(page: Arc<Page>, client: Arc<Client>, grid: GridSize, cell: CellSize) -> Self {
        let (jobs_tx, jobs_rx) = mpsc::unbounded_channel();
        let input = InputPump::spawn(jobs_tx.clone());
        let session = Session::new(grid, cell);
        let pages = HashMap::from([(session.focused_id(), page)]);

        Self {
            pages,
            client,
            renderer: Renderer::new(),
            session,
            jobs_tx,
            jobs_rx,
            input,
        }
    }

    /// Say something in the statusline before the loop starts.
    pub fn notice(&mut self, message: &str) {
        self.session.notice(message);
    }

    pub async fn run(&mut self, out: &mut impl Write) -> Result<()> {
        let mut terminal = EventStream::new();
        let mut cdp = self.client.subscribe();
        let mut resize_at: Option<Instant> = None;

        let effects = self.session.begin();
        if self.apply(effects, out)? {
            return Ok(());
        }
        self.present(out)?;

        loop {
            // Every arm produces an event and nothing else. Touching
            // `self` inside one would borrow it while the other futures are
            // still alive, which is what used to force a whole spawned task
            // to merge two channels into one.
            let event = tokio::select! {
                Some(Ok(event)) = terminal.next() => match event {
                    TermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                        Some(Event::Key(key))
                    }
                    TermEvent::Mouse(mouse) => Some(Event::Mouse(mouse)),
                    TermEvent::Resize(..) => {
                        resize_at = Some(Instant::now() + RESIZE_DEBOUNCE);
                        None
                    }
                    _ => None,
                },

                Some(event) = cdp.recv() => self
                    .pages
                    .iter()
                    .find(|(_, page)| page.is_dirty(&event))
                    .map(|(id, _)| Event::Dirty(*id)),

                Some(job) = self.jobs_rx.recv() => Some(Event::Done(job)),

                () = async { sleep_until(resize_at.expect("guarded")).await },
                    if resize_at.is_some() =>
                {
                    resize_at = None;
                    let (grid, cell) = wwt_term::probe().context("re-measure the terminal")?;
                    Some(Event::Resized(grid, cell))
                }
            };

            // Only what the session saw can have changed what it looks
            // like. An arm that produced no event — a key release, a CDP
            // message that was not our dirty signal, the first edge of a
            // resize — left every field untouched, so composing again would
            // build the same frame and diff it against itself. A page that
            // chatters on the console would pay for a repaint per line.
            if let Some(event) = event {
                let effects = self.session.on(event);
                if self.apply(effects, out)? {
                    return Ok(());
                }
                self.present(out)?;
            }
        }
    }

    /// Do what the session asked for. `true` means it is time to quit.
    fn apply(&mut self, effects: Vec<Effect>, out: &mut impl Write) -> Result<bool> {
        for effect in effects {
            match effect {
                Effect::Quit => return Ok(true),

                Effect::MouseCapture(on) => {
                    if on {
                        execute!(out, EnableMouseCapture).context("enable mouse capture")?;
                    } else {
                        execute!(out, DisableMouseCapture).context("disable mouse capture")?;
                    }
                }

                Effect::Send(id, input) => {
                    if let Some(page) = self.pages.get(&id) {
                        self.input.send(Arc::clone(page), input);
                    }
                }

                Effect::Extract(id) => self.spawn(id, move |page| async move {
                    Some(match page.extract().await {
                        Ok(extraction) => Job::Extracted(id, Box::new(extraction)),
                        Err(error) => Job::Failed(id, error.to_string()),
                    })
                }),

                // Always a `Job::Hints`, however it went. A failure is not a
                // `Job::Failed`: that one clears the extraction and
                // navigation flags, and a hint query has finished neither of
                // those. Nor can it go unreported, or the session would
                // believe a query was still in flight and `f` would be dead
                // for the rest of the run.
                Effect::Hints(id) => self.spawn(id, move |page| async move {
                    Some(Job::Hints(
                        id,
                        page.hints().await.map_err(|e| e.to_string()),
                    ))
                }),

                Effect::Blur(id) => self.spawn(id, |page| async move {
                    page.blur().await.err().map(|e| Job::Noted(e.to_string()))
                }),

                Effect::Scroll(id, scroll) => {
                    let vp = self.session.viewport();
                    self.spawn(id, move |page| async move {
                        let done = match scroll {
                            Scroll::By(dy) => page.scroll_by(dy, vp).await,
                            Scroll::Top => page.scroll_to_top().await,
                            Scroll::End => page.scroll_to_end().await,
                        };
                        done.err().map(|e| Job::Failed(id, e.to_string()))
                    });
                }

                Effect::Navigate(id, navigation) => self.spawn(id, move |page| async move {
                    let done = match navigation {
                        Navigation::Open(url) => page.navigate(&url).await,
                        Navigation::Back => page.back().await.map(|_| ()),
                        Navigation::Forward => page.forward().await.map(|_| ()),
                        Navigation::Reload => page.reload().await,
                    };
                    Some(match done {
                        Ok(()) => Job::Settled(id),
                        Err(error) => Job::Failed(id, error.to_string()),
                    })
                }),

                Effect::SetViewport(id, vp) => {
                    // A diff against a frame of different dimensions is
                    // meaningless.
                    self.renderer.invalidate();
                    self.resize_page(id, vp);
                }
            }
        }
        Ok(false)
    }

    /// Tell one page how big its window is now.
    fn resize_page(&self, id: TabId, vp: Viewport) {
        self.spawn(id, move |page| async move {
            Some(match page.set_viewport(vp).await {
                Ok(()) => Job::Resized(id),
                Err(error) => Job::Failed(id, error.to_string()),
            })
        });
    }

    /// Run one page operation off the loop's thread and report what it did.
    ///
    /// The one place anything is spawned. A thirty-second load still leaves
    /// keys responsive because nothing here is awaited by the loop, and each
    /// operation says for itself what its failure means by choosing the
    /// `Job` it reports, or reporting none.
    ///
    /// An effect naming a page we do not hold is dropped. That is reachable
    /// only between asking for a tab and being told it opened, where the tab
    /// is marked loading and nothing could have expected to land.
    fn spawn<F, Fut>(&self, id: TabId, make: F)
    where
        F: FnOnce(Arc<Page>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Option<Job>> + Send,
    {
        let Some(page) = self.pages.get(&id).map(Arc::clone) else {
            return;
        };
        let tx = self.jobs_tx.clone();
        tokio::spawn(async move {
            if let Some(job) = make(page).await {
                let _ = tx.send(job);
            }
        });
    }

    fn present(&mut self, out: &mut impl Write) -> Result<()> {
        let frame = self.session.compose();
        self.renderer.render(&frame, out).context("write the frame")?;
        out.flush().context("flush the terminal")?;
        Ok(())
    }
}
