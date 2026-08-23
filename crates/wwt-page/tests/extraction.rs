mod common;

use common::{fixture_url, harness, open, runtime, viewport};
use wwt_page::Page;

#[test]
fn extracts_the_visible_text_of_a_page() {
    let h = harness();
    runtime().block_on(async {
        let runs = open(&h, "simple.html").await.extract().await.expect("extract").runs;

        let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
        assert!(texts.contains(&"Heading"), "runs were {texts:?}");
        assert!(texts.contains(&"First paragraph."), "runs were {texts:?}");
    });
}

#[test]
fn skips_hidden_text() {
    let h = harness();
    runtime().block_on(async {
        let runs = open(&h, "simple.html").await.extract().await.expect("extract").runs;

        let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
        assert!(
            !texts.contains(&"Invisible text."),
            "visibility:hidden text must not be extracted: {texts:?}"
        );
    });
}

#[test]
fn carries_color_and_weight_through() {
    let h = harness();
    runtime().block_on(async {
        let runs = open(&h, "simple.html").await.extract().await.expect("extract").runs;

        let heading = runs.iter().find(|r| r.text == "Heading").expect("heading run");
        assert_eq!(heading.style.fg, wwt_frame::Rgb { r: 255, g: 0, b: 0 });
        assert!(heading.style.bold, "font-weight 700 is bold");

        let para = runs
            .iter()
            .find(|r| r.text == "First paragraph.")
            .expect("paragraph run");
        assert_eq!(para.style.fg, wwt_frame::Rgb { r: 0, g: 0, b: 255 });
        assert!(!para.style.bold);
    });
}

#[test]
fn orders_runs_down_the_page() {
    let h = harness();
    runtime().block_on(async {
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
    });
}

#[test]
fn reads_the_document_title() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        assert_eq!(page.extract().await.expect("extract").status.title, "Fixture Page");
    });
}

#[test]
fn lays_the_page_out_at_the_viewport_we_asked_for() {
    let h = harness();
    runtime().block_on(async {
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
    });
}

#[test]
fn a_dom_mutation_signals_dirtiness() {
    let h = harness();
    runtime().block_on(async {
        let mut events = h.client.subscribe();
        let page = open(&h, "mutating.html").await;

        // The fixture mutates itself 100ms after load, so the signal arrives
        // after we are already subscribed and watching.
        let signalled = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while let Some(event) = events.recv().await {
                if event.method == "Runtime.bindingCalled"
                    && event.params["name"] == wwt_page::DIRTY_BINDING
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
    });
}

#[test]
fn extraction_reports_scroll_geometry() {
    let h = harness();
    runtime().block_on(async {
        let extraction = open(&h, "simple.html").await.extract().await.expect("extract");

        assert_eq!(extraction.status.scroll_y, 0.0);
        assert!(extraction.status.viewport_height > 0.0);
        assert!(extraction.status.url.ends_with("simple.html"), "url was {}", extraction.status.url);
        assert_eq!(extraction.status.title, "Fixture Page");
    });
}

#[test]
fn scroll_progress_is_zero_when_the_document_fits() {
    let e = wwt_page::Status {
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
    let e = wwt_page::Status {
        title: String::new(),
        url: String::new(),
        scroll_y: 600.0,
        scroll_height: 1000.0,
        viewport_height: 400.0,
    };
    assert_eq!(e.scroll_progress(), 1.0);
}

/// The wheel event is dispatched to the compositor, so the scroll it causes
/// is not complete when the command returns. Poll for the effect rather than
/// sleeping a fixed amount.
async fn await_scroll_past(page: &Page, floor: f64) -> f64 {
    for _ in 0..100 {
        let y = page.extract().await.expect("extract").status.scroll_y;
        if y > floor {
            return y;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the page never scrolled past {floor}");
}

#[test]
fn scrolling_moves_the_page_and_changes_the_runs() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "tall.html").await;

        let before = page.extract().await.expect("extract");
        assert_eq!(before.status.scroll_y, 0.0);
        let first_before = before.runs.first().expect("a run").text.clone();

        page.scroll_by(200.0, viewport()).await.expect("scroll");
        await_scroll_past(&page, 0.0).await;

        let after = page.extract().await.expect("extract");
        let first_after = after.runs.first().expect("a run").text.clone();
        assert_ne!(
            first_before, first_after,
            "the topmost run should differ after scrolling"
        );
    });
}

#[test]
fn a_text_node_taller_than_the_viewport_keeps_the_lines_on_screen() {
    // Extraction culls a text node it can see is off screen rather than
    // splitting it into lines first. One node several viewports tall is
    // where that goes wrong if the question is asked about the wrong box:
    // its visible lines are in its middle, so neither end is on screen.
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "straddling.html").await;

        page.eval("window.scrollTo(0, 1500)").await.expect("scroll");
        await_scroll_past(&page, 0.0).await;

        let runs = page.extract().await.expect("extract").runs;
        assert!(
            runs.len() > 15,
            "a screenful of the middle of the node, not {} runs",
            runs.len()
        );
        let first = &runs.first().expect("a run").text;
        assert!(
            !first.starts_with("w0 "),
            "the head of the node is scrolled off, but the runs start at it: {first:?}"
        );
    });
}

