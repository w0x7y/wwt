//! The fallback path, asserted on the same way the script path is: what
//! comes back is an `Extraction`, whatever produced it.

mod common;

use common::{harness, open, runtime, viewport};

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
