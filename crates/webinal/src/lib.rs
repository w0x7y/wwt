//! Wiring: browser, page, frame.

use std::sync::Arc;

use anyhow::{Context, Result};
use wb_cdp::{Chromium, Client};
use wb_frame::{Frame, Viewport};
use wb_page::Page;

/// Launch a browser, render one URL, and return the resulting frame.
///
/// M1 tears the browser down on return. M4 replaces this with a session that
/// keeps the browser and its targets alive across navigations.
pub async fn render_url(url: &str, vp: Viewport) -> Result<Frame> {
    let browser = Chromium::launch().await.context("launch chromium")?;
    let client = Client::connect(browser.ws_url())
        .await
        .context("connect to chromium")?;
    let page = Page::open(Arc::new(client), url, vp).await?;

    let extraction = page.extract().await?;
    let mut frame = Frame::new(vp.grid());
    for run in &extraction.runs {
        frame.paint_run(&vp, run);
    }
    Ok(frame)
}
