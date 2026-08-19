//! One page: navigate, size it to the terminal, pull its text runs out.

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::json;
use tokio::time::{Duration, Instant, sleep};
use wb_cdp::Client;
use wb_frame::{CssRect, Style, TextRun, Viewport};

use crate::color::parse_css_color;

const EXTRACT_JS: &str = include_str!("../assets/extract.js");
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
const LOAD_POLL: Duration = Duration::from_millis(50);

/// The shape `extract.js` returns.
#[derive(Debug, Deserialize)]
struct RawExtraction {
    runs: Vec<RawRun>,
}

#[derive(Debug, Deserialize)]
struct RawRun {
    text: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    baseline: f64,
    color: String,
    bold: bool,
    z: i32,
}

pub struct Page<'a> {
    client: &'a Client,
    session_id: String,
}

impl<'a> Page<'a> {
    /// Create a target, size it to the viewport, navigate, and wait for load.
    pub async fn open(client: &'a Client, url: &str, vp: Viewport) -> Result<Page<'a>> {
        let target = client
            .call("Target.createTarget", json!({ "url": "about:blank" }))
            .await
            .context("create a page target")?;
        let target_id = target["targetId"]
            .as_str()
            .ok_or_else(|| anyhow!("Target.createTarget returned no targetId"))?
            .to_string();

        let attached = client
            .call(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
            )
            .await
            .context("attach to the page target")?;
        let session_id = attached["sessionId"]
            .as_str()
            .ok_or_else(|| anyhow!("Target.attachToTarget returned no sessionId"))?
            .to_string();

        let page = Page { client, session_id };
        page.set_viewport(vp).await?;
        page.navigate(url).await?;
        Ok(page)
    }

    /// Tell Chromium the window is exactly the terminal grid. Spec section 3.
    pub async fn set_viewport(&self, vp: Viewport) -> Result<()> {
        self.client
            .call_on(
                &self.session_id,
                "Emulation.setDeviceMetricsOverride",
                json!({
                    "width": vp.css_width(),
                    "height": vp.css_height(),
                    "deviceScaleFactor": 1,
                    "mobile": false,
                }),
            )
            .await
            .context("set the device metrics override")?;
        Ok(())
    }

    async fn navigate(&self, url: &str) -> Result<()> {
        self.client
            .call_on(&self.session_id, "Page.enable", json!({}))
            .await
            .context("enable the Page domain")?;

        let result = self
            .client
            .call_on(&self.session_id, "Page.navigate", json!({ "url": url }))
            .await
            .with_context(|| format!("navigate to {url}"))?;

        if let Some(error) = result.get("errorText").and_then(|v| v.as_str()) {
            bail!("navigation to {url} failed: {error}");
        }

        self.wait_for_load().await
    }

    /// M1 polls `document.readyState`. M2 replaces this with the
    /// `Page.loadEventFired` event once the CDP event pump exists.
    async fn wait_for_load(&self) -> Result<()> {
        let deadline = Instant::now() + LOAD_TIMEOUT;
        loop {
            let state = self
                .client
                .call_on(
                    &self.session_id,
                    "Runtime.evaluate",
                    json!({ "expression": "document.readyState", "returnByValue": true }),
                )
                .await
                .context("poll document.readyState")?;

            if state["result"]["value"].as_str() == Some("complete") {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("the page did not finish loading within {LOAD_TIMEOUT:?}");
            }
            sleep(LOAD_POLL).await;
        }
    }

    pub async fn title(&self) -> Result<String> {
        let result = self
            .client
            .call_on(
                &self.session_id,
                "Runtime.evaluate",
                json!({ "expression": "document.title", "returnByValue": true }),
            )
            .await
            .context("read document.title")?;
        Ok(result["result"]["value"].as_str().unwrap_or_default().to_string())
    }

    /// Run the extraction script and convert its output into `TextRun`s.
    pub async fn extract(&self) -> Result<Vec<TextRun>> {
        let result = self
            .client
            .call_on(
                &self.session_id,
                "Runtime.evaluate",
                json!({
                    "expression": EXTRACT_JS,
                    "returnByValue": true,
                    "awaitPromise": false,
                }),
            )
            .await
            .context("run the extraction script")?;

        if let Some(details) = result.get("exceptionDetails") {
            bail!("the extraction script threw: {details}");
        }

        let raw: RawExtraction = serde_json::from_value(result["result"]["value"].clone())
            .context("the extraction script returned an unexpected shape")?;

        Ok(raw
            .runs
            .into_iter()
            .map(|r| TextRun {
                text: r.text,
                rect: CssRect { x: r.x, y: r.y, w: r.w, h: r.h },
                baseline: r.baseline,
                style: Style {
                    fg: parse_css_color(&r.color),
                    bold: r.bold,
                    reverse: false,
                },
                z: r.z,
            })
            .collect())
    }
}
