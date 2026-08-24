use std::io::{BufWriter, Write, stdout};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use wwt_cdp::{Chromium, Client};
use wwt_ui::command::normalize_url;

use wwt::core::{Core, Startup};

/// Room for a frame of a large terminal without the buffer filling mid-paint.
/// Overrunning it is only an extra syscall, never a wrong frame.
const FRAME_BUFFER: usize = 256 * 1024;

#[tokio::main]
async fn main() -> Result<()> {
    let (new_session, argument) = parse_args()?;

    // Before the argument is interpreted: a word that is not a URL is a
    // search, and where a search goes is one of the three things this file
    // decides.
    let (config, complaints) = wwt::config::load(wwt::store::config_path().as_deref());

    let url = match argument {
        Some(argument) => Some(
            normalize_url(&argument, &config.search).map_err(|message| anyhow::anyhow!(message))?,
        ),
        None => None,
    };

    let (grid, cell) = wwt_term::probe().context("measure the terminal")?;

    // Everything that can fail loudly happens before we touch the terminal,
    // so a failure leaves the user's screen exactly as it was.
    //
    // The profile is the lock. Chromium refuses a user-data-dir another
    // Chromium holds, so a second wwt needs no lock file of ours to go stale
    // after a crash: it gets a temporary profile, is told so, and writes no
    // session file. The instance holding the profile owns that file.
    let profile = wwt::store::profile_path();
    let (browser, private) = match profile.as_deref() {
        Some(path) => match Chromium::launch(Some(path), config.chromium.as_deref()).await {
            Ok(browser) => (browser, false),
            Err(_) => (
                Chromium::launch(None, config.chromium.as_deref())
                    .await
                    .context("launch chromium")?,
                true,
            ),
        },
        None => (
            Chromium::launch(None, config.chromium.as_deref())
                .await
                .context("launch chromium")?,
            true,
        ),
    };
    // What a relaunch should launch onto. `None` for a private session, so
    // the replacement gets a fresh temporary profile rather than trying to
    // take the one another wwt is holding: the fallback is the same decision
    // made twice, and it has to be made the same way both times.
    let relaunch_profile = (!private).then(|| profile.clone()).flatten();

    let client = Arc::new(
        Client::connect(browser.ws_url())
            .await
            .context("connect to chromium")?,
    );
    // Before the first target exists, or it races the setting. Every page
    // takes its session from this, adopted and asked for alike.
    client
        .auto_attach()
        .await
        .context("watch for tabs the page opens")?;

    let session_file = (!private).then(wwt::store::session_path).flatten();
    let (snapshot, session_error) = match (&session_file, new_session) {
        (Some(path), false) => match wwt::store::load(path) {
            Ok(snapshot) => (snapshot, None),
            Err(message) => (None, Some(message)),
        },
        _ => (None, None),
    };

    // Asked before raw mode, before the alternate screen and before the
    // first paint: the one moment stdin belongs to nobody. After the input
    // pump exists, the terminal's reply would arrive as a keystroke.
    let graphics = wwt_term::graphics::detect::query(wwt_term::graphics::detect::WINDOW);

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, cursor::Hide)?;
    // Its own call, because a terminal that refuses mouse capture is still a
    // terminal you can read with. Bundling it with the alternate screen
    // would make one refusal cost the whole session.
    let mouse = execute!(stdout(), EnableMouseCapture).is_ok();

    let mut core = Core::new(
        client,
        Startup {
            grid,
            cell,
            snapshot,
            open: url,
            session_file,
            graphics,
            config: config.clone(),
            // The browser is `Core`'s from here: the thing that restarts one
            // has to hold it, and it is dropped when the loop ends exactly
            // as it was when it was a local of `main`.
            browser,
            profile: relaunch_profile,
        },
    );
    // The statusline holds one notice, so the last of these is the one you
    // see: least worth knowing first. A typo in a config file matters less
    // than a mouse you cannot use, and both matter less than a session you
    // cannot save.
    if let Some(complaint) = complaints.first() {
        core.notice(&format!("config.toml: {complaint}"));
    }
    if !mouse {
        core.notice("mouse unavailable");
    }
    if private {
        core.notice("private session: another wwt has the profile");
    } else if let Some(message) = session_error {
        core.notice(&format!("session file: {message}"));
    }
    // `stdout()` is a `LineWriter`, so a full repaint's `\r\n` between rows
    // costs a write syscall each: forty on a forty-row terminal, for one
    // frame. Buffered, the same frame is three.
    let mut out = BufWriter::with_capacity(FRAME_BUFFER, stdout());
    let result = core.run(&mut out).await;
    let _ = out.flush();

    // The renderer sets the cursor to a bar while a field is focused, so
    // hand the terminal back the shape it had.
    write!(stdout(), "\x1b[0 q")?;
    execute!(stdout(), cursor::Show, DisableMouseCapture, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    result
}

/// `wwt`, `wwt <url>`, `wwt --new [url]`. Hand-rolled, because the whole
/// surface is one flag.
fn parse_args() -> Result<(bool, Option<String>)> {
    let mut new_session = false;
    let mut url = None;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--new" => new_session = true,
            "-h" | "--help" => bail!("usage: wwt [--new] [url]"),
            other if other.starts_with('-') => bail!("unknown option: {other}"),
            other => url = Some(other.to_string()),
        }
    }
    Ok((new_session, url))
}
