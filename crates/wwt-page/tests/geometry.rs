//! The arithmetic inside the injected script, asserted on directly.
//!
//! These are the sharpest functions in the codebase and the ones whose
//! mistakes are least visible from anywhere else: an insertion point two
//! characters too far along still looks like a caret in roughly the right
//! place, in a frame, in a terminal. Every one of them takes data and
//! returns data, so `__wwt.__pure` reaches them without a page, a fixture,
//! or a layout — one browser answers the whole file.

mod common;

use common::{harness, open, runtime};
use serde_json::{Value, json};
use wwt_page::Page;

/// Call one of the script's pure functions and return what it said.
async fn pure(page: &Page, call: &str) -> Value {
    page.eval(&format!("window.__wwt.__pure.{call}"))
        .await
        .unwrap_or_else(|e| panic!("{call} failed: {e}"))
}

/// Lines the way a wrapped control reports them: text, where the line
/// starts on screen, and where its first character sits in the value.
fn lines() -> Value {
    json!([
        { "text": "hello", "x": 10.0, "y": 0.0, "start": 0 },
        { "text": "world", "x": 10.0, "y": 20.0, "start": 6 },
    ])
}

#[test]
fn the_caret_counts_characters_into_its_own_line() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        let lines = lines();

        let head = pure(&page, &format!("caretIn({lines}, 3)")).await;
        assert_eq!(head["offset"], 3, "three characters into the first line");
        assert_eq!(head["y"], 0.0);

        // The value's character 8 is character 2 of the second line. This is
        // the whole reason the caret is an offset into a line rather than an
        // offset into the value: painting counts from the line's own start.
        let tail = pure(&page, &format!("caretIn({lines}, 8)")).await;
        assert_eq!(tail["offset"], 2, "two characters into the second line");
        assert_eq!(tail["y"], 20.0);
    });
}

#[test]
fn a_soft_wrap_puts_the_caret_at_the_start_of_the_second_line() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        let lines = lines();

        // Offset 6 belongs to both lines: it is the end of one and the start
        // of the next. The browser puts the caret at the start of the
        // second, so we do too.
        let caret = pure(&page, &format!("caretIn({lines}, 6)")).await;
        assert_eq!(caret["offset"], 0);
        assert_eq!(caret["y"], 20.0, "the line below, not the one that ended");
    });
}

#[test]
fn an_offset_in_the_gap_a_line_broke_at_clamps_to_the_end_of_the_text() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        let lines = lines();

        // Character 5 is the space the line was broken at. Nothing paints
        // it, so the furthest the caret can go is past the last character
        // that is painted.
        let caret = pure(&page, &format!("caretIn({lines}, 5)")).await;
        assert_eq!(caret["offset"], 5, "one past the last painted character");
        assert_eq!(caret["y"], 0.0);
    });
}

#[test]
fn an_offset_left_of_a_scrolled_window_clamps_to_its_first_character() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        // A field scrolled sideways shows a window into its value, so the
        // line it reports starts partway along.
        let scrolled = json!([{ "text": "789", "x": 0.0, "y": 0.0, "start": 7 }]);

        let caret = pure(&page, &format!("caretIn({scrolled}, 2)")).await;
        assert_eq!(caret["offset"], 0, "the insertion point is left of what is shown");
    });
}

#[test]
fn a_control_with_no_lines_has_no_caret() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        assert_eq!(pure(&page, "caretIn([], 0)").await, Value::Null);
    });
}

#[test]
fn splitting_one_line_box_keeps_the_whole_string() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        let split = pure(&page, "splitLines([{top: 0}], 'hello world', () => 0)").await;

        assert_eq!(split.as_array().expect("an array").len(), 1);
        assert_eq!(split[0]["text"], "hello world");
        assert_eq!(split[0]["start"], 0);
        assert_eq!(split[0]["end"], 11);
    });
}

#[test]
fn splitting_finds_the_character_each_line_starts_at() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        // Two line boxes, and a string whose seventh character is the first
        // to have moved down to the second of them.
        let split = pure(
            &page,
            "splitLines([{top: 0}, {top: 20}], 'hello world', (k) => (k < 6 ? 0 : 20))",
        )
        .await;

        assert_eq!(split[0]["text"], "hello ");
        assert_eq!(split[1]["text"], "world");
        assert_eq!(split[1]["start"], 6, "the boundary is where the tops step");
        assert_eq!(split[1]["end"], 11);
    });
}

#[test]
fn splitting_three_boxes_keeps_every_character_exactly_once() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        let text = "one two three";
        let split = pure(
            &page,
            "splitLines([{top: 0}, {top: 20}, {top: 40}], 'one two three', \
             (k) => (k < 4 ? 0 : k < 8 ? 20 : 40))",
        )
        .await;

        let parts: Vec<&str> = split
            .as_array()
            .expect("an array")
            .iter()
            .map(|line| line["text"].as_str().expect("a string"))
            .collect();
        assert_eq!(parts, vec!["one ", "two ", "three"]);
        assert_eq!(parts.concat(), text, "wrapping must not lose or reorder text");
    });
}

#[test]
fn a_string_with_no_line_boxes_has_no_lines() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        let split = pure(&page, "splitLines([], 'hello', () => 0)").await;
        assert_eq!(split.as_array().expect("an array").len(), 0);
    });
}

#[test]
fn the_search_finds_the_first_offset_that_crosses() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;

        assert_eq!(pure(&page, "firstWhere(0, 10, (k) => k >= 4)").await, 4);
        assert_eq!(
            pure(&page, "firstWhere(0, 10, () => false)").await,
            10,
            "nothing crosses, so the answer is the end of the range"
        );
        assert_eq!(
            pure(&page, "firstWhere(0, 10, () => true)").await,
            0,
            "everything crosses, so the answer is the start of it"
        );
        assert_eq!(pure(&page, "firstWhere(3, 3, () => true)").await, 3, "an empty range");
        assert_eq!(
            pure(&page, "firstWhere(4, 10, (k) => k >= 2)").await,
            4,
            "the search never looks left of where it was told to start"
        );
    });
}
