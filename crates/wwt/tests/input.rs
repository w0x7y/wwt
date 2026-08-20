use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use wwt::input::InputPump;
use wwt_cdp::{Chromium, Client};
use wwt_frame::{CellSize, GridSize, Viewport};
use wwt_page::{Input, KeyInput, Page};

fn fixture_url(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    format!("file://{}", path.display())
}

fn letter(c: char) -> KeyInput {
    KeyInput {
        key: c.to_string(),
        code: format!("Key{}", c.to_ascii_uppercase()),
        windows_virtual_key_code: c.to_ascii_uppercase() as u32,
        text: c.to_string(),
        modifiers: 0,
    }
}

/// The space bar comes off a key of its own, not `Key `.
fn space() -> KeyInput {
    KeyInput {
        key: " ".to_string(),
        code: "Space".to_string(),
        windows_virtual_key_code: 32,
        text: " ".to_string(),
        modifiers: 0,
    }
}

/// Typing is the first page operation whose order matters. Sending a burst
/// without awaiting anything is exactly what the core loop does, so this is
/// the shape of the bug it would otherwise have.
#[tokio::test]
async fn a_burst_of_keys_arrives_in_the_order_it_was_typed() {
    let browser = Chromium::launch().await.expect("launch chromium");
    let client = Arc::new(Client::connect(browser.ws_url()).await.expect("connect"));
    let vp = Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
    let page = Arc::new(
        Page::open(Arc::clone(&client), &fixture_url("form.html"), vp)
            .await
            .expect("open the fixture"),
    );
    page.eval("document.querySelector('#name').focus()").await.expect("focus");

    let (jobs_tx, mut jobs_rx) = mpsc::unbounded_channel();
    let pump = InputPump::spawn(Arc::clone(&page), jobs_tx);

    let typed = "the quick brown fox";
    for c in typed.chars() {
        // No await between sends: the pump is what keeps these in order.
        pump.send(Input::Key(if c == ' ' { space() } else { letter(c) }));
    }

    // What the field holds is read the way the browser reads it, so an
    // ordering bug that somehow left the DOM right and the frame wrong is
    // still a failure here.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut shown = String::new();
    while Instant::now() < deadline {
        let extraction = page.extract().await.expect("extract");
        shown = extraction
            .runs
            .iter()
            .map(|run| run.text.as_str())
            .find(|text| text.len() == typed.len())
            .unwrap_or_default()
            .to_string();
        if shown == typed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert_eq!(shown, typed);
    assert!(jobs_rx.try_recv().is_err(), "the pump reported a failure");
}
