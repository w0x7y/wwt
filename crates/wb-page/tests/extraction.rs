use std::sync::Arc;

use wb_cdp::{Chromium, Client};
use wb_frame::{CellSize, GridSize, Viewport};
use wb_page::Page;

fn fixture_url(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    format!("file://{}", path.display())
}

fn viewport() -> Viewport {
    Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 })
}

/// Owns the browser and the connection, so every test in this file shares one
/// Chromium rather than launching its own.
struct Harness {
    _browser: Chromium,
    client: Arc<Client>,
}

async fn harness() -> Harness {
    let browser = Chromium::launch().await.expect("launch chromium");
    let client = Client::connect(browser.ws_url()).await.expect("connect");
    Harness {
        _browser: browser,
        client: Arc::new(client),
    }
}

async fn open(h: &Harness, fixture: &str) -> Page {
    Page::open(Arc::clone(&h.client), &fixture_url(fixture), viewport())
        .await
        .expect("open the fixture")
}

#[tokio::test]
async fn extracts_the_visible_text_of_a_page() {
    let h = harness().await;
    let runs = open(&h, "simple.html").await.extract().await.expect("extract").runs;

    let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
    assert!(texts.contains(&"Heading"), "runs were {texts:?}");
    assert!(texts.contains(&"First paragraph."), "runs were {texts:?}");
}

#[tokio::test]
async fn skips_hidden_text() {
    let h = harness().await;
    let runs = open(&h, "simple.html").await.extract().await.expect("extract").runs;

    let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
    assert!(
        !texts.contains(&"Invisible text."),
        "visibility:hidden text must not be extracted: {texts:?}"
    );
}

#[tokio::test]
async fn carries_color_and_weight_through() {
    let h = harness().await;
    let runs = open(&h, "simple.html").await.extract().await.expect("extract").runs;

    let heading = runs.iter().find(|r| r.text == "Heading").expect("heading run");
    assert_eq!(heading.style.fg, wb_frame::Rgb { r: 255, g: 0, b: 0 });
    assert!(heading.style.bold, "font-weight 700 is bold");

    let para = runs
        .iter()
        .find(|r| r.text == "First paragraph.")
        .expect("paragraph run");
    assert_eq!(para.style.fg, wb_frame::Rgb { r: 0, g: 0, b: 255 });
    assert!(!para.style.bold);
}

#[tokio::test]
async fn orders_runs_down_the_page() {
    let h = harness().await;
    let runs = open(&h, "simple.html").await.extract().await.expect("extract").runs;

    let heading = runs.iter().find(|r| r.text == "Heading").expect("heading");
    let para = runs
        .iter()
        .find(|r| r.text == "First paragraph.")
        .expect("paragraph");
    assert!(
        heading.baseline < para.baseline,
        "the heading sits above the paragraph"
    );
}

#[tokio::test]
async fn reads_the_document_title() {
    let h = harness().await;
    let page = open(&h, "simple.html").await;
    assert_eq!(page.extract().await.expect("extract").title, "Fixture Page");
}

#[tokio::test]
async fn lays_the_page_out_at_the_viewport_we_asked_for() {
    let h = harness().await;
    let runs = open(&h, "simple.html").await.extract().await.expect("extract").runs;

    // The viewport is 80 * 9 = 720 CSS px wide; nothing may be laid out
    // beyond it, which is how we know setDeviceMetricsOverride took effect.
    for run in &runs {
        assert!(
            run.rect.x < 720.0,
            "run {:?} starts outside the 720px viewport",
            run.text
        );
    }
}

#[tokio::test]
async fn a_dom_mutation_signals_dirtiness() {
    let h = harness().await;
    let mut events = h.client.subscribe();
    let page = open(&h, "mutating.html").await;

    // The fixture mutates itself 100ms after load, so the signal arrives
    // after we are already subscribed and watching.
    let signalled = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            if event.method == "Runtime.bindingCalled"
                && event.params["name"] == wb_page::DIRTY_BINDING
                && event.session_id.as_deref() == Some(page.session_id())
            {
                return true;
            }
        }
        false
    })
    .await
    .expect("the dirty binding should fire within ten seconds");

    assert!(signalled, "the CDP connection closed before the binding fired");
}

#[tokio::test]
async fn extraction_reports_scroll_geometry() {
    let h = harness().await;
    let extraction = open(&h, "simple.html").await.extract().await.expect("extract");

    assert_eq!(extraction.scroll_y, 0.0);
    assert!(extraction.viewport_height > 0.0);
    assert!(extraction.url.ends_with("simple.html"), "url was {}", extraction.url);
    assert_eq!(extraction.title, "Fixture Page");
}

#[test]
fn scroll_progress_is_zero_when_the_document_fits() {
    let e = wb_page::Extraction {
        runs: Vec::new(),
        title: String::new(),
        url: String::new(),
        scroll_y: 0.0,
        scroll_height: 400.0,
        viewport_height: 400.0,
    };
    assert_eq!(e.scroll_progress(), 0.0);
}

#[test]
fn scroll_progress_is_one_at_the_bottom() {
    let e = wb_page::Extraction {
        runs: Vec::new(),
        title: String::new(),
        url: String::new(),
        scroll_y: 600.0,
        scroll_height: 1000.0,
        viewport_height: 400.0,
    };
    assert_eq!(e.scroll_progress(), 1.0);
}
