//! One page: navigate, size it to the terminal, pull its text runs out.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use wwt_cdp::{Client, Event};
use wwt_frame::{CssRect, Style, TextRun, Viewport};

use crate::color::parse_css_color;
use crate::input::KeyInput;

const BOOTSTRAP_JS: &str = include_str!("../assets/bootstrap.js");
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// The page-side function the injected script calls to say it changed.
/// Arrives back as a `Runtime.bindingCalled` event.
pub const DIRTY_BINDING: &str = "__wwt_dirty";

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

    /// Navigate this page, and wait for its load event.
    pub async fn navigate(&self, url: &str) -> Result<()> {
        // Subscribe before issuing the command: the load event for a fast
        // page can arrive before the navigate response does.
        let mut events = self.client.subscribe();

        let result = self
            .client
            .call_on(&self.session_id, "Page.navigate", json!({ "url": url }))
            .await
            .with_context(|| format!("navigate to {url}"))?;

        if let Some(error) = result.get("errorText").and_then(|v| v.as_str()) {
            bail!("navigation to {url} failed: {error}");
        }

        self.wait_for_load(&mut events).await
    }

    async fn wait_for_load(&self, events: &mut mpsc::UnboundedReceiver<Event>) -> Result<()> {
        let watch = async {
            while let Some(event) = events.recv().await {
                if event.method == "Page.loadEventFired"
                    && event.session_id.as_deref() == Some(self.session_id.as_str())
                {
                    return Ok(());
                }
            }
            Err(anyhow!("the CDP connection closed while the page was loading"))
        };

        match timeout(LOAD_TIMEOUT, watch).await {
            Ok(result) => result,
            Err(_) => bail!("the page did not finish loading within {LOAD_TIMEOUT:?}"),
        }
    }

    /// Move `delta` entries through the browser's own history.
    ///
    /// Returns `false` when there is no such entry — the end of the history
    /// is a fact about the world, not an error.
    async fn go(&self, delta: i64) -> Result<bool> {
        let history = self
            .client
            .call_on(&self.session_id, "Page.getNavigationHistory", json!({}))
            .await
            .context("read the navigation history")?;

        let index = history["currentIndex"]
            .as_i64()
            .ok_or_else(|| anyhow!("the navigation history has no currentIndex"))?;
        let entries = history["entries"]
            .as_array()
            .ok_or_else(|| anyhow!("the navigation history has no entries"))?;

        let target = index + delta;
        if target < 0 || target >= entries.len() as i64 {
            return Ok(false);
        }
        let entry_id = entries[target as usize]["id"]
            .as_i64()
            .ok_or_else(|| anyhow!("a history entry has no id"))?;

        let mut events = self.client.subscribe();
        self.client
            .call_on(
                &self.session_id,
                "Page.navigateToHistoryEntry",
                json!({ "entryId": entry_id }),
            )
            .await
            .context("navigate to a history entry")?;
        self.wait_for_load(&mut events).await?;
        Ok(true)
    }

    pub async fn back(&self) -> Result<bool> {
        self.go(-1).await
    }

    pub async fn forward(&self) -> Result<bool> {
        self.go(1).await
    }

    pub async fn reload(&self) -> Result<()> {
        let mut events = self.client.subscribe();
        self.client
            .call_on(&self.session_id, "Page.reload", json!({}))
            .await
            .context("reload the page")?;
        self.wait_for_load(&mut events).await
    }

    /// Scroll by a distance in CSS pixels, positive being downward.
    ///
    /// This dispatches a real wheel event rather than calling `scrollBy`, so
    /// Chromium performs the scroll: sticky headers stick, infinite scroll
    /// loads, and virtualized lists virtualize, all with no help from us.
    pub async fn scroll_by(&self, dy: f64, vp: Viewport) -> Result<()> {
        self.client
            .call_on(
                &self.session_id,
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseWheel",
                    "x": f64::from(vp.css_width()) / 2.0,
                    "y": f64::from(vp.css_height()) / 2.0,
                    "deltaX": 0.0,
                    "deltaY": dy,
                    "button": "none",
                    "clickCount": 0,
                    "modifiers": 0,
                }),
            )
            .await
            .context("dispatch a wheel event")?;
        Ok(())
    }

    pub async fn scroll_to_top(&self) -> Result<()> {
        self.scroll_to("0").await
    }

    /// Jump to the end of the document.
    ///
    /// This is the one place M2 does not scroll natively: the distance to the
    /// document's end is not known to us, and on an infinite-scroll page it
    /// changes as we go. The consequence is that this reaches the end of what
    /// has loaded, which is the correct behavior — it is simply not
    /// wheel-driven.
    pub async fn scroll_to_end(&self) -> Result<()> {
        self.scroll_to("document.documentElement.scrollHeight").await
    }

    async fn scroll_to(&self, y_expression: &str) -> Result<()> {
        self.client
            .call_on(
                &self.session_id,
                "Runtime.evaluate",
                json!({
                    "expression": format!("window.scrollTo(0, {y_expression})"),
                    "returnByValue": true,
                }),
            )
            .await
            .context("scroll to a document position")?;
        Ok(())
    }

    /// Run the extraction script and convert its output.
    pub async fn extract(&self) -> Result<Extraction> {
        let result = self
            .client
            .call_on(
                &self.session_id,
                "Runtime.evaluate",
                json!({
                    "expression": "window.__wwt.extract()",
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
    /// Evaluate an expression in the page and return its value.
    ///
    /// This is the escape hatch the tests use to see what a keystroke did.
    /// It is deliberately not how anything in the browser reads the page:
    /// that is `extract`, once, in one round trip.
    pub async fn eval(&self, expression: &str) -> Result<serde_json::Value> {
        let result = self
            .client
            .call_on(
                &self.session_id,
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true }),
            )
            .await
            .with_context(|| format!("evaluate {expression}"))?;

        if let Some(details) = result.get("exceptionDetails") {
            bail!("{expression} threw: {details}");
        }
        Ok(result["result"]["value"].clone())
    }

    /// Send one key to the page.
    ///
    /// A key that inserts text dispatches `keyDown`, which Chromium turns
    /// into a character insertion. A key that inserts nothing dispatches
    /// `rawKeyDown`, which stays a bare key event. Sending the wrong one
    /// either loses your typing or types your shortcuts.
    pub async fn dispatch_key(&self, key: &KeyInput) -> Result<()> {
        let mut down = json!({
            "type": if key.text.is_empty() { "rawKeyDown" } else { "keyDown" },
            "key": key.key,
            "code": key.code,
            "windowsVirtualKeyCode": key.windows_virtual_key_code,
            "nativeVirtualKeyCode": key.windows_virtual_key_code,
            "modifiers": key.modifiers,
        });
        if !key.text.is_empty() {
            down["text"] = json!(key.text);
            down["unmodifiedText"] = json!(key.text);
        }

        self.client
            .call_on(&self.session_id, "Input.dispatchKeyEvent", down)
            .await
            .context("dispatch a key down")?;
        self.client
            .call_on(
                &self.session_id,
                "Input.dispatchKeyEvent",
                json!({
                    "type": "keyUp",
                    "key": key.key,
                    "code": key.code,
                    "windowsVirtualKeyCode": key.windows_virtual_key_code,
                    "nativeVirtualKeyCode": key.windows_virtual_key_code,
                    "modifiers": key.modifiers,
                }),
            )
            .await
            .context("dispatch a key up")?;
        Ok(())
    }

    /// Take focus off whatever has it.
    ///
    /// Leaving insert mode has to be local: if this fails, the mode changes
    /// anyway. Taking the keyboard back must not depend on the page.
    pub async fn blur(&self) -> Result<()> {
        self.eval("document.activeElement && document.activeElement.blur()")
            .await?;
        Ok(())
    }
}
