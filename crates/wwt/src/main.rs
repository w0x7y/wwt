use std::io::{Write, stdout};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use wwt_cdp::{Chromium, Client};
use wwt_page::Page;
use wwt_ui::command::normalize_url;

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
    let vp = wwt::session::page_viewport(grid, cell);
    let page = Arc::new(Page::open(Arc::clone(&client), &url, vp).await?);

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, cursor::Hide)?;
    // Its own call, because a terminal that refuses mouse capture is still a
    // terminal you can read with. Bundling it with the alternate screen
    // would make one refusal cost the whole session.
    let mouse = execute!(stdout(), EnableMouseCapture).is_ok();

    let mut core = Core::new(page, client, grid, cell);
    if !mouse {
        core.notice("mouse unavailable");
    }
    let mut out = stdout();
    let result = core.run(&mut out).await;
    let _ = out.flush();

    // The renderer sets the cursor to a bar while a field is focused, so
    // hand the terminal back the shape it had.
    write!(stdout(), "\x1b[0 q")?;
    execute!(stdout(), cursor::Show, DisableMouseCapture, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    result
}
