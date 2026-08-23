//! The fallback path, asserted on the same way the script path is: what
//! comes back is an `Extraction`, whatever produced it.

mod common;

use common::{harness, open, runtime, viewport};
use wwt_frame::TargetKind;

#[test]
fn a_snapshot_reads_the_text_on_screen() {
    let h = harness();
    runtime().block_on(async {
        let extraction =
            open(&h, "simple.html").await.snapshot(viewport()).await.expect("snapshot");

        let texts: Vec<&str> = extraction.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("Heading")), "runs were {texts:?}");
        assert!(texts.iter().any(|t| t.contains("First paragraph.")), "runs were {texts:?}");
        assert!(extraction.caret.is_none(), "a snapshot has no caret to offer");
    });
}

#[test]
fn a_snapshot_leaves_out_text_the_browser_does_not_show() {
    // The script never sees a `visibility: hidden` node; a snapshot reports
    // one with ordinary non-empty bounds, so culling it is this path's own
    // job and the reason `visibility` is one of the styles asked for.
    let h = harness();
    runtime().block_on(async {
        let extraction =
            open(&h, "simple.html").await.snapshot(viewport()).await.expect("snapshot");

        let texts: Vec<&str> = extraction.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(
            !texts.iter().any(|t| t.contains("Invisible text.")),
            "runs were {texts:?}"
        );
    });
}

#[test]
fn a_snapshot_carries_the_title_url_and_scroll_geometry() {
    let h = harness();
    runtime().block_on(async {
        // 200 lines of 20px, so it is four times the viewport.
        let extraction = open(&h, "tall.html").await.snapshot(viewport()).await.expect("snapshot");

        assert_eq!(extraction.title, "Tall Fixture");
        assert!(extraction.url.ends_with("tall.html"), "url was {}", extraction.url);
        assert_eq!(extraction.scroll_y, 0.0);
        assert!(
            extraction.scroll_height > extraction.viewport_height,
            "a 4000px page must be taller than the viewport: {} vs {}",
            extraction.scroll_height,
            extraction.viewport_height
        );
    });
}

#[test]
fn a_snapshot_positions_its_runs_where_the_script_does() {
    // The fidelity test, and the one that says the baseline rule and the
    // scroll-offset subtraction are right. Both paths on the same page,
    // compared by the cell each run lands in, which is what actually
    // reaches the screen: agreeing to the pixel is not required and
    // agreeing to the cell is.
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        let vp = viewport();
        let script = page.extract().await.expect("extract");
        let snapshot = page.snapshot(vp).await.expect("snapshot");

        for run in &script.runs {
            let text = run.text.trim();
            if text.is_empty() {
                continue;
            }
            let same = snapshot
                .runs
                .iter()
                .find(|other| other.text.trim() == text)
                .unwrap_or_else(|| {
                    panic!("the snapshot did not find {text:?} in {:?}", snapshot.runs)
                });
            assert_eq!(
                vp.row_of(run.baseline),
                vp.row_of(same.baseline),
                "{text:?} landed on different rows: script {} snapshot {}",
                run.baseline,
                same.baseline
            );
            assert_eq!(
                vp.col_of(run.rect.x),
                vp.col_of(same.rect.x),
                "{text:?} landed in different columns"
            );
        }
    });
}

#[test]
fn a_snapshot_reads_a_page_whose_script_is_broken() {
    // The reason the fallback exists, arranged the way this repo arranges
    // a fixture: `eval` breaks the page's idea of our script, and the
    // assertion is on what `snapshot` returns anyway.
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        page.eval("window.__wwt.extract = () => { throw new Error('broken') }")
            .await
            .expect("break the script");

        assert!(
            page.extract().await.is_err(),
            "the script must be broken for this test to mean anything"
        );

        let extraction = page.snapshot(viewport()).await.expect("snapshot");
        let texts: Vec<&str> = extraction.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("Heading")), "runs were {texts:?}");
    });
}

#[test]
fn a_snapshot_leaves_out_what_is_below_the_viewport() {
    // The snapshot is the whole document, so culling is ours. Without it a
    // long page paints two hundred runs into a frame with room for
    // twenty-three of them.
    let h = harness();
    runtime().block_on(async {
        let extraction = open(&h, "tall.html").await.snapshot(viewport()).await.expect("snapshot");

        let texts: Vec<&str> = extraction.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("line 0")), "runs were {texts:?}");
        assert!(
            !texts.iter().any(|t| t.contains("line 199")),
            "the bottom of a 4000px page is not on screen: {texts:?}"
        );
    });
}

