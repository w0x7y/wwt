mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Harness, harness, open, open_url, runtime, viewport};
use tokio::sync::mpsc;
use wwt_cdp::{Client, Event};
use wwt_frame::{CssPoint, TargetKind};
use wwt_page::{Extraction, KeyInput, MouseInput, Page};

/// Every run's text, for a failure message worth reading.
fn texts(extraction: &Extraction) -> Vec<&str> {
    extraction.runs.iter().map(|r| r.text.as_str()).collect()
}

/// Poll the page until its extraction says what the test is waiting for.
///
/// Dispatching a key does not wait for what the key sets in motion, so a
/// test that asserts immediately is a test that flakes. Deliberately
/// `extract` rather than `eval`: a change that leaves the DOM right and the
/// extraction wrong has to fail something, and it only does that when the
/// assertion crosses the same seam the browser crosses.
async fn eventually<F>(page: &Page, wanted: &str, ready: F) -> Extraction
where
    F: Fn(&Extraction) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let extraction = page.extract().await.expect("extract");
        if ready(&extraction) {
            return extraction;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {wanted}; the page showed {:?}",
            texts(&extraction)
        );
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

#[test]
fn typed_keys_land_in_the_focused_field() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "form.html").await;
        page.eval("document.querySelector('#name').focus()").await.expect("focus");

        for c in "hi".chars() {
            page.dispatch_key(&letter(c)).await.expect("dispatch a key");
        }

        let extraction = eventually(&page, "the typed text", |e| texts(e).contains(&"hi")).await;
        assert!(
            texts(&extraction).contains(&"hi"),
            "what you typed has to be what you can see: {:?}",
            texts(&extraction)
        );
    });
}

#[test]
fn enter_submits_the_form_it_is_typed_into() {
    let h = harness();
    runtime().block_on(async {
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

        let extraction = eventually(&page, "the submitted page", |e| e.title == "Submitted").await;
        assert_eq!(extraction.title, "Submitted");
    });
}

#[test]
fn blurring_takes_the_focus_off_the_field() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "form.html").await;
        page.eval("document.querySelector('#name').focus()").await.expect("focus");
        eventually(&page, "a focused field", |e| e.caret.is_some()).await;

        page.blur().await.expect("blur");

        // Whether anything has focus is visible through the interface as
        // whether there is anywhere for typing to land.
        let extraction = eventually(&page, "nothing focused", |e| e.caret.is_none()).await;
        assert!(
            extraction.caret.is_none(),
            "focus should have gone back to the document: {:?}",
            extraction.caret
        );
    });
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

#[test]
fn clicking_a_link_follows_it() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "form.html").await;
        let at = center_of(&page, "#link").await;

        page.dispatch_mouse(&MouseInput::press(at)).await.expect("press");
        page.dispatch_mouse(&MouseInput::release(at)).await.expect("release");

        let extraction =
            eventually(&page, "the linked page", |e| e.title == "Fixture Page").await;
        assert!(
            extraction.url.ends_with("simple.html"),
            "the click should have followed the link: {}",
            extraction.url
        );
    });
}

#[test]
fn a_wheel_scrolls_what_is_under_the_pointer() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "form.html").await;
        let at = center_of(&page, "#scroller").await;
        let before = page
            .extract()
            .await
            .expect("extract")
            .runs
            .iter()
            .find(|r| r.text == "inner content")
            .expect("the scroller's content is on screen")
            .rect
            .y;

        page.dispatch_mouse(&MouseInput::wheel(at, 200.0)).await.expect("wheel");

        // The scroller's own content moving is visible as its text moving up
        // the page, which is the only part of it the browser ever sees.
        let inner_y = |e: &Extraction| {
            e.runs.iter().find(|r| r.text == "inner content").map(|r| r.rect.y)
        };
        let extraction = eventually(&page, "the scroller to move", |e| {
            inner_y(e).is_some_and(|y| y < before)
        })
        .await;

        assert!(
            inner_y(&extraction).is_some_and(|y| y < before),
            "the scroller under the pointer should have moved"
        );
        assert_eq!(
            extraction.scroll_y, 0.0,
            "the document must not scroll when a nested scroller was under the pointer"
        );
    });
}

#[test]
fn hints_find_every_interactive_element_in_document_order() {
    let h = harness();
    runtime().block_on(async {
        let targets = open(&h, "interactive.html").await.hints().await.expect("hints");

        let kinds: Vec<TargetKind> = targets.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![TargetKind::Clickable, TargetKind::Clickable, TargetKind::Editable],
            "expected the link, the button, and the text field, in that order"
        );
    });
}

