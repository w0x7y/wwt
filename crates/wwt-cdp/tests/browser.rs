//! These tests launch a real Chromium of their own, because launching and
//! connecting is what they are testing. Everything else that needs a
//! browser shares one; see `wwt-page/tests/common`.

use serde_json::json;
use wwt_cdp::{Chromium, Client};

#[tokio::test]
async fn launches_chromium_and_reports_its_version() {
    let browser = Chromium::launch(None).await.expect("launch chromium");
    let client = Client::connect(browser.ws_url()).await.expect("connect");

    let result = client
        .call("Browser.getVersion", json!({}))
        .await
        .expect("Browser.getVersion");

    let product = result["product"].as_str().expect("product string");
    assert!(
        product.contains("Chrome"),
        "unexpected product string: {product}"
    );
}

#[tokio::test]
async fn attaches_to_a_page_target_and_evaluates_javascript() {
    let browser = Chromium::launch(None).await.expect("launch chromium");
    let client = Client::connect(browser.ws_url()).await.expect("connect");

    let target = client
        .call("Target.createTarget", json!({ "url": "about:blank" }))
        .await
        .expect("createTarget");
    let target_id = target["targetId"].as_str().expect("targetId").to_string();

    let attached = client
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
        )
        .await
        .expect("attachToTarget");
    let session_id = attached["sessionId"].as_str().expect("sessionId").to_string();

    let evaluated = client
        .call_on(
            &session_id,
            "Runtime.evaluate",
            json!({ "expression": "6 * 7", "returnByValue": true }),
        )
        .await
        .expect("Runtime.evaluate");

    assert_eq!(evaluated["result"]["value"].as_i64(), Some(42));
}

#[tokio::test]
async fn a_failing_command_returns_an_error_rather_than_hanging() {
    let browser = Chromium::launch(None).await.expect("launch chromium");
    let client = Client::connect(browser.ws_url()).await.expect("connect");

    let err = client
        .call("Nonexistent.method", json!({}))
        .await
        .expect_err("an unknown method must be an error");

    assert!(
        err.to_string().contains("Nonexistent.method") || err.to_string().contains("not found"),
        "unhelpful error: {err}"
    );
}

/// The whole fallback in spec section 7 rests on this: Chromium refuses a
/// profile directory another Chromium is holding, so a second `wwt` needs no
/// lock file of ours to go stale after a crash. If this test ever fails, the
/// design is wrong and section 7 has to be rewritten around an explicit lock.
#[tokio::test]
async fn a_second_browser_cannot_have_a_profile_the_first_one_holds() {
    let profile = tempfile::tempdir().expect("a profile directory");

    let first = Chromium::launch(Some(profile.path()))
        .await
        .expect("the first browser takes the profile");

    let second = Chromium::launch(Some(profile.path())).await;
    assert!(
        second.is_err(),
        "a held profile must be refused, or the private-session fallback never triggers"
    );

    // Released on drop, so the next instance can have it.
    drop(first);
}

#[tokio::test]
async fn a_browser_with_no_profile_directory_gets_a_temporary_one() {
    let browser = Chromium::launch(None)
        .await
        .expect("launch on a temp profile");
    assert!(browser.ws_url().starts_with("ws://"));
}