#[test]
fn a_snapshot_shows_what_is_typed_in_a_field() {
    // A control's value is not in the DOM: `input.childNodes` is empty
    // however much you type, so no text box can carry it. `eval` arranges
    // the value the way a keystroke would; the assertion is on what the
    // extraction says.
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "fields.html").await;
        page.eval("document.getElementById('typed').value = 'typed in'")
            .await
            .expect("type into the field");

        let extraction = page.snapshot(viewport()).await.expect("snapshot");
        let texts: Vec<&str> = extraction.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("typed in")), "runs were {texts:?}");
    });
}

#[test]
fn a_snapshot_shows_a_placeholder_for_an_empty_field_and_bullets_for_a_password() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "fields.html").await;
        page.eval("document.getElementById('secret').value = 'hunter2'")
            .await
            .expect("set a password");

        let extraction = page.snapshot(viewport()).await.expect("snapshot");
        let texts: Vec<&str> = extraction.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(
            texts.iter().any(|t| t.contains("search the web")),
            "an empty field shows its placeholder: {texts:?}"
        );
        assert!(texts.iter().any(|t| t.contains("\u{2022}\u{2022}\u{2022}")), "runs were {texts:?}");
        assert!(
            !texts.iter().any(|t| t.contains("hunter2")),
            "a password must never be painted: {texts:?}"
        );
    });
}

#[test]
fn a_snapshot_finds_the_things_worth_hinting() {
    let h = harness();
    runtime().block_on(async {
        let targets =
            open(&h, "interactive.html").await.snapshot_hints(viewport()).await.expect("hints");

        assert!(
            targets.iter().any(|t| t.kind == TargetKind::Editable),
            "the input is hintable and entering insert mode is what a hint on it does: {targets:?}"
        );
        assert!(
            targets.iter().filter(|t| t.kind == TargetKind::Clickable).count() >= 2,
            "the link and the button: {targets:?}"
        );
        assert!(targets.iter().all(|t| t.rect.w > 0.0 && t.rect.h > 0.0), "{targets:?}");
    });
}

#[test]
fn hints_from_a_snapshot_leave_out_what_has_no_box_and_what_is_off_screen() {
    // `display:none` has no layout box at all, so it never reaches us.
    // The one 3000px down is culled here, the same way runs are.
    //
    // The covered link is deliberately NOT excluded: the script hit-tests
    // a candidate and a snapshot has nothing to hit test with, so this
    // path labels it. A spurious label costs a keystroke, and the
    // alternative costs a round trip per candidate.
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "interactive.html").await;
        let snapshot = page.snapshot_hints(viewport()).await.expect("snapshot hints");
        let script = page.hints().await.expect("script hints");

        let height = f64::from(viewport().css_height());
        assert!(
            snapshot.iter().all(|t| t.rect.y < height),
            "nothing off screen, and the viewport is {height} tall: {snapshot:?}"
        );
        assert!(
            snapshot.len() >= script.len(),
            "the snapshot cannot exclude a covered link, so it finds at least as many: \
             snapshot {snapshot:?} script {script:?}"
        );
    });
}

/// What reading a page the degraded way costs. Run with:
///
///     cargo test -p wwt-page --test snapshot measure_snapshot -- --nocapture
///
/// `heavy.html` is fifteen hundred paragraphs of which a dozen are on
/// screen. The script path costs ~4ms because it stops measuring what
/// nobody can see; a snapshot is the whole document and cannot. This test
/// exists to make the gap a fact rather than a guess, and open question 2
/// of the M6 spec is decided against the number it prints.
#[test]
fn measure_snapshot_of_a_heavy_page() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "heavy.html").await;
        let vp = viewport();

        // One warm pass each, so both numbers are steady-state rather than
        // first-run, which is what `measure_extraction` does and the only
        // way the two are comparable.
        page.snapshot(vp).await.expect("snapshot");
        page.extract().await.expect("extract");

        let start = std::time::Instant::now();
        let snapshot = page.snapshot(vp).await.expect("snapshot");
        let snapshot_time = start.elapsed();

        let start = std::time::Instant::now();
        let script = page.extract().await.expect("extract");
        let script_time = start.elapsed();

        eprintln!(
            "heavy.html: snapshot {} runs in {snapshot_time:?}, script {} runs in {script_time:?}",
            snapshot.runs.len(),
            script.runs.len()
        );
    });
}
