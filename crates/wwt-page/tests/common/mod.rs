//! One browser for the whole file, one test at a time.
//!
//! What varies between these tests is the fixture, not the process, and a
//! Chromium per test is why `cargo test --workspace` used to launch 43 of
//! them. Sharing one needs a runtime that outlives any single test, so these
//! tests are `#[test]` over a shared runtime rather than `#[tokio::test]`
//! over one apiece.
//!
//! Two things make that safe:
//!
//! **One at a time.** `Input.dispatchMouseEvent` is answered by the target
//! the browser has in front. Two tests driving two pages at once is two
//! tests fighting over which that is, and the loser's wheel never comes
//! back — a thirty-second timeout rather than a wrong answer. `harness()`
//! hands out a turn along with the browser, so a test cannot forget.
//!
//! **Nothing is left running.** The browser is handed out as an `Arc` with
//! only a `Weak` kept behind, so it lives exactly as long as some test is
//! holding it and is killed by the last one to let go. A `static` holding it
//! outright would leak a Chromium and a profile directory per run, because
//! nothing drops a static. Taking the browser *before* queueing for a turn
//! is what keeps it alive across the queue: every waiting test is already
//! holding one.

// Each test binary compiles this module separately, so anything only one of
// them uses looks dead to the other.
#![allow(dead_code)]

use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

use tokio::runtime::Runtime;
use wwt_cdp::{Chromium, Client};
use wwt_frame::{CellSize, GridSize, Viewport};
use wwt_page::Page;

pub struct Harness {
    _browser: Chromium,
    pub client: Arc<Client>,
}

/// A browser, and the right to be the only test using it.
pub struct Turn {
    harness: Arc<Harness>,
    _turn: MutexGuard<'static, ()>,
}

impl Deref for Turn {
    type Target = Harness;

    fn deref(&self) -> &Harness {
        &self.harness
    }
}

/// The runtime every test in this file runs on.
///
/// Never dropped, which is what lets the websocket task behind `Client`
/// outlive the test that first connected it.
pub fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("build a runtime"))
}

/// The shared browser, launching one if nothing is holding it.
///
/// Call this from the test body proper, never from inside `block_on`:
/// launching blocks on the runtime, and a runtime cannot be entered from
/// within itself.
pub fn harness() -> Turn {
    static SHARED: Mutex<Weak<Harness>> = Mutex::new(Weak::new());
    static TURN: Mutex<()> = Mutex::new(());

    let harness = {
        let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        match shared.upgrade() {
            Some(harness) => harness,
            None => {
                let harness = Arc::new(runtime().block_on(async {
                    let browser = Chromium::launch(None, None).await.expect("launch chromium");
                    let client = Client::connect(browser.ws_url()).await.expect("connect");
                    // `Page::open` takes its session from auto-attach rather
                    // than attaching for itself, so a client without this
                    // opens nothing at all.
                    client.auto_attach().await.expect("turn on auto-attach");
                    Harness { _browser: browser, client: Arc::new(client) }
                }));
                *shared = Arc::downgrade(&harness);
                harness
            }
        }
    };

    // A test that panicked poisoned this. It still had its turn, and the
    // next test is still entitled to one.
    let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    Turn { harness, _turn: turn }
}

pub fn fixture_url(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    format!("file://{}", path.display())
}

pub fn viewport() -> Viewport {
    Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 })
}

pub async fn open(h: &Harness, fixture: &str) -> Page {
    Page::open(Arc::clone(&h.client), &fixture_url(fixture), viewport())
        .await
        .expect("open the fixture")
}

/// Open a page at an arbitrary URL rather than a fixture.
///
/// `about:blank` and a second copy of a fixture are both things a tab test
/// wants and a fixture name cannot say.
pub async fn open_url(h: &Harness, url: &str) -> Page {
    Page::open(Arc::clone(&h.client), url, viewport())
        .await
        .expect("open the url")
}