#[test]
fn hints_skip_what_is_outside_the_viewport() {
    let h = harness();
    runtime().block_on(async {
        let targets = open(&h, "interactive.html").await.hints().await.expect("hints");

        assert!(
            targets.iter().all(|t| t.rect.y < 1000.0),
            "a target 3000px down the page was labelled: {targets:?}"
        );
    });
}

#[test]
fn hints_skip_what_something_else_is_covering() {
    let h = harness();
    runtime().block_on(async {
        let targets = open(&h, "interactive.html").await.hints().await.expect("hints");

        // The covered link is the only thing at x >= 600. A label on it would
        // lie: the click would land on the div on top of it.
        assert!(
            targets.iter().all(|t| t.rect.x < 600.0),
            "a covered target was labelled: {targets:?}"
        );
    });
}

#[test]
fn hint_geometry_is_the_elements_own_box() {
    let h = harness();
    runtime().block_on(async {
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
    });
}

#[test]
fn measure_hints_on_a_page_full_of_links() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "links.html").await;

        // One warm pass, so the number is steady-state rather than first-run.
        page.hints().await.expect("hints");

        let start = std::time::Instant::now();
        let targets = page.hints().await.expect("hints");
        let elapsed = start.elapsed();

        println!("links.html: {} targets found in {elapsed:?}", targets.len());
        assert!(!targets.is_empty(), "the fixture is full of links");
    });
}

#[test]
fn a_fields_value_is_extracted_even_though_it_is_not_a_text_node() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "fields.html").await;
        page.eval("document.querySelector('#typed').focus()").await.expect("focus");
        for c in "hi".chars() {
            page.dispatch_key(&letter(c)).await.expect("dispatch");
        }
        let extraction = eventually(&page, "the typed text", |e| texts(e).contains(&"hi")).await;
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
    });
}

#[test]
fn a_placeholder_stands_in_for_an_empty_field() {
    let h = harness();
    runtime().block_on(async {
        let extraction = open(&h, "fields.html").await.extract().await.expect("extract");
        assert!(
            texts(&extraction).contains(&"search the web"),
            "the placeholder is on screen, so it belongs in the frame: {:?}",
            texts(&extraction)
        );
    });
}

#[test]
fn a_password_is_extracted_as_bullets_and_never_as_itself() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "fields.html").await;
        page.eval("document.querySelector('#secret').focus()").await.expect("focus");
        for c in "pw".chars() {
            page.dispatch_key(&letter(c)).await.expect("dispatch");
        }
        // Two characters typed, so two bullets: the frame shows what the
        // browser shows, and never what was typed.
        let extraction = eventually(&page, "the bullets", |e| texts(e).contains(&"\u{2022}\u{2022}")).await;
        let texts = texts(&extraction);
        assert!(texts.contains(&"••"), "a password field shows bullets: {texts:?}");
        assert!(
            !texts.contains(&"pw"),
            "the frame must show what the browser shows, never the password itself: {texts:?}"
        );
    });
}

#[test]
fn a_select_shows_its_chosen_option_and_a_checkbox_shows_nothing() {
    let h = harness();
    runtime().block_on(async {
        let extraction = open(&h, "fields.html").await.extract().await.expect("extract");
        let texts = texts(&extraction);

        assert!(texts.contains(&"second"), "the chosen option is what is on screen: {texts:?}");
        assert!(!texts.contains(&"first"), "an unchosen option is not on screen: {texts:?}");
        // A checkbox's value is the string "on". Nothing renders it, so nor do we.
        assert!(!texts.contains(&"on"), "a checkbox has no text: {texts:?}");
    });
}

#[test]
fn a_textarea_wraps_the_way_the_browser_wraps_it() {
    let h = harness();
    runtime().block_on(async {
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
    });
}

#[test]
fn a_field_scrolled_sideways_shows_the_part_you_are_looking_at() {
    let h = harness();
    runtime().block_on(async {
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
    });
}

#[test]
fn the_caret_follows_the_insertion_point() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "fields.html").await;
        page.eval("const el = document.querySelector('#typed'); el.value = 'hello world'; el.focus();")
            .await
            .expect("set up the field");

        let mut carets = Vec::new();
        for offset in [0, 5, 11] {
            page.eval(&format!(
                "document.querySelector('#typed').setSelectionRange({offset}, {offset})"
            ))
            .await
            .expect("move the insertion point");
            let extraction = page.extract().await.expect("extract");
            carets.push(extraction.caret.expect("a focused field has an insertion point"));
        }

        // The value is one unwrapped line, so every caret sits on it and the
        // offset is what moves. Counted in characters, not pixels: the frame
        // gives each character a cell whatever the font's advance was.
        assert_eq!(
            carets.iter().map(|c| c.offset).collect::<Vec<_>>(),
            vec![0, 5, 11],
            "the caret counts characters into the line"
        );
        assert!(
            carets.iter().all(|c| c.x == carets[0].x && c.baseline == carets[0].baseline),
            "the line itself did not move: {carets:?}"
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
            (carets[0].x - edge).abs() < 3.0,
            "the caret's line starts at the left edge of the box: caret {}, edge {edge}",
            carets[0].x
        );
    });
}

