//! The real binary, driven through a real pseudoterminal.
//!
//! The implementation plan expected a shared PTY harness, but this repository
//! had none. `script` owns the terminal emulation here; this test only feeds
//! keys and observes bytes, so it does not duplicate `wwt-term`'s renderer or
//! maintain a second model of terminal state.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(20);

struct Pty {
    child: Child,
    input: ChildStdin,
    output: Arc<Mutex<Vec<u8>>>,
    _data: TempDir,
    _config: TempDir,
}

impl Pty {
    fn spawn(fixture: &str) -> Self {
        let binary = Path::new(env!("CARGO_BIN_EXE_wwt"));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture);
        let url = format!("file://{}", fixture.display());
        let command = format!(
            "stty cols 80 rows 24; exec {} --new {}",
            shell_quote(&binary.display().to_string()),
            shell_quote(&url)
        );
        let data = tempfile::tempdir().expect("a private data directory");
        let config = tempfile::tempdir().expect("a private config directory");
        let mut child = Command::new("script")
            .args(["-qefc", &command, "/dev/null"])
            .env("TERM", "xterm-256color")
            .env("XDG_DATA_HOME", data.path())
            .env("XDG_CONFIG_HOME", config.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("launch the binary in util-linux script's PTY");
        let input = child.stdin.take().expect("PTY input");
        let mut stdout = child.stdout.take().expect("PTY output");
        let output = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&output);
        thread::spawn(move || {
            let mut bytes = [0; 4096];
            while let Ok(read) = stdout.read(&mut bytes) {
                if read == 0 {
                    break;
                }
                captured
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .extend_from_slice(&bytes[..read]);
            }
        });

        Self {
            child,
            input,
            output,
            _data: data,
            _config: config,
        }
    }

    fn send(&mut self, keys: &[u8]) {
        self.input.write_all(keys).expect("send keys to the PTY");
        self.input.flush().expect("flush PTY input");
    }

    fn mark(&self) -> usize {
        self.output
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }

    fn wait_for(&self, start: usize, needle: &str) -> Vec<u8> {
        let deadline = Instant::now() + WAIT;
        loop {
            let output = self
                .output
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let tail = &output[start.min(output.len())..];
            if contains(tail, needle.as_bytes()) {
                return tail.to_vec();
            }
            if Instant::now() >= deadline {
                panic!(
                    "did not see {needle:?} after byte {start}; output was {:?}",
                    String::from_utf8_lossy(tail)
                );
            }
            drop(output);
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn quit(mut self) {
        self.send(b"q");
        let status = self.child.wait().expect("wait for wwt");
        assert!(status.success(), "wwt exited with {status}");
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn reader_flow_crosses_the_binary_and_pty_boundary() {
    let mut terminal = Pty::spawn("reader.html");
    terminal.wait_for(0, "PAGE TOP FURNITURE");

    let entered = terminal.mark();
    terminal.send(b"r");
    terminal.wait_for(entered, "[reading]");
    terminal.wait_for(entered, "PTY ARTICLE TITLE");
    let reader = terminal.wait_for(entered, "er]");
    assert!(contains(&reader, b"PTY ARTICLE TITLE"));
    assert!(
        !contains(&reader, b"PAGE TOP FURNITURE"),
        "reader repaint retained site furniture"
    );

    let scrolled = terminal.mark();
    terminal.send(b"  ");
    terminal.wait_for(scrolled, "FOLLOW READER DESTINATION");

    let followed = terminal.mark();
    terminal.send(b"fs");
    terminal.wait_for(followed, "PTY DESTINATION ARRIVED");
    terminal.quit();
}

#[test]
fn leaving_reader_restores_the_live_pages_rows_and_progress() {
    let mut terminal = Pty::spawn("reader.html");
    terminal.wait_for(0, "PAGE TOP FURNITURE");

    let page_end = terminal.mark();
    terminal.send(b"G");
    terminal.wait_for(page_end, "PAGE BOTTOM FURNITURE");

    let entered = terminal.mark();
    terminal.send(b"r");
    terminal.wait_for(entered, "PTY ARTICLE TITLE");
    terminal.wait_for(entered, "er]");
    let reader_scroll = terminal.mark();
    terminal.send(b"d");
    terminal.wait_for(reader_scroll, "Article line 14");

    let left = terminal.mark();
    terminal.send(b"r");
    let restored = terminal.wait_for(left, "PAGE BOTTOM FURNITURE");
    assert!(
        contains(&restored, b"100"),
        "leaving reader did not repaint the page's 100-percent progress"
    );
    terminal.quit();
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
