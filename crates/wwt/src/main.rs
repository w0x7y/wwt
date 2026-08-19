use std::io::{Write, stdout};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use wwt_cdp::{Chromium, Client};
use wwt_page::Page;
use wwt::command::normalize_url;
use wwt::core::Core;

#[tokio::main]
async fn main() -> Result<()> {
    let Some(argument) = std::env::args().nth(1) else {
        bail!("usage: wwt <url>");
    };
    let url = normalize_url(&argument).map_err(|message| anyhow::anyhow!(message))?;

    let (grid, cell) = wwt_term::probe().context("measure the terminal")?;

    // Everything that can fail loudly happens before we touch the terminal,
    // so a failure leaves the user's screen exactly as it was.
    let browser = Chromium::launch().await.context("launch chromium")?;
    let client = Arc::new(
        Client::connect(browser.ws_url())
            .await
            .context("connect to chromium")?,
    );
    let vp = wwt::core::page_viewport(grid, cell);
    let page = Arc::new(Page::open(Arc::clone(&client), &url, vp).await?);

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, cursor::Hide)?;

    let mut core = Core::new(page, client, grid, cell);
    let mut out = stdout();
    let result = core.run(&mut out).await;
    let _ = out.flush();

    execute!(stdout(), cursor::Show, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    result
}
