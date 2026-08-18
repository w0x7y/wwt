use std::io::{Write, stdout};

use anyhow::{Context, Result, bail};
use crossterm::event::{Event, KeyCode, KeyEvent, read};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use wb_frame::Viewport;

#[tokio::main]
async fn main() -> Result<()> {
    let Some(url) = std::env::args().nth(1) else {
        bail!("usage: webinal <url>");
    };

    let (grid, cell) = wb_term::probe().context("measure the terminal")?;
    let vp = Viewport::new(grid, cell);

    // Render before touching the terminal, so a failure leaves the user's
    // screen exactly as it was.
    let frame = webinal::render_url(&url, vp).await?;

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, cursor::Hide)?;

    let result = run(&frame);

    execute!(stdout(), cursor::Show, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    result
}

fn run(frame: &wb_frame::Frame) -> Result<()> {
    let mut out = stdout();
    wb_term::render(frame, &mut out)?;
    out.flush()?;

    loop {
        if let Event::Key(KeyEvent { code, .. }) = read()?
            && matches!(code, KeyCode::Char('q') | KeyCode::Esc)
        {
            return Ok(());
        }
    }
}
