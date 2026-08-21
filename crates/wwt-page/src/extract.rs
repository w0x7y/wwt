//! One page: navigate, size it to the terminal, pull its text runs out.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use wwt_cdp::{Attached, Client, Event, TargetId};
use wwt_frame::{Caret, CssPoint, CssRect, HintTarget, Style, TargetKind, TextRun, Viewport};

use crate::color::parse_css_color;
use crate::input::{Input, KeyInput, MouseAction, MouseInput};

const BOOTSTRAP_JS: &str = include_str!("../assets/bootstrap.js");
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// The page-side function the injected script calls to say it changed.
/// Arrives back as a `Runtime.bindingCalled` event.
pub const DIRTY_BINDING: &str = "__wwt_dirty";

/// The shape `extract.js` returns.
#[derive(Debug, Deserialize)]
struct RawExtraction {
    runs: Vec<RawRun>,
    caret: Option<RawCaret>,
    title: String,
    url: String,
    #[serde(rename = "scrollY")]
    scroll_y: f64,
    #[serde(rename = "scrollHeight")]
    scroll_height: f64,
    #[serde(rename = "innerHeight")]
    inner_height: f64,
}

/// The insertion point, as the injected script measured it.
#[derive(Debug, Deserialize)]
struct RawCaret {
    x: f64,
    baseline: f64,
    offset: usize,
}

/// One pass of the extraction script: everything the renderer and the
/// statusline need, from one round trip.
#[derive(Debug, Clone)]
pub struct Extraction {
    pub runs: Vec<TextRun>,
    /// Where typing would land, when a form control has focus.
    pub caret: Option<Caret>,
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

/// The shape one entry of `__wwt.hints()` returns.
#[derive(Debug, Deserialize)]
struct RawTarget {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    editable: bool,
}

pub struct Page {
    client: Arc<Client>,
    session_id: String,
    target_id: String,
}

/// Its identity and nothing else: a `Page` is a handle on a browser, and the
/// browser is not something to print.
impl std::fmt::Debug for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Page")
            .field("target", &self.target_id)
            .field("session", &self.session_id)
            .finish()
    }
}

impl Page {
    /// Create a target, prepare it, size it to the viewport, navigate, and
    /// wait for load.
    ///
    /// The session is not asked for. Auto-attach delivers one for every new
    /// target, so waiting for it here is what keeps a tab we opened and a tab
    /// a page opened on the same path, and the caller has to have turned it
    /// on. Subscribed before the create, because the attach for a fast target
    /// arrives before the create's own answer does.
    pub async fn open(client: Arc<Client>, url: &str, vp: Viewport) -> Result<Page> {
        let mut events = client.subscribe();
        let created = client
            .call("Target.createTarget", json!({ "url": "about:blank" }))
            .await
            .context("create a page target")?;
        let target = TargetId(
            created["targetId"]
                .as_str()
                .ok_or_else(|| anyhow!("Target.createTarget returned no targetId"))?
                .to_string(),
        );

        let attached = timeout(LOAD_TIMEOUT, async {
            loop {
                let event = events
                    .recv()
                    .await
                    .ok_or_else(|| anyhow!("the browser went away"))?;
                if let Some(attached) = Client::attached_page(&event)
                    && attached.target == target
                {
                    return Ok::<_, anyhow::Error>(attached);
                }
            }
        })
        .await
        .map_err(|_| anyhow!("the browser did not attach to the target it created"))??;

        let page = Page::prepare(client, attached, vp).await?;
        page.navigate(url).await?;
        Ok(page)
    }

    /// Take over a target a page opened for itself.
    ///
    /// A target we did not create has already started, and possibly
    /// finished, loading its document by the time the browser reports it, and
    /// `Page.addScriptToEvaluateOnNewDocument` only reaches documents that
    /// have not started. Registering it is still what covers the next
    /// document; this evaluates the same source into the one already there,
    /// so a tab arrives readable rather than blank until it navigates. The
    /// bootstrap returns early when it finds itself installed, so whichever
    /// of the two got there first, only one takes effect.
    pub async fn adopt(client: Arc<Client>, attached: Attached, vp: Viewport) -> Result<Page> {
        let page = Page::prepare(client, attached, vp).await?;
        page.js(BOOTSTRAP_JS)
            .await
            .context("install the bootstrap in the document the tab already loaded")?;
        Ok(page)
    }

