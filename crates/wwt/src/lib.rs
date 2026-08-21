//! Wiring: browser, page, frame.

pub mod core;
pub mod effect;
pub mod event;
pub mod input;
pub mod keys;
pub mod keymap;
pub mod session;

use std::sync::Arc;

use anyhow::{Context, Result};
use wwt_cdp::{Chromium, Client};
use wwt_frame::{Frame, Viewport};
use wwt_page::Page;

/// Launch a browser, render one URL, and return the resulting frame.
///
/// M1 tears the browser down on return. M4 replaces this with a session that
/// keeps the browser and its targets alive across navigations.
pub async fn render_url(url: &str, vp: Viewport) -> Result<Frame> {
    let browser = Chromium::launch(None).await.context("launch chromium")?;
    let client = Client::connect(browser.ws_url())
        .await
        .context("connect to chromium")?;
    let page = Page::open(Arc::new(client), url, vp).await?;

    let extraction = page.extract().await?;
    let mut frame = Frame::new(vp.grid());
    frame.paint_runs(&vp, &extraction.runs);
    Ok(frame)
}