#[test]
fn the_caret_counts_from_the_start_of_its_own_wrapped_line() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "fields.html").await;
        // A textarea wide enough for a handful of words per line, so the
        // insertion point below is on neither the first line nor the last.
        page.eval(
            "const el = document.querySelector('#wrapped'); el.focus(); \
             el.setSelectionRange(30, 30);",
        )
        .await
        .expect("set up the field");

        let caret = page
            .extract()
            .await
            .expect("extract")
            .caret
            .expect("a focused field has an insertion point");

        // Character 30 of the value, but the line it is on does not start at
        // character 0, so the offset has to be smaller than 30.
        assert!(
            caret.offset < 30,
            "the offset is into the line, not into the value: {caret:?}"
        );
        assert!(caret.baseline > 0.0, "on one of the wrapped lines: {caret:?}");
    });
}

#[test]
fn there_is_no_caret_when_no_field_is_focused() {
    let h = harness();
    runtime().block_on(async {
        let extraction = open(&h, "fields.html").await.extract().await.expect("extract");
        assert!(
            extraction.caret.is_none(),
            "nothing is focused, so nothing is being typed into"
        );
    });
}

#[test]
fn extracting_a_field_does_not_signal_the_page_dirty() {
    let h = harness();
    runtime().block_on(async {
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
        drain(&mut events).await;

        page.extract().await.expect("extract");

        // Measuring a field puts a mirror of it into the document. If our own
        // MutationObserver sees that, the page reports itself dirty, the core
        // re-extracts, and an idle page spins forever.
        assert_eq!(
            count_signals(&mut events, &page).await,
            0,
            "extraction made the page call itself dirty"
        );
    });
}

/// How long a page has to stay silent before we call it settled.
const QUIET: Duration = Duration::from_millis(300);

/// Throw away everything the page has said, and everything it says until it
/// has been quiet for `QUIET`.
///
/// Draining once is not enough: the setup a test does before it starts
/// counting can itself signal, and under a loaded machine that signal can
/// arrive after a fixed sleep has expired.
async fn drain(events: &mut mpsc::UnboundedReceiver<Event>) {
    loop {
        let mut seen = false;
        while events.try_recv().is_ok() {
            seen = true;
        }
        tokio::time::sleep(QUIET).await;
        if !seen && events.is_empty() {
            return;
        }
    }
}

/// Count the dirty signals that arrive before the page goes quiet.
async fn count_signals(events: &mut mpsc::UnboundedReceiver<Event>, page: &Page) -> usize {
    let mut signals = 0;
    tokio::time::sleep(QUIET).await;
    while let Ok(event) = events.try_recv() {
        if page.is_dirty(&event) {
            signals += 1;
        }
    }
    signals
}

/// Run `act`, then count the dirty signals it produced.
///
/// The subscription is opened and drained before acting, because focusing a
/// field can scroll it and the signal that causes is not the one under test.
async fn signals_from<F, Fut>(h: &Harness, page: &Page, act: F) -> usize
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut events = h.client.subscribe();
    drain(&mut events).await;
    act().await;
    count_signals(&mut events, page).await
}

/// Drive the scroll throttle with a clock the test owns.
///
/// `signal()` is one scroll event and `fire()` is the window expiring, so
/// the call pattern below is a fact rather than a race. The throttle is the
/// one piece of this script whose mistakes are invisible from a frame: too
/// eager only looks like extra work, and too lazy only looks like lag.
async fn throttle_log(page: &Page, actions: &str) -> Vec<String> {
    let script = format!(
        r#"(() => {{
            const log = [];
            let due = null;
            const schedule = (fn) => {{ due = fn; }};
            const fire = () => {{ const f = due; due = null; if (f) f(); }};
            const signal = window.__wwt.__pure.throttle(
                () => log.push("out"), 16, schedule
            );
            {actions}
            return JSON.stringify(log);
        }})()"#
    );
    let raw = page.eval(&script).await.expect("drive the throttle");
    serde_json::from_str(raw.as_str().expect("a JSON string")).expect("a log")
}

