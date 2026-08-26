use std::io::{BufWriter, Write, stdout};
use std::process::Command;
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

const HELP: &str = "Usage: wwt [OPTIONS] [URL OR SEARCH]\n\n\
Options:\n  \
  --new          Start without restoring the saved session\n  \
-h, --help     Print help\n  \
-V, --version  Print version\n";

#[tokio::main]
async fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments
        .first()
        .is_some_and(|argument| argument == "--launch")
    {
        return launch_in_terminal(&arguments[1..]);
    }
    if arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "-h" | "--help"))
    {
        print!("{HELP}");
        return Ok(());
    }
    if arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "-V" | "--version"))
    {
        println!("wwt {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let (new_session, argument) = parse_args_from(arguments)?;

    // Before the argument is interpreted: a word that is not a URL is a
    // search, and where a search goes is one of the things this file
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

/// Start the interactive process inside the terminal selected for desktop
/// launches. Direct command-line invocations never pass through here.
fn launch_in_terminal(arguments: &[String]) -> Result<()> {
    let (config, _) = wwt::config::load(wwt::store::config_path().as_deref());
    let (terminal, terminal_arguments) = config
        .terminal
        .split_first()
        .context("terminal command is empty")?;
    let executable = std::env::current_exe().context("find the wwt executable")?;

    Command::new(terminal)
        .args(terminal_arguments)
        .arg(executable)
        .args(arguments)
        .spawn()
        .with_context(|| format!("launch terminal {terminal}"))?;
    Ok(())
}

/// `wwt`, `wwt <url-or-search>`, `wwt --new [url-or-search]`.
fn parse_args_from(arguments: impl IntoIterator<Item = String>) -> Result<(bool, Option<String>)> {
    let mut new_session = false;
    let mut target = Vec::new();
    for argument in arguments {
        match argument.as_str() {
            "--new" => new_session = true,
            "-h" | "--help" => bail!("usage: wwt [--new] [url]"),
            other if other.starts_with('-') => bail!("unknown option: {other}"),
            other => target.push(other.to_string()),
        }
    }
    let target = (!target.is_empty()).then(|| target.join(" "));
    Ok((new_session, target))
}

#[cfg(test)]
mod tests {
    use super::parse_args_from;

    #[test]
    fn unquoted_words_are_one_search_phrase() {
        let arguments = ["rust", "terminal", "browser"].map(String::from);

        assert_eq!(
            parse_args_from(arguments).expect("parse arguments"),
            (false, Some("rust terminal browser".to_string()))
        );
    }

    #[test]
    fn login_is_not_a_command_line_option() {
        let error = parse_args_from(["--login".to_string()])
            .expect_err("login belongs to WWT's command line");
        assert_eq!(error.to_string(), "unknown option: --login");
    }
}