    /// Everything a target needs before it is worth looking at.
    ///
    /// The order is load-bearing: the binding exists before the bootstrap
    /// that calls it, and the bootstrap is registered before the document
    /// that should contain it is navigated to.
    async fn prepare(client: Arc<Client>, attached: Attached, vp: Viewport) -> Result<Page> {
        let page = Page {
            client,
            session_id: attached.session,
            target_id: attached.target.0,
        };
        page.client
            .call_on(&page.session_id, "Page.enable", json!({}))
            .await
            .context("enable the Page domain")?;
        page.client
            .call_on(&page.session_id, "Runtime.enable", json!({}))
            .await
            .context("enable the Runtime domain")?;
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
        Ok(page)
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    /// Make this page the one the browser has in front.
    ///
    /// `Input.dispatchMouseEvent` is answered by whichever target is
    /// foreground, so switching tabs without this would leave clicks landing
    /// on the page you just left. M5's screencast will want the same
    /// guarantee.
    pub async fn activate(&self) -> Result<()> {
        self.client
            .call(
                "Target.activateTarget",
                json!({ "targetId": self.target_id }),
            )
            .await
            .context("activate the target")?;
        Ok(())
    }

    /// Close this page's target.
    ///
    /// Browser-level rather than `call_on`: a session cannot outlive the
    /// target it is attached to, so asking the target to close itself races
    /// its own answer.
    pub async fn close(&self) -> Result<()> {
        self.client
            .call("Target.closeTarget", json!({ "targetId": self.target_id }))
            .await
            .context("close the target")?;
        Ok(())
    }

    /// Whether a CDP event is this page's dirty signal.
    ///
    /// One browser serves several pages, and every one of them reports on
    /// the same subscription, so the session id is half the question. This
    /// lives here because the binding name does.
    pub fn is_dirty(&self, event: &Event) -> bool {
        event.session_id.as_deref() == Some(self.session_id.as_str())
            && event.method == "Runtime.bindingCalled"
            && event.params["name"] == DIRTY_BINDING
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
        // Aimed at the middle of the viewport, because a keyboard scroll has
        // no pointer to aim with.
        let at = CssPoint {
            x: f64::from(vp.css_width()) / 2.0,
            y: f64::from(vp.css_height()) / 2.0,
        };
        self.dispatch_mouse(&MouseInput::wheel(at, dy)).await
    }

    pub async fn scroll_to_top(&self) -> Result<()> {
        self.scroll_to_expression("0").await
    }

    /// Jump to the end of the document.
    ///
    /// This is the one place M2 does not scroll natively: the distance to the
    /// document's end is not known to us, and on an infinite-scroll page it
    /// changes as we go. The consequence is that this reaches the end of what
    /// has loaded, which is the correct behavior — it is simply not
    /// wheel-driven.
    pub async fn scroll_to_end(&self) -> Result<()> {
        self.scroll_to_expression("document.documentElement.scrollHeight")
            .await
    }

    /// Put the document at an absolute offset.
    ///
    /// Restoring a scroll position, and nothing else. A wheel event would be
    /// the wrong tool: we know exactly where the page should be, and letting
    /// Chromium animate its way there would mean the extraction after it
    /// reads a position on the way rather than the one asked for.
    pub async fn scroll_to(&self, y: f64) -> Result<()> {
        self.scroll_to_expression(&y.to_string()).await
    }

    async fn scroll_to_expression(&self, y_expression: &str) -> Result<()> {
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
        let value = self
            .js("window.__wwt.extract()")
            .await
            .context("run the extraction script")?;
        let raw: RawExtraction = serde_json::from_value(value)
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
            caret: raw
                .caret
                .map(|c| Caret { x: c.x, baseline: c.baseline, offset: c.offset }),
            title: raw.title,
            url: raw.url,
            scroll_y: raw.scroll_y,
            scroll_height: raw.scroll_height,
            viewport_height: raw.inner_height,
        })
    }
    /// Run JavaScript in the page, for a test.
    ///
    /// Gated, because it has no business in what a caller must know: a
    /// caller who can run arbitrary JavaScript can read the page any way it
    /// likes, and `extract` exists so that there is exactly one way. Nothing
    /// in the browser calls this.
    ///
    /// Tests use it to *arrange* — focus a field, set a value, move the
    /// insertion point — and to reach `__wwt.__pure`. What they assert on is
    /// what this crate returns, so a change that keeps the DOM right and the
    /// `Extraction` wrong still fails.
    #[cfg(feature = "test-support")]
    pub async fn eval(&self, expression: &str) -> Result<serde_json::Value> {
        self.js(expression).await
    }

    /// Evaluate an expression in the page and return its value.
    ///
    /// Deliberately not how anything reads the page: that is `extract`,
    /// once, in one round trip. The two callers here are commands this crate
    /// issues rather than reads it performs.
    async fn js(&self, expression: &str) -> Result<serde_json::Value> {
        let mut result = self
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
        Ok(result["result"]["value"].take())
    }

    /// Send one input to the page.
    ///
    /// The two kinds go to different CDP commands, and which one is this
    /// crate's business rather than its caller's.
    pub async fn dispatch(&self, input: &Input) -> Result<()> {
        match input {
            Input::Key(key) => self.dispatch_key(key).await,
            Input::Mouse(mouse) => self.dispatch_mouse(mouse).await,
        }
    }

    /// Send one key to the page.
    ///
    /// A key that inserts text dispatches `keyDown`, which Chromium turns
    /// into a character insertion. A key that inserts nothing dispatches
    /// `rawKeyDown`, which stays a bare key event. Sending the wrong one
    /// either loses your typing or types your shortcuts.
    pub async fn dispatch_key(&self, key: &KeyInput) -> Result<()> {
        // The same key, going down and coming back up. Describing it twice
        // is how the two come to disagree.
        let key_fields = json!({
            "key": key.key,
            "code": key.code,
            "windowsVirtualKeyCode": key.windows_virtual_key_code,
            "nativeVirtualKeyCode": key.windows_virtual_key_code,
            "modifiers": key.modifiers,
        });

        let mut down = key_fields.clone();
        down["type"] = json!(if key.text.is_empty() { "rawKeyDown" } else { "keyDown" });
        if !key.text.is_empty() {
            down["text"] = json!(key.text);
            down["unmodifiedText"] = json!(key.text);
        }

        let mut up = key_fields;
        up["type"] = json!("keyUp");

        self.client
            .call_on(&self.session_id, "Input.dispatchKeyEvent", down)
            .await
            .context("dispatch a key down")?;
        self.client
            .call_on(&self.session_id, "Input.dispatchKeyEvent", up)
            .await
            .context("dispatch a key up")?;
        Ok(())
    }

    /// Take focus off whatever has it.
    ///
    /// Leaving insert mode has to be local: if this fails, the mode changes
    /// anyway. Taking the keyboard back must not depend on the page.
    pub async fn blur(&self) -> Result<()> {
        self.js("document.activeElement && document.activeElement.blur()")
            .await?;
        Ok(())
    }
    /// Send one mouse event to the page.
    ///
    /// The point is the page's, not the terminal's: the caller converts
    /// through `Viewport`, which is the only place a cell becomes a pixel.
    pub async fn dispatch_mouse(&self, mouse: &MouseInput) -> Result<()> {
        let params = match mouse.action {
            MouseAction::Press => json!({
                "type": "mousePressed",
                "x": mouse.at.x,
                "y": mouse.at.y,
                "button": "left",
                "buttons": 1,
                "clickCount": 1,
                "modifiers": 0,
            }),
            MouseAction::Release => json!({
                "type": "mouseReleased",
                "x": mouse.at.x,
                "y": mouse.at.y,
                "button": "left",
                "buttons": 0,
                "clickCount": 1,
                "modifiers": 0,
            }),
            MouseAction::Wheel(dy) => json!({
                "type": "mouseWheel",
                "x": mouse.at.x,
                "y": mouse.at.y,
                "deltaX": 0.0,
                "deltaY": dy,
                "button": "none",
                "clickCount": 0,
                "modifiers": 0,
            }),
        };

        self.client
            .call_on(&self.session_id, "Input.dispatchMouseEvent", params)
            .await
            .context("dispatch a mouse event")?;
        Ok(())
    }
    /// Every interactive box on screen, in document order.
    ///
    /// Run when hint mode opens rather than during extraction: it sweeps the
    /// document and hit-tests each candidate, which is too much to pay on
    /// every scroll frame for something pressed occasionally.
    pub async fn hints(&self) -> Result<Vec<HintTarget>> {
        let value = self.js("window.__wwt.hints()").await.context("run the hint query")?;
        let raw: Vec<RawTarget> = serde_json::from_value(value)
            .context("the hint query returned an unexpected shape")?;

        Ok(raw
            .into_iter()
            .map(|t| HintTarget {
                rect: CssRect { x: t.x, y: t.y, w: t.w, h: t.h },
                kind: if t.editable { TargetKind::Editable } else { TargetKind::Clickable },
            })
            .collect())
    }
}