#[test]
fn one_scroll_signals_at_once_and_never_again() {
    // The whole point of the leading edge. A keypress produces exactly one
    // scroll event, so waiting out a window before saying so was sixteen
    // milliseconds of latency buying nothing.
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;

        assert_eq!(
            throttle_log(&page, "signal();").await,
            ["out"],
            "the first event goes out without waiting for anything"
        );
        assert_eq!(
            throttle_log(&page, "signal(); fire();").await,
            ["out"],
            "nothing arrived during the window, so there is nothing to say at the end of it"
        );
    });
}

#[test]
fn a_burst_of_scrolls_is_coalesced_into_one_more_signal() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;

        assert_eq!(
            throttle_log(&page, "signal(); signal(); fire();").await,
            ["out", "out"],
            "the second event waits out the window rather than being lost"
        );
        assert_eq!(
            throttle_log(&page, "signal(); signal(); signal(); signal(); fire();").await,
            ["out", "out"],
            "three events during one window are one signal, not three"
        );
    });
}

#[test]
fn a_scroll_that_keeps_going_signals_once_per_window() {
    // A page still scrolling when the window expires must not be allowed to
    // reopen the leading edge, or a fling would signal per frame.
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;

        assert_eq!(
            throttle_log(&page, "signal(); signal(); fire(); signal(); fire(); fire();").await,
            ["out", "out", "out"],
            "one per window while it keeps arriving, and one last one when it stops"
        );
    });
}

#[test]
fn a_scroll_after_the_page_settles_signals_at_once_again() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;

        assert_eq!(
            throttle_log(&page, "signal(); fire(); signal();").await,
            ["out", "out"],
            "a quiet window leaves the next scroll to go out immediately"
        );
    });
}

#[test]
fn measure_scroll_latency() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "heavy.html").await;
        page.extract().await.expect("warm");

        let mut events = h.client.subscribe();
        drain(&mut events).await;

        let start = Instant::now();
        page.scroll_by(20.0, viewport()).await.expect("scroll");
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("the page should say it scrolled")
                .expect("the connection stays open");
            if page.is_dirty(&event) {
                break;
            }
        }
        let signalled = start.elapsed();
        let runs = page.extract().await.expect("extract").runs.len();
        let total = start.elapsed();

        println!(
            "heavy.html: wheel to dirty signal {signalled:?}, {runs} new runs in hand {total:?}"
        );
    });
}

fn arrow_left() -> KeyInput {
    KeyInput {
        key: "ArrowLeft".to_string(),
        code: "ArrowLeft".to_string(),
        windows_virtual_key_code: 37,
        text: String::new(),
        modifiers: 0,
    }
}

#[test]
fn typing_into_a_field_signals_the_page_dirty() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "fields.html").await;
        page.eval("document.querySelector('#typed').focus()").await.expect("focus");

        // A control's value is element state, not a text node, so no mutation
        // the observer can see accompanies it.
        let signals = signals_from(&h, &page, || async {
            for c in "hi".chars() {
                page.dispatch_key(&letter(c)).await.expect("dispatch a key");
            }
        })
        .await;

        assert!(signals > 0, "typing left the frame showing the old value");
    });
}

#[test]
fn moving_the_insertion_point_signals_the_page_dirty() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "fields.html").await;
        page.eval("const el = document.querySelector('#typed'); el.value = 'hello'; el.focus();")
            .await
            .expect("set up the field");

        let signals = signals_from(&h, &page, || async {
            page.dispatch_key(&arrow_left()).await.expect("dispatch an arrow");
        })
        .await;

        assert!(signals > 0, "the caret would stay where it was until something else moved");
    });
}

#[test]
fn focusing_a_field_signals_the_page_dirty() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "fields.html").await;

        // Whether a control is focused decides whether it has a caret at all,
        // and focus changes nothing else in the document.
        let signals = signals_from(&h, &page, || async {
            page.eval("document.querySelector('#typed').focus()").await.expect("focus");
        })
        .await;

        assert!(signals > 0, "a field clicked into would have no caret until it changed");
    });
}

#[test]
fn scrolling_to_an_offset_lands_there() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "tall.html").await;

        page.scroll_to(400.0).await.expect("scroll to an offset");
        let extraction = page.extract().await.expect("extract");

        // Chromium clamps to the scrollable range and rounds to device
        // pixels, so this asserts the neighbourhood rather than the exact
        // number.
        assert!(
            (extraction.scroll_y - 400.0).abs() < 2.0,
            "scrolled to {}, wanted 400",
            extraction.scroll_y
        );
    });
}

