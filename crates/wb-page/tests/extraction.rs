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
    let runs = open(&h, "simple.html").await.extract().await.expect("extract");

    let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
    assert!(texts.contains(&"Heading"), "runs were {texts:?}");
    assert!(texts.contains(&"First paragraph."), "runs were {texts:?}");
}

#[tokio::test]
async fn skips_hidden_text() {
    let h = harness().await;
    let runs = open(&h, "simple.html").await.extract().await.expect("extract");

    let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
    assert!(
        !texts.contains(&"Invisible text."),
        "visibility:hidden text must not be extracted: {texts:?}"
    );
}

#[tokio::test]
async fn carries_color_and_weight_through() {
    let h = harness().await;
    let runs = open(&h, "simple.html").await.extract().await.expect("extract");

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
    let runs = open(&h, "simple.html").await.extract().await.expect("extract");

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
    assert_eq!(page.title().await.expect("title"), "Fixture Page");
}

#[tokio::test]
async fn lays_the_page_out_at_the_viewport_we_asked_for() {
    let h = harness().await;
    let runs = open(&h, "simple.html").await.extract().await.expect("extract");

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
