//! These tests launch a real Chromium. They are the only tests in the
//! workspace that need one.

use serde_json::json;
use wm_cdp::{Chromium, Client};

#[tokio::test]
async fn launches_chromium_and_reports_its_version() {
    let browser = Chromium::launch().await.expect("launch chromium");
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
    let browser = Chromium::launch().await.expect("launch chromium");
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
    let browser = Chromium::launch().await.expect("launch chromium");
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
