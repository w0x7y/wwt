//! Starting a Chromium and finding its websocket endpoint.

use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Context, Result, anyhow, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{Duration, timeout};

const CANDIDATES: &[&str] = &["chromium", "chromium-browser", "google-chrome-stable"];
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);

/// Locate a Chromium binary. `WWT_CHROMIUM` wins if set.
///
/// We never download a browser; an absent one is a clear error with an
/// actionable message, per spec section 8.
pub fn find_chromium() -> Result<PathBuf> {
    if let Ok(explicit) = std::env::var("WWT_CHROMIUM") {
        let path = PathBuf::from(&explicit);
        if !path.is_file() {
            bail!("WWT_CHROMIUM is set to {explicit}, which is not a file");
        }
        return Ok(path);
    }

    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for name in CANDIDATES {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err(anyhow!(
        "no Chromium found. Install one (`sudo pacman -S chromium`) or set \
         WWT_CHROMIUM to the absolute path of a Chromium binary."
    ))
}

/// A running headless Chromium. Killed on drop.
pub struct Chromium {
    child: Child,
    ws_url: String,
    /// Held so a temporary profile outlives the browser. `None` when the
    /// profile is a directory the caller owns and expects to survive us.
    _profile: Option<tempfile::TempDir>,
}

impl Chromium {
    /// Launch a browser on `profile`, or on a temporary directory when it is
    /// `None`.
    ///
    /// Where a persistent profile lives is the binary's business, not this
    /// crate's: `wwt-cdp` launches browsers and has no opinion about the
    /// user's data directory.
    ///
    /// A profile another Chromium already holds is refused by Chromium
    /// itself, which exits without announcing an endpoint, so this returns an
    /// error rather than a second browser sharing a cookie jar. That is the
    /// whole of the locking in spec section 7.
    pub async fn launch(profile: Option<&std::path::Path>) -> Result<Self> {
        let binary = find_chromium()?;
        let temporary = match profile {
            Some(_) => None,
            None => Some(tempfile::tempdir().context("create a temporary profile directory")?),
        };
        let dir = match (profile, &temporary) {
            (Some(path), _) => path.to_path_buf(),
            (None, Some(temp)) => temp.path().to_path_buf(),
            (None, None) => unreachable!("a temporary profile is created when none is given"),
        };

        let mut child = Command::new(&binary)
            .arg("--headless=new")
            // Port 0 lets the OS pick; we read the real one back off stderr.
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", dir.display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-gpu")
            // Headless still paces frame production at the display's rate,
            // and a scroll is not visible to the page until the frame it
            // lands on. That cap was two thirds of the latency between
            // pressing `j` and having the text: 32ms to 21ms without it,
            // and to 5ms once the scroll signal stopped trailing too. An
            // idle page produces no frames, so it costs nothing to uncap.
            .arg("--disable-frame-rate-limit")
            .arg("about:blank")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start {}", binary.display()))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("chromium stderr was not piped"))?;

        let ws_url = timeout(STARTUP_TIMEOUT, read_ws_url(stderr))
            .await
            .map_err(|_| anyhow!("chromium did not report a debugging endpoint within 20s"))??;

        Ok(Self {
            child,
            ws_url,
            _profile: temporary,
        })
    }

    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }
}

impl Drop for Chromium {
    fn drop(&mut self) {
        // kill_on_drop handles the process; start_kill makes it prompt.
        let _ = self.child.start_kill();
    }
}

/// Chromium announces its endpoint on stderr as
/// `DevTools listening on ws://127.0.0.1:PORT/devtools/browser/UUID`.
async fn read_ws_url(stderr: tokio::process::ChildStderr) -> Result<String> {
    let mut lines = BufReader::new(stderr).lines();
    while let Some(line) = lines.next_line().await? {
        if let Some(idx) = line.find("ws://") {
            return Ok(line[idx..].trim().to_string());
        }
    }
    bail!("chromium exited before announcing a debugging endpoint")
}
