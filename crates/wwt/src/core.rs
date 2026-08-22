//! The event loop: the adapter between tokio and the session.
//!
//! Nothing here decides anything. It turns what arrives on the `select!`
//! into a [`Event`], hands it to the [`Session`], and does what the session
//! asks for. Every rule about what a key means or when an extraction may
//! start lives on the other side of that seam, where it can be tested
//! without a browser.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event as TermEvent, EventStream, KeyEventKind,
};
use crossterm::execute;
use futures_util::StreamExt;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, sleep_until};
use wwt_cdp::{Client, TargetId};
use wwt_frame::{CellSize, GridSize, Viewport};
use wwt_page::Page;
use wwt_term::Renderer;

use crate::effect::{Effect, Navigation, Scroll};
use crate::event::{Event, Job};
use crate::input::InputPump;
use crate::session::Session;
use crate::store::Snapshot;
use crate::tab::TabId;

/// A dragged window edge produces a resize event per frame, and each one
/// would otherwise cost a Chromium relayout and a full extraction.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(100);

/// A held `j` produces a scroll and an extraction per frame, and every one of
/// them changes the scroll offset a restart would come back to. Writing each
/// would be a syscall per frame for a file nobody reads until the next launch.
const SAVE_DEBOUNCE: Duration = Duration::from_secs(1);

/// What arrives on the loop's one result channel.
///
/// Most of it is a `Job` on its way to the session unchanged. A target that
/// finished opening is not: the `Page` it produced belongs to `Core`, and the
/// session must never hold one, so the page is filed here and the session
/// hears only that the tab opened.
#[derive(Debug)]
enum Finished {
    Job(Job),
    Opened(TabId, Result<Arc<Page>, String>),
}

impl From<Job> for Finished {
    fn from(job: Job) -> Self {
        Finished::Job(job)
    }
}

/// What one turn of the loop picked up. An arm produces one of these and
/// touches nothing, because borrowing `self` in one while the other futures
/// are alive is what used to force a whole spawned task to merge two
/// channels into one.
enum Incoming {
    Event(Event),
    Finished(Finished),
}

pub struct Core {
    pages: HashMap<TabId, Arc<Page>>,
    /// Targets attached to us but not yet made into pages.
    ///
    /// Only adoption writes here. A tab we opened ourselves and failed to
    /// open has no target to answer for; one the browser handed us exists
    /// whether we manage to prepare it or not, and closing it needs its id
    /// after the `Page` that would have carried it is gone.
    opening: HashMap<TabId, TargetId>,
    client: Arc<Client>,
    renderer: Renderer,
    session: Session,

    jobs_tx: mpsc::UnboundedSender<Finished>,
    jobs_rx: mpsc::UnboundedReceiver<Finished>,

    /// Ordered delivery of keys and clicks, across every page.
    input: InputPump,

    /// Where the session file goes, or `None` when this instance does not own
    /// it. A private session, on a profile another instance holds, writes
    /// nothing.
    session_file: Option<PathBuf>,
    /// The most recent snapshot not yet written.
    pending: Option<Snapshot>,
}

/// What the browser starts as.
pub struct Startup {
    pub grid: GridSize,
    pub cell: CellSize,
    pub snapshot: Option<Snapshot>,
    /// A URL from the command line, opened beside whatever was restored.
    pub open: Option<String>,
    /// Where the session file goes, or `None` when this instance does not
    /// own it.
    pub session_file: Option<PathBuf>,
    /// Whether the terminal answered that it can show a picture. Asked once,
    /// before raw mode, because after the input pump exists a reply would be
    /// a keystroke.
    pub graphics: bool,
}

impl Core {
    /// A browser with no pages in it yet. Every tab, including the first,
    /// comes into being through `Effect::OpenTab`, so there is one path from
    /// a url to a page rather than one for the first tab and one for the
    /// rest.
    pub fn new(client: Arc<Client>, startup: Startup) -> Self {
        let (jobs_tx, jobs_rx) = mpsc::unbounded_channel();
        let input = InputPump::spawn(jobs_tx.clone());
        let mut session =
            Session::restore(startup.grid, startup.cell, startup.snapshot, startup.open);
        session.set_graphics(startup.graphics);

        Self {
            pages: HashMap::new(),
            opening: HashMap::new(),
            client,
            renderer: Renderer::new(),
            session,
            jobs_tx,
            jobs_rx,
            input,
            session_file: startup.session_file,
            pending: None,
        }
    }

