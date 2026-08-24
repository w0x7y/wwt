//! The half of the restart path that needs a real browser. The rules it
//! serves are unit tests in `session.rs`, which need none.

use wwt_cdp::Chromium;

/// A relaunch produces a browser that actually works: connected, attached,
/// and able to open a page. Asserting it returned `Ok` would pass on a
/// client whose websocket was already closed.
#[tokio::test]
async fn a_browser_killed_is_replaced_by_one_that_can_open_a_page() {
    let profile = tempfile::tempdir().expect("a profile to relaunch onto");

    let first = Chromium::launch(Some(profile.path()), None)
        .await
        .expect("launch chromium");
    // Dropped, which is what a relaunch does first: kill_on_drop is what
    // releases the profile lock, and Chromium refuses a user-data-dir
    // another Chromium holds.
    drop(first);

    let (browser, client) = wwt::core::relaunch(Some(profile.path()), None)
        .await
        .expect("a replacement browser");

    let vp = wwt::session::page_viewport(
        wwt_frame::GridSize { cols: 80, rows: 24 },
        wwt_frame::CellSize { w: 9, h: 20 },
    );
    let page = wwt_page::Page::open(client, "about:blank", vp)
        .await
        .expect("the replacement browser opens pages");
    page.extract().await.expect("and answers our script");
    drop(browser);
}

/// The profile is the lock, so a relaunch that begins before the old
/// browser is gone must fail rather than quietly land on a temporary
/// profile and lose the cookie jar.
///
/// It costs the whole backoff before it fails, which is the price of
/// asserting the behaviour rather than the shape.
#[tokio::test]
async fn a_relaunch_onto_a_held_profile_fails_rather_than_going_private() {
    let profile = tempfile::tempdir().expect("a profile");
    let held = Chromium::launch(Some(profile.path()), None)
        .await
        .expect("launch chromium");

    let result = wwt::core::relaunch(Some(profile.path()), None).await;
    assert!(
        result.is_err(),
        "a relaunch that cannot have the profile is a failed attempt, not a \
         private session: the cookie jar is the reason for holding one"
    );
    drop(held);
}