#[test]
fn two_pages_on_one_browser_read_their_own_documents() {
    // One client, one websocket, one event stream. Every command a page
    // issues is `call_on` its own session, and this is what says so: two
    // targets alive at once, each extracting what is in it and not what is
    // in the other.
    let h = harness();
    runtime().block_on(async {
        let first = open(&h, "hello.html").await;
        let second = open(&h, "blank.html").await;

        let one = first.extract().await.expect("extract the first");
        let two = second.extract().await.expect("extract the second");

        assert!(
            one.runs.iter().any(|run| run.text.contains("hello")),
            "{:?}",
            texts(&one)
        );
        assert!(
            two.runs.iter().any(|run| run.text.contains("opener")),
            "{:?}",
            texts(&two)
        );
        assert_ne!(one.url, two.url);

        second.close().await.expect("close the second");
        first.close().await.expect("close the first");
    });
}

#[test]
fn a_closed_page_is_gone_from_the_browser() {
    let h = harness();
    runtime().block_on(async {
        let page = open_url(&h, "about:blank").await;
        let target = page.target_id().to_string();

        page.close().await.expect("close the target");

        // `Target.closeTarget` answers as soon as the browser has accepted
        // the close, not once the target is gone, so this waits rather than
        // asserting on the instant after. Making `close` itself wait would
        // put a second round trip on every tab you shut for the sake of an
        // answer nothing needs.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let targets = h
                .client
                .call("Target.getTargets", serde_json::json!({}))
                .await
                .expect("list targets");
            let still_there = targets["targetInfos"]
                .as_array()
                .expect("an array of targets")
                .iter()
                .any(|info| info["targetId"] == target.as_str());
            if !still_there {
                break;
            }
            assert!(Instant::now() < deadline, "the target outlived the close");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
}

#[test]
fn activating_a_page_makes_it_the_one_the_browser_has_in_front() {
    // Input dispatch is answered by whichever target is in front, which is
    // why switching tabs has to activate. Two targets, activate the first,
    // and it is the one that takes the click.
    let h = harness();
    runtime().block_on(async {
        let first = open(&h, "click.html").await;
        let second = open_url(&h, "about:blank").await;

        first.activate().await.expect("activate the first page");

        let at = CssPoint { x: 40.0, y: 20.0 };
        first.dispatch_mouse(&MouseInput::press(at)).await.expect("press");
        first.dispatch_mouse(&MouseInput::release(at)).await.expect("release");

        let clicked = first.eval("window.__clicked === true").await.expect("read the flag");
        assert_eq!(clicked, serde_json::Value::Bool(true));

        second.close().await.expect("close the second");
        first.close().await.expect("close the first");
    });
}

#[test]
fn a_tab_the_page_opened_for_itself_is_adopted_with_our_script_in_it() {
    let h = harness();
    runtime().block_on(async {
        // Subscribed before the opener exists: the attach for the tab it
        // opens arrives unasked for, and a subscription taken afterwards has
        // already missed it.
        let mut events = h.client.subscribe();
        let opener = open(&h, "blank.html").await;

        // Clicked rather than scripted. `window.open` from an evaluation has
        // no user activation behind it and Chromium's popup blocker returns
        // null, and a link opened without a gesture carries no opener for
        // `opened_by_a_page` to recognise it by.
        let at = center_of(&opener, "#new").await;
        opener.dispatch_mouse(&MouseInput::press(at)).await.expect("press");
        opener.dispatch_mouse(&MouseInput::release(at)).await.expect("release");

        let attached = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = events.recv().await.expect("the browser stayed up");
                if let Some(attached) = Client::opened_by_a_page(&event) {
                    return attached;
                }
            }
        })
        .await
        .expect("the browser reported the tab its page opened");

        let adopted = Page::adopt(Arc::clone(&h.client), attached, viewport())
            .await
            .expect("adopt the tab");

        // The assertion that matters: the bootstrap is in the document the
        // tab already loaded, not merely in the next one it navigates to. A
        // tab whose document ran before we reached it has no `__wwt` in it at
        // all, and extracting one fails rather than coming back empty.
        let extraction = eventually(&adopted, "the adopted tab's text", |extraction| {
            extraction.runs.iter().any(|run| run.text.contains("hello"))
        })
        .await;
        assert!(
            extraction.url.ends_with("hello.html"),
            "adopted the wrong target: {}",
            extraction.url
        );

        adopted.close().await.expect("close the adopted tab");
        opener.close().await.expect("close the opener");
    });
}
