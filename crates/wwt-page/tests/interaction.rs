use std::sync::Arc;
use std::time::{Duration, Instant};

use wwt_cdp::{Chromium, Client};
use wwt_frame::{CellSize, CssPoint, GridSize, TargetKind, Viewport};
use wwt_page::{DIRTY_BINDING, KeyInput, MouseInput, Page};

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

#[tokio::test]
async fn hints_find_every_interactive_element_in_document_order() {
    let h = harness().await;
    let targets = open(&h, "interactive.html").await.hints().await.expect("hints");

    let kinds: Vec<TargetKind> = targets.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![TargetKind::Clickable, TargetKind::Clickable, TargetKind::Editable],
        "expected the link, the button, and the text field, in that order"
    );
}

#[tokio::test]
async fn hints_skip_what_is_outside_the_viewport() {
    let h = harness().await;
    let targets = open(&h, "interactive.html").await.hints().await.expect("hints");

    assert!(
        targets.iter().all(|t| t.rect.y < 1000.0),
        "a target 3000px down the page was labelled: {targets:?}"
    );
}

#[tokio::test]
async fn hints_skip_what_something_else_is_covering() {
    let h = harness().await;
    let targets = open(&h, "interactive.html").await.hints().await.expect("hints");

    // The covered link is the only thing at x >= 600. A label on it would
    // lie: the click would land on the div on top of it.
    assert!(
        targets.iter().all(|t| t.rect.x < 600.0),
        "a covered target was labelled: {targets:?}"
    );
}

#[tokio::test]
async fn hint_geometry_is_the_elements_own_box() {
    let h = harness().await;
    let page = open(&h, "interactive.html").await;
    let targets = page.hints().await.expect("hints");
    let button = &targets[1];

    let expected = center_of(&page, "#two").await;
    assert!(
        (button.center().x - expected.x).abs() < 1.0
            && (button.center().y - expected.y).abs() < 1.0,
        "hint centre {:?} should match the button's own centre {expected:?}",
        button.center()
    );
}

#[tokio::test]
async fn measure_hints_on_a_page_full_of_links() {
    let h = harness().await;
    let page = open(&h, "links.html").await;

    // One warm pass, so the number is steady-state rather than first-run.
    page.hints().await.expect("hints");

    let start = std::time::Instant::now();
    let targets = page.hints().await.expect("hints");
    let elapsed = start.elapsed();

    println!("links.html: {} targets found in {elapsed:?}", targets.len());
    assert!(!targets.is_empty(), "the fixture is full of links");
}

/// Every run's text, for a failure message worth reading.
fn texts(extraction: &wwt_page::Extraction) -> Vec<&str> {
    extraction.runs.iter().map(|r| r.text.as_str()).collect()
}

#[tokio::test]
async fn a_fields_value_is_extracted_even_though_it_is_not_a_text_node() {
    let h = harness().await;
    let page = open(&h, "fields.html").await;
    page.eval("document.querySelector('#typed').focus()").await.expect("focus");
    for c in "hi".chars() {
        page.dispatch_key(&letter(c)).await.expect("dispatch");
    }
    eventually(&page, "document.querySelector('#typed').value", "hi").await;

    let extraction = page.extract().await.expect("extract");
    let run = extraction
        .runs
        .iter()
        .find(|r| r.text == "hi")
        .unwrap_or_else(|| panic!("typed text is missing from the frame: {:?}", texts(&extraction)));

    // It has to land inside the box it was typed into, not at the origin.
    let centre = center_of(&page, "#typed").await;
    let run_centre_y = run.rect.y + run.rect.h / 2.0;
    assert!(
        (run_centre_y - centre.y).abs() < 12.0 && run.rect.x < centre.x,
        "the value should be painted inside its own box: run {:?}, box centre {centre:?}",
        run.rect
    );
}

#[tokio::test]
async fn a_placeholder_stands_in_for_an_empty_field() {
    let h = harness().await;
    let extraction = open(&h, "fields.html").await.extract().await.expect("extract");
    assert!(
        texts(&extraction).contains(&"search the web"),
        "the placeholder is on screen, so it belongs in the frame: {:?}",
        texts(&extraction)
    );
}

#[tokio::test]
async fn a_password_is_extracted_as_bullets_and_never_as_itself() {
    let h = harness().await;
    let page = open(&h, "fields.html").await;
    page.eval("document.querySelector('#secret').focus()").await.expect("focus");
    for c in "pw".chars() {
        page.dispatch_key(&letter(c)).await.expect("dispatch");
    }
    eventually(&page, "document.querySelector('#secret').value", "pw").await;

    let extraction = page.extract().await.expect("extract");
    let texts = texts(&extraction);
    assert!(texts.contains(&"••"), "a password field shows bullets: {texts:?}");
    assert!(
        !texts.contains(&"pw"),
        "the frame must show what the browser shows, never the password itself: {texts:?}"
    );
}

#[tokio::test]
async fn a_select_shows_its_chosen_option_and_a_checkbox_shows_nothing() {
    let h = harness().await;
    let extraction = open(&h, "fields.html").await.extract().await.expect("extract");
    let texts = texts(&extraction);

    assert!(texts.contains(&"second"), "the chosen option is what is on screen: {texts:?}");
    assert!(!texts.contains(&"first"), "an unchosen option is not on screen: {texts:?}");
    // A checkbox's value is the string "on". Nothing renders it, so nor do we.
    assert!(!texts.contains(&"on"), "a checkbox has no text: {texts:?}");
}