    /// Write the pending snapshot, if there is one and it is ours to write.
    ///
    /// `spawn_blocking` rather than the loop's own thread: it is a small file
    /// and a rename, but the loop's promise is that nothing in it waits on a
    /// syscall.
    fn flush_save(&mut self) {
        let (Some(path), Some(snapshot)) = (self.session_file.clone(), self.pending.take()) else {
            return;
        };
        let tx = self.jobs_tx.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(error) = crate::store::save(&path, &snapshot) {
                let _ = tx.send(Finished::Job(Job::Unsaved(error)));
            }
        });
    }

    /// Say something in the statusline before the loop starts.
    pub fn notice(&mut self, message: &str) {
        self.session.notice(message);
    }

    pub async fn run(&mut self, out: &mut impl Write) -> Result<()> {
        let mut terminal = EventStream::new();
        let mut cdp = self.client.subscribe();
        let mut resize_at: Option<Instant> = None;
        let mut save_at: Option<Instant> = None;

        let effects = self.session.begin();
        if self.apply(effects, &mut save_at, out)? {
            return Ok(());
        }
        self.present(out)?;

        loop {
            let mut due_to_save = false;

            // Every arm produces an event and nothing else. Touching
            // `self` inside one would borrow it while the other futures are
            // still alive, which is what used to force a whole spawned task
            // to merge two channels into one.
            let incoming = tokio::select! {
                Some(Ok(event)) = terminal.next() => match event {
                    TermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                        Some(Incoming::Event(Event::Key(key)))
                    }
                    TermEvent::Mouse(mouse) => Some(Incoming::Event(Event::Mouse(mouse))),
                    TermEvent::Resize(..) => {
                        resize_at = Some(Instant::now() + RESIZE_DEBOUNCE);
                        None
                    }
                    _ => None,
                },

                // Two questions of one event, in this order: a target we
                // never asked for belongs to no page yet, so asking the pages
                // about it first would only ever answer no.
                // Three questions of one event, cheapest first. A target we
                // never asked for belongs to no page yet, so asking the
                // pages about it first would only ever answer no. Then the
                // method name, which is one string compare, before
                // iterating every page to ask whose picture this is: in
                // pixel mode a frame is much the most frequent event there
                // is, and every other event would pay for that walk.
                Some(event) = cdp.recv() => match Client::opened_by_a_page(&event) {
                    Some(attached) => Some(Incoming::Event(Event::TargetOpened(attached))),
                    None if event.method == wwt_page::SCREENCAST_FRAME => self
                        .pages
                        .iter()
                        .find_map(|(id, page)| {
                            page.screencast_frame(&event)
                                .map(|frame| Incoming::Event(Event::Frame(*id, Box::new(frame))))
                        }),
                    None => self
                        .pages
                        .iter()
                        .find(|(_, page)| page.is_dirty(&event))
                        .map(|(id, _)| Incoming::Event(Event::Dirty(*id))),
                },

                Some(finished) = self.jobs_rx.recv() => Some(Incoming::Finished(finished)),

                () = async { sleep_until(resize_at.expect("guarded")).await },
                    if resize_at.is_some() =>
                {
                    resize_at = None;
                    let (grid, cell) = wwt_term::probe().context("re-measure the terminal")?;
                    Some(Incoming::Event(Event::Resized(grid, cell)))
                }

                // No event on purpose: a write changes nothing about what is
                // on screen, and composing again would build the same frame
                // and diff it against itself. The flag rather than the write
                // itself, because an arm that borrowed `self` would borrow it
                // while the other futures are still alive.
                () = async { sleep_until(save_at.expect("guarded")).await },
                    if save_at.is_some() =>
                {
                    save_at = None;
                    due_to_save = true;
                    None
                }
            };

            if due_to_save {
                self.flush_save();
            }

            // A page is `Core`'s. This is where one is filed, because it is
            // the first point in the turn that can borrow `self` mutably.
            let event = match incoming {
                Some(Incoming::Event(event)) => Some(event),
                Some(Incoming::Finished(Finished::Job(job))) => Some(Event::Done(job)),
                Some(Incoming::Finished(Finished::Opened(id, Ok(page)))) => {
                    self.opening.remove(&id);
                    self.pages.insert(id, page);
                    Some(Event::Done(Job::Opened(id, Ok(()))))
                }
                Some(Incoming::Finished(Finished::Opened(id, Err(error)))) => {
                    // The session is about to drop the tab. A target the
                    // browser handed us outlives that: left alone it is a
                    // page loading somewhere nobody can see and nothing can
                    // reach. There is no `Page` to close it with, so it is
                    // closed by id, and a failure to close is not worth a
                    // second notice on top of the first.
                    if let Some(target) = self.opening.remove(&id) {
                        let client = Arc::clone(&self.client);
                        tokio::spawn(async move {
                            let _ = client
                                .call("Target.closeTarget", json!({ "targetId": target.0 }))
                                .await;
                        });
                    }
                    Some(Event::Done(Job::Opened(id, Err(error))))
                }
                None => None,
            };

            // Only what the session saw can have changed what it looks
            // like. An arm that produced no event — a key release, a CDP
            // message that was not our dirty signal, the first edge of a
            // resize — left every field untouched, so composing again would
            // build the same frame and diff it against itself. A page that
            // chatters on the console would pay for a repaint per line.
            if let Some(event) = event {
                let effects = self.session.on(event);
                if self.apply(effects, &mut save_at, out)? {
                    return Ok(());
                }
                self.present(out)?;
            }
        }
    }

    /// Do what the session asked for. `true` means it is time to quit.
    fn apply(
        &mut self,
        effects: Vec<Effect>,
        save_at: &mut Option<Instant>,
        out: &mut impl Write,
    ) -> Result<bool> {
        for effect in effects {
            match effect {
                Effect::Quit => {
                    // The last second of browsing is exactly the part you
                    // would notice missing.
                    self.flush_save();
                    return Ok(true);
                }

                Effect::Save(snapshot) => {
                    self.pending = Some(snapshot);
                    *save_at = Some(Instant::now() + SAVE_DEBOUNCE);
                }

                Effect::MouseCapture(on) => {
                    if on {
                        execute!(out, EnableMouseCapture).context("enable mouse capture")?;
                    } else {
                        execute!(out, DisableMouseCapture).context("disable mouse capture")?;
                    }
                }

                Effect::Send(id, input) => {
                    if let Some(page) = self.pages.get(&id) {
                        self.input.send(id, Arc::clone(page), input);
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

                Effect::Blur(id) => self.spawn(id, move |page| async move {
                    page.blur().await.err().map(|e| Job::Noted(id, e.to_string()))
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

                Effect::OpenTab { id, url, scroll_y } => {
                    let vp = self.session.viewport();
                    let client = Arc::clone(&self.client);
                    let tx = self.jobs_tx.clone();
                    tokio::spawn(async move {
                        let opened = match Page::open(client, &url, vp).await {
                            // Before it is reported open, so the first
                            // extraction reads the page where it was left
                            // rather than racing the scroll to it. A page
                            // that will not scroll there is still a page.
                            Ok(page) => {
                                if scroll_y > 0.0 {
                                    let _ = page.scroll_to(scroll_y).await;
                                }
                                Ok(Arc::new(page))
                            }
                            Err(error) => Err(error.to_string()),
                        };
                        let _ = tx.send(Finished::Opened(id, opened));
                    });
                }

                Effect::AdoptTab { id, target } => {
                    let vp = self.session.viewport();
                    let client = Arc::clone(&self.client);
                    let tx = self.jobs_tx.clone();
                    // Noted before the spawn, so a preparation that fails
                    // leaves something to close the target by. A tab we
                    // created and could not open has no target to close.
                    self.opening.insert(id, target.target.clone());
                    tokio::spawn(async move {
                        let opened = Page::adopt(client, target, vp)
                            .await
                            .map(Arc::new)
                            .map_err(|error| error.to_string());
                        let _ = tx.send(Finished::Opened(id, opened));
                    });
                }

                Effect::CloseTab(id) => {
                    // Taken out of the map first: whatever happens to the
                    // target, nothing may still be sent to a tab the session
                    // has already let go of.
                    if let Some(page) = self.pages.remove(&id) {
                        let tx = self.jobs_tx.clone();
                        tokio::spawn(async move {
                            if let Err(error) = page.close().await {
                                let _ = tx.send(Finished::Job(Job::Noted(id, error.to_string())));
                            }
                        });
                    }
                }

                Effect::Activate(id) => self.spawn(id, move |page| async move {
                    page.activate()
                        .await
                        .err()
                        .map(|e| Job::Noted(id, e.to_string()))
                }),

                Effect::StartScreencast(id) => {
                    let vp = self.session.viewport();
                    self.spawn(id, move |page| async move {
                        // A screencast that will not start is worth saying
                        // out loud: the mode is on and no picture is coming.
                        page.start_screencast(vp)
                            .await
                            .err()
                            .map(|e| Job::Noted(id, e.to_string()))
                    });
                }

                Effect::StopScreencast(id) => self.spawn(id, move |page| async move {
                    // Failing to stop is not worth a word. The mode is
                    // already off, the image is already deleted, and any
                    // frames that keep arriving are acked and dropped.
                    let _ = page.stop_screencast().await;
                    None
                }),

                Effect::AckFrame(id, ack) => self.spawn(id, move |page| async move {
                    // A failed ack stops the screencast, which shows up as a
                    // picture that stopped moving rather than as an error,
                    // so it is worth naming.
                    page.ack_frame(ack)
                        .await
                        .err()
                        .map(|e| Job::Noted(id, e.to_string()))
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
    /// only between asking for a tab and being told it opened.
    ///
    /// Dropped silently, so the session must not set an in-flight flag
    /// beside an effect it emits in that window: the flag is cleared by the
    /// answer, and there will not be one. `Tab::opened` is what names the
    /// window on that side.
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
                let _ = tx.send(Finished::Job(job));
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
