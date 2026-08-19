use std::sync::Arc;
use std::time::{Duration, Instant};

use wwt_cdp::{Chromium, Client};
use wwt_frame::{CellSize, CssPoint, GridSize, Viewport};
use wwt_page::{KeyInput, MouseInput, Page};

fn fixture_url(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    format!("file://{}", path.display())
}

fn viewport() -> Viewport {
    Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 })
}

struct Harness {
    _browser: Chromium,
    client: Arc<Client>,
}

async fn harness() -> Harness {
    let browser = Chromium::launch().await.expect("launch chromium");
    let client = Client::connect(browser.ws_url()).await.expect("connect");
    Harness { _browser: browser, client: Arc::new(client) }
}

async fn open(h: &Harness, fixture: &str) -> Page {
    Page::open(Arc::clone(&h.client), &fixture_url(fixture), viewport())
        .await
        .expect("open the fixture")
}

/// Poll an expression until it equals `expected`, then return what it last
/// said. Dispatching a key does not wait for what the key sets in motion, so
/// a test that asserts immediately is a test that flakes.
async fn eventually(page: &Page, expression: &str, expected: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let value = page.eval(expression).await.expect("eval");
        let value = value.as_str().unwrap_or_default().to_string();
        if value == expected || Instant::now() > deadline {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A printable key, described the way `keys::describe` will describe it.
fn letter(c: char) -> KeyInput {
    KeyInput {
        key: c.to_string(),
        code: format!("Key{}", c.to_ascii_uppercase()),
        windows_virtual_key_code: c.to_ascii_uppercase() as u32,
        text: c.to_string(),
        modifiers: 0,
    }
}

#[tokio::test]
async fn typed_keys_land_in_the_focused_field() {
    let h = harness().await;
    let page = open(&h, "form.html").await;
    page.eval("document.querySelector('#name').focus()").await.expect("focus");

    for c in "hi".chars() {
        page.dispatch_key(&letter(c)).await.expect("dispatch a key");
    }

    let value = eventually(&page, "document.querySelector('#name').value", "hi").await;
    assert_eq!(value, "hi");
}

#[tokio::test]
async fn enter_submits_the_form_it_is_typed_into() {
    let h = harness().await;
    let page = open(&h, "form.html").await;
    page.eval("document.querySelector('#name').focus()").await.expect("focus");

    let enter = KeyInput {
        key: "Enter".to_string(),
        code: "Enter".to_string(),
        windows_virtual_key_code: 13,
        text: "\r".to_string(),
        modifiers: 0,
    };
    page.dispatch_key(&enter).await.expect("dispatch enter");

    assert_eq!(eventually(&page, "document.title", "Submitted").await, "Submitted");
}

#[tokio::test]
async fn blurring_takes_the_focus_off_the_field() {
    let h = harness().await;
    let page = open(&h, "form.html").await;
    page.eval("document.querySelector('#name').focus()").await.expect("focus");
    assert_eq!(
        eventually(&page, "document.activeElement.id", "name").await,
        "name"
    );

    page.blur().await.expect("blur");

    let active = eventually(&page, "document.activeElement.tagName", "BODY").await;
    assert_eq!(active, "BODY", "focus should have gone back to the document");
}

/// Where an element is, right now, in CSS pixels.
async fn center_of(page: &Page, selector: &str) -> CssPoint {
    let value = page
        .eval(&format!(
            "(() => {{ const r = document.querySelector('{selector}').getBoundingClientRect(); \
              return {{ x: r.left + r.width / 2, y: r.top + r.height / 2 }}; }})()"
        ))
        .await
        .expect("read a rect");
    CssPoint {
        x: value["x"].as_f64().expect("an x"),
        y: value["y"].as_f64().expect("a y"),
    }
}

#[tokio::test]
async fn clicking_a_link_follows_it() {
    let h = harness().await;
    let page = open(&h, "form.html").await;
    let at = center_of(&page, "#link").await;

    page.dispatch_mouse(&MouseInput::press(at)).await.expect("press");
    page.dispatch_mouse(&MouseInput::release(at)).await.expect("release");

    assert_eq!(
        eventually(&page, "document.title", "Fixture Page").await,
        "Fixture Page"
    );
}

#[tokio::test]
async fn a_wheel_scrolls_what_is_under_the_pointer() {
    let h = harness().await;
    let page = open(&h, "form.html").await;
    let at = center_of(&page, "#scroller").await;

    page.dispatch_mouse(&MouseInput::wheel(at, 200.0)).await.expect("wheel");

    assert_eq!(
        eventually(
            &page,
            "String(document.querySelector('#scroller').scrollTop > 0)",
            "true"
        )
        .await,
        "true",
        "the scroller under the pointer should have moved"
    );
    let document_scroll = page.eval("window.scrollY").await.expect("scrollY");
    assert_eq!(
        document_scroll.as_f64(),
        Some(0.0),
        "the document must not scroll when a nested scroller was under the pointer"
    );
}
