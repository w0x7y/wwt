//! One page: navigate, size it to the terminal, pull its text runs out.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::json;
use tokio::time::{Duration, Instant, sleep};
use wb_cdp::Client;
use wb_frame::{CssRect, Style, TextRun, Viewport};

use crate::color::parse_css_color;

const BOOTSTRAP_JS: &str = include_str!("../assets/bootstrap.js");
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
const LOAD_POLL: Duration = Duration::from_millis(50);

/// The page-side function the injected script calls to say it changed.
/// Arrives back as a `Runtime.bindingCalled` event.
pub const DIRTY_BINDING: &str = "__webinal_dirty";

/// The shape `extract.js` returns.
#[derive(Debug, Deserialize)]
struct RawExtraction {
    runs: Vec<RawRun>,
    title: String,
    url: String,
    #[serde(rename = "scrollY")]
    scroll_y: f64,
    #[serde(rename = "scrollHeight")]
    scroll_height: f64,
    #[serde(rename = "innerHeight")]
    inner_height: f64,
}

/// One pass of the extraction script: everything the renderer and the
/// statusline need, from one round trip.
#[derive(Debug, Clone)]
pub struct Extraction {
    pub runs: Vec<TextRun>,
    pub title: String,
    pub url: String,
    pub scroll_y: f64,
    pub scroll_height: f64,
    pub viewport_height: f64,
}

impl Extraction {
    /// How far down the document we are: 0.0 at the top, 1.0 when the last
    /// line is on screen, and 0.0 when the document fits without scrolling.
    pub fn scroll_progress(&self) -> f64 {
        let scrollable = self.scroll_height - self.viewport_height;
        if scrollable <= 0.0 {
            return 0.0;
        }
        (self.scroll_y / scrollable).clamp(0.0, 1.0)
    }
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

pub struct Page {
    client: Arc<Client>,
    session_id: String,
}

impl Page {
    /// Create a target, size it to the viewport, navigate, and wait for load.
    pub async fn open(client: Arc<Client>, url: &str, vp: Viewport) -> Result<Page> {
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
        page.client
            .call_on(&page.session_id, "Page.enable", json!({}))
            .await
            .context("enable the Page domain")?;
        page.client
            .call_on(&page.session_id, "Runtime.enable", json!({}))
            .await
            .context("enable the Runtime domain")?;
        // Registered before the first navigation, so the binding exists by
        // the time the bootstrap runs.
        page.client
            .call_on(
                &page.session_id,
                "Runtime.addBinding",
                json!({ "name": DIRTY_BINDING }),
            )
            .await
            .context("install the dirty-signal binding")?;
        page.install_bootstrap().await?;
        page.set_viewport(vp).await?;
        page.navigate(url).await?;
        Ok(page)
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Install the page-side script for every document this target loads,
    /// including ones it navigates to later.
    async fn install_bootstrap(&self) -> Result<()> {
        self.client
            .call_on(
                &self.session_id,
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": BOOTSTRAP_JS }),
            )
            .await
            .context("install the bootstrap script")?;
        Ok(())
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

    /// Run the extraction script and convert its output.
    pub async fn extract(&self) -> Result<Extraction> {
        let result = self
            .client
            .call_on(
                &self.session_id,
                "Runtime.evaluate",
                json!({
                    "expression": "window.__webinal.extract()",
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

        Ok(Extraction {
            runs: raw
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
                .collect(),
            title: raw.title,
            url: raw.url,
            scroll_y: raw.scroll_y,
            scroll_height: raw.scroll_height,
            viewport_height: raw.inner_height,
        })
    }
}