#[test]
fn scroll_to_end_reaches_the_bottom() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "tall.html").await;

        page.scroll_to_end().await.expect("scroll to end");
        let end = page.extract().await.expect("extract");
        assert!(end.status.scroll_progress() > 0.99, "progress was {}", end.status.scroll_progress());

        page.scroll_to_top().await.expect("scroll to top");
        let top = page.extract().await.expect("extract");
        assert_eq!(top.status.scroll_y, 0.0);
    });
}

#[test]
fn history_moves_back_and_forward() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        page.navigate(&fixture_url("tall.html")).await.expect("navigate");

        assert!(page.back().await.expect("back"), "there should be an entry to go back to");
        assert!(
            page.extract().await.expect("extract").status.url.ends_with("simple.html"),
            "back should land on the first fixture"
        );

        assert!(page.forward().await.expect("forward"), "there should be an entry to go forward to");
        assert!(
            page.extract().await.expect("extract").status.url.ends_with("tall.html"),
            "forward should land on the second fixture"
        );

        assert!(!page.forward().await.expect("forward"), "there is nothing further forward");
    });
}

#[test]
fn reload_keeps_the_same_url() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        page.reload().await.expect("reload");
        assert!(page.extract().await.expect("extract").status.url.ends_with("simple.html"));
    });
}

/// Paint an extraction into a frame and return it as lines of text. This is
/// the ASCII art that makes snapshot diffs readable in review.
fn snapshot(extraction: &wwt_page::Extraction) -> String {
    let vp = viewport();
    let mut frame = wwt_frame::Frame::new(vp.grid());
    for run in &extraction.runs {
        frame.paint_run(&vp, run);
    }
    (0..vp.grid().rows)
        .map(|row| frame.row_text(row))
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

#[test]
fn simple_page_matches_its_snapshot() {
    let h = harness();
    runtime().block_on(async {
        let extraction = open(&h, "simple.html").await.extract().await.expect("extract");
        let got = snapshot(&extraction);

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/simple.txt");
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("create the snapshot dir");
            std::fs::write(&path, format!("{got}\n")).expect("write the snapshot");
            return;
        }

        let want = std::fs::read_to_string(&path)
            .expect("missing snapshot; regenerate with UPDATE_SNAPSHOTS=1");
        assert_eq!(got, want.trim_end(), "the rendered page changed");
    });
}

/// Not an assertion — a measurement. Run with `--nocapture` and record the
/// number; it is the floor on how fast a scroll can feel.
#[test]
fn measure_extraction_of_a_heavy_page() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "heavy.html").await;

        // One warm pass, so the number is steady-state rather than first-run.
        page.extract().await.expect("extract");

        let start = std::time::Instant::now();
        let extraction = page.extract().await.expect("extract");
        let elapsed = start.elapsed();

        println!(
            "heavy.html: {} runs extracted in {elapsed:?}",
            extraction.runs.len()
        );
        assert!(!extraction.runs.is_empty());
    });
}

/// The fidelity test for the cheap read: whatever `status()` skips, it must
/// not skip an answer.
///
/// Both halves come from the same document at the same moment, so a field
/// the two disagree about is a field `status()` computes differently rather
/// than more cheaply, which is the only way this split can go wrong. Read on
/// a page that has been scrolled, or `scroll_y` agrees at zero for free and
/// the geometry half of the comparison proves nothing.
#[test]
fn a_status_read_says_what_an_extraction_says() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "tall.html").await;
        page.scroll_by(200.0, viewport()).await.expect("scroll");
        let moved = await_scroll_past(&page, 0.0).await;
        assert!(moved > 0.0, "the fixture has to have moved for this to test anything");

        let extraction = page.extract().await.expect("extract");
        let status = page.status().await.expect("status");

        assert_eq!(status, extraction.status);
    });
}

/// What the chrome costs on its own. Run with:
///
///     cargo test -p wwt-page --test extraction measure_status -- --nocapture
///
/// The number to read it against is `measure_extraction`, on the same
/// fixture in the same run: `heavy.html` is fifteen hundred paragraphs, and
/// a status read walks none of them. This is the whole case for a mode that
/// does not paint runs asking a different question, and it is a measurement
/// rather than an assertion for the reason every other one here is.
#[test]
fn measure_status_of_a_heavy_page() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "heavy.html").await;

        // One warm pass each, so both numbers are steady-state.
        page.extract().await.expect("extract");
        page.status().await.expect("status");

        let start = std::time::Instant::now();
        let extraction = page.extract().await.expect("extract");
        let extract_time = start.elapsed();

        let start = std::time::Instant::now();
        let status = page.status().await.expect("status");
        let status_time = start.elapsed();

        println!(
            "heavy.html: extract {} runs in {extract_time:?}, status in {status_time:?}",
            extraction.runs.len()
        );
        assert_eq!(status, extraction.status, "the cheap read still has to be right");
    });
}
