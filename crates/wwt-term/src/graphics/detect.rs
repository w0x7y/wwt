//! Whether this terminal can show an image, asked once.
//!
//! `CLAUDE.md` rejects `supports_keyboard_enhancement` for taking stdin for
//! up to two seconds on every run. The objection is the two seconds and
//! whose the timeout is, not the asking: this asks once, before raw mode and
//! before the first paint, and gives up after a window we choose.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use rustix::termios::{
    LocalModes, OptionalActions, SpecialCodeIndex, tcgetattr, tcsetattr,
};

use super::protocol::IMAGE_ID;

/// How long a terminal gets to answer.
///
/// Long enough for a local terminal and for one at the far end of an ssh
/// connection on a bad day, short enough that a terminal which will never
/// answer does not delay the first frame enough to notice. `VTIME` counts in
/// deciseconds, so this is the finest granularity the timeout can have.
pub const WINDOW: Duration = Duration::from_millis(100);

/// A 1x1 PNG, base64. The smallest thing that is a picture, sent only so a
/// terminal has something to say OK about.
const PROBE_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGO4Y6QPAAMrAT6QmmeVAAAAAElFTkSuQmCC";

/// Whether what came back is a terminal saying it can do this.
///
/// Separated from the reading so the decision can be tested with data. A
/// terminal that answers with an error implements the protocol but not for
/// this image, and for our purposes that is not support.
pub fn reply_is_support(reply: &str) -> bool {
    reply.contains(&format!("\x1b_Gi={IMAGE_ID}")) && reply.contains("OK")
}

/// Ask, and wait `timeout` for an answer. Silence is not support.
///
/// This is the only place outside the input pump that reads stdin, and it
/// runs before the pump exists. A terminal that is not a terminal, or one
/// whose settings cannot be changed, answers no without anything being sent.
pub fn query(timeout: Duration) -> bool {
    let stdin = std::io::stdin();
    let Ok(original) = tcgetattr(&stdin) else {
        // Not a tty. Nothing to ask and nobody to answer.
        return false;
    };

    // VMIN 0 with VTIME set is a read that returns empty when the time is up
    // rather than blocking forever. Without this the deadline below is never
    // reached: a terminal that will never answer would hang the startup.
    let mut raw = original.clone();
    raw.local_modes &= !(LocalModes::ICANON | LocalModes::ECHO);
    raw.special_codes[SpecialCodeIndex::VMIN] = 0;
    raw.special_codes[SpecialCodeIndex::VTIME] =
        u8::try_from(timeout.as_millis() / 100).unwrap_or(1).max(1);
    if tcsetattr(&stdin, OptionalActions::Now, &raw).is_err() {
        return false;
    }

    let supported = ask(&stdin, timeout);

    // Put the terminal back however the asking went. A session that left
    // echo off would be unusable in a way nothing else would explain.
    let _ = tcsetattr(&stdin, OptionalActions::Now, &original);
    supported
}

/// The asking itself, with the terminal already in a state that can time out.
fn ask(mut stdin: &std::io::Stdin, timeout: Duration) -> bool {
    let mut out = std::io::stdout();
    // q=0 rather than q=2: this is the one sequence whose reply we want.
    if write!(out, "\x1b_Gi={IMAGE_ID},a=q,f=100,t=d,m=0;{PROBE_PNG}\x1b\\").is_err()
        || out.flush().is_err()
    {
        return false;
    }

    let deadline = Instant::now() + timeout;
    let mut reply = String::new();
    let mut buf = [0u8; 64];
    while Instant::now() < deadline {
        match stdin.read(&mut buf) {
            // The read timed out. A terminal that has said nothing by now is
            // one that is not going to.
            Ok(0) => break,
            Ok(n) => {
                reply.push_str(&String::from_utf8_lossy(&buf[..n]));
                // The answer is terminated, so there is nothing more coming
                // and no reason to wait out the rest of the window.
                if reply.contains("\x1b\\") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    reply_is_support(&reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_saying_ok_can_do_this() {
        assert!(reply_is_support("\x1b_Gi=7829364;OK\x1b\\"));
    }

    #[test]
    fn silence_is_not_support() {
        assert!(!reply_is_support(""));
    }

    #[test]
    fn an_answer_about_someone_elses_image_is_not_ours() {
        assert!(!reply_is_support("\x1b_Gi=42;OK\x1b\\"));
    }

    #[test]
    fn an_error_reply_is_not_support() {
        assert!(!reply_is_support(
            "\x1b_Gi=7829364;ENOTSUPPORTED:whatever\x1b\\"
        ));
    }

    #[test]
    fn asking_where_there_is_no_terminal_answers_no() {
        // Tests do not have a tty, so this exercises the path a redirected
        // stdout takes: nothing is written and nothing is waited for.
        assert!(!query(Duration::from_millis(1)));
    }
}
