//! Asking a page for a picture of itself, repeatedly.
//!
//! Four calls and a predicate. Nothing here decodes anything: a frame's data
//! arrives base64 and leaves base64, which is why pixel mode costs no
//! dependency.

use anyhow::{Context, Result};
use serde_json::json;
use wwt_cdp::Event;

use crate::extract::Page;

/// The CDP event a picture arrives as.
///
/// Exposed so a caller can ask the cheap question first: one string compare
/// against the method, before iterating every page to ask whose it is.
pub const SCREENCAST_FRAME: &str = "Page.screencastFrame";

/// One picture of a page, on its way to the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreencastFrame {
    /// Base64 PNG, exactly as CDP sent it and exactly as the graphics
    /// protocol wants it.
    pub data: String,
    /// What the ack must quote back.
    ///
    /// CDP calls this field `sessionId` and it is not a CDP session id: it
    /// counts screencasts on one target. `wwt-cdp` already means something
    /// else by session, so this is the ack id.
    pub ack: i64,
}

impl Page {
    /// Start sending pictures of this page, no larger than the given size.
    ///
    /// PNG rather than JPEG: a lossy picture of text is the one thing a
    /// browser in a terminal must not produce, and both ends of this
    /// pipeline already speak PNG.
    ///
    /// The size is the caller's decision and not the viewport's, because
    /// a terminal without a graphics protocol wants a picture two orders
    /// of magnitude smaller and Chromium is better at scaling than we are.
    pub async fn start_screencast(&self, width: u32, height: u32) -> Result<()> {
        self.client()
            .call_on(
                self.session_id(),
                "Page.startScreencast",
                json!({
                    "format": "png",
                    "maxWidth": width,
                    "maxHeight": height,
                    "everyNthFrame": 1,
                }),
            )
            .await
            .context("start the screencast")?;
        Ok(())
    }

    pub async fn stop_screencast(&self) -> Result<()> {
        self.client()
            .call_on(self.session_id(), "Page.stopScreencast", json!({}))
            .await
            .context("stop the screencast")?;
        Ok(())
    }

    /// Tell the page the frame arrived.
    ///
    /// Not optional and not batchable: Chromium sends the next frame only
    /// after the last one is acked, so a dropped ack is a screencast that
    /// stops after exactly one picture.
    pub async fn ack_frame(&self, ack: i64) -> Result<()> {
        self.client()
            .call_on(
                self.session_id(),
                "Page.screencastFrameAck",
                json!({ "sessionId": ack }),
            )
            .await
            .context("ack the screencast frame")?;
        Ok(())
    }

    /// Whether a CDP event is a picture of this page, and what is in it.
    ///
    /// The session id is half the question, exactly as it is for the dirty
    /// signal: one browser serves every page and they all report on one
    /// subscription.
    pub fn screencast_frame(&self, event: &Event) -> Option<ScreencastFrame> {
        if event.session_id.as_deref() != Some(self.session_id())
            || event.method != SCREENCAST_FRAME
        {
            return None;
        }
        Some(ScreencastFrame {
            data: event.params["data"].as_str()?.to_string(),
            ack: event.params["sessionId"].as_i64()?,
        })
    }
}
