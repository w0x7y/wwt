//! Starting a Chromium and finding its websocket endpoint.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Context, Result, anyhow, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{Duration, timeout};

const CANDIDATES: &[&str] = &["chromium", "chromium-browser", "google-chrome-stable"];
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);

/// Arguments for a normal visible Chromium window.
///
/// Kept separate from the headless launcher so automation flags cannot leak
/// into the browser used for an interactive login.
fn visible_arguments(profile: &std::path::Path, url: &str) -> Vec<OsString> {
    vec![
        OsString::from(format!("--user-data-dir={}", profile.display())),
        OsString::from("--no-first-run"),
        OsString::from("--no-default-browser-check"),
        OsString::from(url),
    ]
}

/// Locate a Chromium binary. `WWT_CHROMIUM` wins, then `configured`, then
/// the `PATH`.
///
/// The environment beats the config file because it is the more specific
/// thing: a variable is set for one run and a file is written for all of
/// them.
///
/// We never download a browser; an absent one is a clear error with an
/// actionable message, per spec section 8.
pub fn find_chromium(configured: Option<&std::path::Path>) -> Result<PathBuf> {
    if let Ok(explicit) = std::env::var("WWT_CHROMIUM") {
        let path = PathBuf::from(&explicit);
        if !path.is_file() {
            bail!("WWT_CHROMIUM is set to {explicit}, which is not a file");
        }
        return Ok(path);
    }

    if let Some(path) = configured {
        if !path.is_file() {
            bail!("config.toml names {}, which is not a file", path.display());
        }
        return Ok(path.to_path_buf());
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
    /// Which binary is the caller's business too, for the same reason: the
    /// config file is read by the binary and this crate only launches what
    /// it is given.
    ///
    /// A profile another Chromium already holds is refused by Chromium
    /// itself, which exits without announcing an endpoint, so this returns an
    /// error rather than a second browser sharing a cookie jar. That is the
    /// whole of the locking in spec section 7.
    pub async fn launch(
        profile: Option<&std::path::Path>,
        binary: Option<&std::path::Path>,
    ) -> Result<Self> {
        let binary = find_chromium(binary)?;
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
            // Keep presentation off vblank for low-latency scrolling without
            // disabling begin-frame pacing for page animation and app work.
            .arg("--disable-gpu-vsync")
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

    /// Stop Chromium and wait until it has released its profile.
    pub async fn shutdown(mut self) -> Result<()> {
        if self
            .child
            .try_wait()
            .context("check whether Chromium has exited")?
            .is_none()
        {
            self.child.start_kill().context("stop Chromium")?;
        }
        self.child.wait().await.context("wait for Chromium to stop")?;
        Ok(())
    }
}

impl Drop for Chromium {
    fn drop(&mut self) {
        // kill_on_drop handles the process; start_kill makes it prompt.
        let _ = self.child.start_kill();
    }
}

/// An ordinary visible Chromium process with no automation interface.
pub struct VisibleChromium {
    child: Child,
}

impl VisibleChromium {
    pub fn launch(
        profile: &std::path::Path,
        binary: Option<&std::path::Path>,
        url: &str,
    ) -> Result<Self> {
        let binary = find_chromium(binary)?;
        let child = Command::new(&binary)
            .args(visible_arguments(profile, url))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start visible {}", binary.display()))?;
        Ok(Self { child })
    }

    /// Wait until the user closes the visible browser window.
    pub async fn wait(self) -> Result<()> {
        let output = self
            .child
            .wait_with_output()
            .await
            .context("wait for visible Chromium")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            if stderr.is_empty() {
                bail!("visible Chromium exited with {}", output.status);
            }
            bail!("visible Chromium exited with {}: {stderr}", output.status);
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::visible_arguments;

    #[test]
    fn a_login_window_uses_the_profile_without_automation_flags() {
        let url = "https://accounts.google.com/";
        let arguments = visible_arguments(Path::new("/tmp/wwt profile"), url);
        let arguments: Vec<_> = arguments
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect();

        assert!(arguments.iter().any(|argument| argument == "--user-data-dir=/tmp/wwt profile"));
        assert!(arguments.iter().any(|argument| argument == url));
        assert!(!arguments.iter().any(|argument| argument.contains("headless")));
        assert!(!arguments.iter().any(|argument| argument.contains("remote-debugging")));
    }
}
