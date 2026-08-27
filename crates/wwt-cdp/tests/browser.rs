//! These tests launch a real Chromium of their own, because launching and
//! connecting is what they are testing. Everything else that needs a
//! browser shares one; see `wwt-page/tests/common`.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use serde_json::json;
use wwt_cdp::{Chromium, Client, VisibleChromium};

fn executable(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("fake-chromium");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake Chromium");
    let mut permissions = fs::metadata(&path).expect("fake metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("make fake executable");
    path
}

#[tokio::test]
async fn a_visible_browser_uses_the_profile_without_automation_flags() {
    let fixture = tempfile::tempdir().expect("a fixture directory");
    let profile = fixture.path().join("profile with spaces");
    let binary = executable(
        fixture.path(),
        r#"printf '%s\n' "$@" > "${0}.args""#,
    );

    VisibleChromium::launch(&profile, Some(&binary), "https://accounts.google.com/")
        .expect("launch visible Chromium")
        .wait()
        .await
        .expect("visible Chromium exits cleanly");

    let arguments = fs::read_to_string(format!("{}.args", binary.display()))
        .expect("captured arguments");
    assert!(arguments.lines().any(|argument| {
        argument == format!("--user-data-dir={}", profile.display())
    }));
    assert!(arguments.lines().any(|argument| argument == "https://accounts.google.com/"));
    assert!(!arguments.lines().any(|argument| argument.contains("headless")));
    assert!(!arguments.lines().any(|argument| argument.contains("remote-debugging")));
}

#[tokio::test]
async fn a_visible_browser_with_a_failing_exit_reports_the_status() {
    let fixture = tempfile::tempdir().expect("a fixture directory");
    let profile = fixture.path().join("profile");
    let binary = executable(fixture.path(), "echo 'profile is locked' >&2\nexit 23");

    let error = VisibleChromium::launch(&profile, Some(&binary), "https://accounts.google.com/")
        .expect("launch visible Chromium")
        .wait()
        .await
        .expect_err("a failing browser exit must be reported");

    assert!(
        error.to_string().contains("23"),
        "the process status should be actionable: {error:#}"
    );
    assert!(
        error.to_string().contains("profile is locked"),
        "Chromium's error should survive: {error:#}"
    );
}

#[tokio::test]
async fn a_headless_browser_preserves_normal_frame_pacing() {
    let fixture = tempfile::tempdir().expect("a fixture directory");
    let profile = fixture.path().join("profile");
    fs::create_dir(&profile).expect("create profile");
    let binary = executable(
        fixture.path(),
        r#"printf '%s\n' "$@" > "${0}.args"
echo 'DevTools listening on ws://127.0.0.1:9222/devtools/browser/test' >&2
sleep 30"#,
    );

    let browser = Chromium::launch(Some(&profile), Some(&binary))
        .await
        .expect("launch headless Chromium");
    let arguments = fs::read_to_string(format!("{}.args", binary.display()))
        .expect("captured arguments");

    assert!(
        !arguments.lines().any(|argument| argument == "--disable-frame-rate-limit"),
        "unpaced animation frames can starve browser application work"
    );
    assert!(
        arguments.lines().any(|argument| argument == "--disable-gpu-vsync"),
        "presentation should not wait for vblank before reporting a scroll"
    );
    drop(browser);
}

#[tokio::test]
async fn launches_chromium_and_reports_its_version() {
    let browser = Chromium::launch(None, None).await.expect("launch chromium");
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
    let browser = Chromium::launch(None, None).await.expect("launch chromium");
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
    let browser = Chromium::launch(None, None).await.expect("launch chromium");
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

    let first = Chromium::launch(Some(profile.path()), None)
        .await
        .expect("the first browser takes the profile");

    let second = Chromium::launch(Some(profile.path()), None).await;
    assert!(
        second.is_err(),
        "a held profile must be refused, or the private-session fallback never triggers"
    );

    // Released on drop, so the next instance can have it.
    drop(first);
}

#[tokio::test]
async fn an_awaited_shutdown_releases_the_profile_before_it_returns() {
    let profile = tempfile::tempdir().expect("a profile directory");
    let first = Chromium::launch(Some(profile.path()), None)
        .await
        .expect("the first browser takes the profile");

    first.shutdown().await.expect("stop the first browser");

    let second = Chromium::launch(Some(profile.path()), None)
        .await
        .expect("the profile is available when shutdown returns");
    drop(second);
}

#[tokio::test]
async fn a_browser_with_no_profile_directory_gets_a_temporary_one() {
    let browser = Chromium::launch(None, None)
        .await
        .expect("launch on a temp profile");
    assert!(browser.ws_url().starts_with("ws://"));
}

/// The stalled rule rests on a timeout being tellable apart from a page
/// whose script threw, and a unit test can only prove that for a socket
/// nobody is listening to. This proves it for a real browser that is
/// answering, just not that fast.
#[tokio::test]
async fn a_page_that_will_not_answer_produces_a_timeout_and_not_a_refusal() {
    let browser = Chromium::launch(None, None).await.expect("launch chromium");
    let client = Client::connect(browser.ws_url()).await.expect("connect");
    // A deadline no round trip can meet. `call_with` is what makes the wait
    // bearable in a test.
    let error = client
        .call_with(
            "Browser.getVersion",
            json!({}),
            std::time::Duration::from_nanos(1),
        )
        .await
        .expect_err("a nanosecond is not enough for a round trip");
    assert!(
        error.downcast_ref::<wwt_cdp::TimedOut>().is_some(),
        "a deadline is a kind of failure and not a message about one: {error:?}"
    );
}