#[tokio::test]
async fn a_textarea_wraps_the_way_the_browser_wraps_it() {
    let h = harness().await;
    let page = open(&h, "fields.html").await;
    let value = page
        .eval("document.querySelector('#wrapped').value")
        .await
        .expect("value")
        .as_str()
        .expect("a string")
        .to_string();

    let extraction = page.extract().await.expect("extract");
    let wrapped: Vec<&wwt_frame::TextRun> = extraction
        .runs
        .iter()
        .filter(|r| !r.text.is_empty() && value.contains(r.text.as_str()))
        .collect();

    let lines: Vec<&str> = wrapped.iter().map(|r| r.text.as_str()).collect();
    assert!(
        lines.len() >= 2,
        "the value is wider than the box, so the browser wrapped it and so must we: {lines:?}"
    );
    assert_eq!(
        lines.join(" "),
        value,
        "wrapping must not lose or reorder any of the text"
    );
    for pair in wrapped.windows(2) {
        assert!(
            pair[1].rect.y > pair[0].rect.y,
            "each wrapped line belongs on the row below the last: {:?}",
            wrapped.iter().map(|r| r.rect.y).collect::<Vec<f64>>()
        );
    }
}

#[tokio::test]
async fn a_field_scrolled_sideways_shows_the_part_you_are_looking_at() {
    let h = harness().await;
    let page = open(&h, "fields.html").await;
    let value = "0123456789".repeat(12);
    page.eval(&format!(
        "const el = document.querySelector('#typed'); \
         el.value = '{value}'; el.scrollLeft = el.scrollWidth;"
    ))
    .await
    .expect("scroll the field to its end");

    let extraction = page.extract().await.expect("extract");
    let run = extraction
        .runs
        .iter()
        .find(|r| !r.text.is_empty() && value.contains(r.text.as_str()))
        .unwrap_or_else(|| panic!("the field's text is missing: {:?}", texts(&extraction)));

    assert!(
        !value.starts_with(run.text.as_str()),
        "a field scrolled to its end must not show its head: {:?}",
        run.text
    );
    assert!(
        value.ends_with(run.text.as_str()),
        "it shows the tail you are looking at: {:?}",
        run.text
    );
}

#[tokio::test]
async fn the_caret_follows_the_insertion_point() {
    let h = harness().await;
    let page = open(&h, "fields.html").await;
    page.eval("const el = document.querySelector('#typed'); el.value = 'hello world'; el.focus();")
        .await
        .expect("set up the field");

    let mut xs = Vec::new();
    for offset in [0, 5, 11] {
        page.eval(&format!(
            "document.querySelector('#typed').setSelectionRange({offset}, {offset})"
        ))
        .await
        .expect("move the insertion point");
        let extraction = page.extract().await.expect("extract");
        let caret = extraction.caret.expect("a focused field has an insertion point");
        xs.push(caret.x);
    }

    assert!(
        xs[0] < xs[1] && xs[1] < xs[2],
        "the caret moves right as the offset grows: {xs:?}"
    );

    let edge = page
        .eval(
            "(() => { const el = document.querySelector('#typed'); \
              const r = el.getBoundingClientRect(); const cs = getComputedStyle(el); \
              return r.left + parseFloat(cs.borderLeftWidth) + parseFloat(cs.paddingLeft); })()",
        )
        .await
        .expect("the box's left edge")
        .as_f64()
        .expect("a number");
    assert!(
        (xs[0] - edge).abs() < 3.0,
        "at offset 0 the caret sits at the left edge of the box: caret {}, edge {edge}",
        xs[0]
    );
}

#[tokio::test]
async fn there_is_no_caret_when_no_field_is_focused() {
    let h = harness().await;
    let extraction = open(&h, "fields.html").await.extract().await.expect("extract");
    assert!(
        extraction.caret.is_none(),
        "nothing is focused, so nothing is being typed into"
    );
}

#[tokio::test]
async fn extracting_a_field_does_not_signal_the_page_dirty() {
    let h = harness().await;
    let page = open(&h, "fields.html").await;
    // Subscribed before the setup, because focusing a field whose value
    // overflows scrolls it to the caret, and that fires a signal of its own.
    // Waiting for that one to pass is a race; draining it is not.
    let mut events = h.client.subscribe();
    page.eval(
        "const el = document.querySelector('#typed'); \
         el.value = 'a value long enough to overflow its box several times over'; el.focus();",
    )
    .await
    .expect("set up the field");
    tokio::time::sleep(Duration::from_millis(300)).await;
    while events.try_recv().is_ok() {}

    page.extract().await.expect("extract");
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Measuring a field puts a mirror of it into the document. If our own
    // MutationObserver sees that, the page reports itself dirty, the core
    // re-extracts, and an idle page spins forever.
    let mut signals = 0;
    while let Ok(event) = events.try_recv() {
        if event.method == "Runtime.bindingCalled" && event.params["name"] == DIRTY_BINDING {
            signals += 1;
        }
    }
    assert_eq!(signals, 0, "extraction made the page call itself dirty");
}

