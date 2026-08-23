# wwt M6 — Degradation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep the browser working when a piece of it does not: a page that breaks the injected script is still read and still clickable, and a terminal with no graphics protocol shows the page in half-block colour rather than refusing.

**Architecture:** Two halves that share no code. The fallback half gives `Effect::Extract` and `Effect::Hints` a `Source`, adds `Page::snapshot()` sourced from `DOMSnapshot.captureSnapshot`, and puts the rule that reaches for it in `Session` where a test needs no browser. The half-block half adds `wwt-png`, a pure crate that decodes base64 and PNG and nothing else, a background colour on `Style`, and `Frame::paint_samples`, so a degraded picture is ordinary cells and the diffing renderer, the overlays and the cursor are untouched.

**Tech Stack:** Rust 2024, tokio, tokio-tungstenite, crossterm, serde/serde_json, anyhow. Chromium as an external process, driven over the hand-rolled CDP client. No image or compression crate: `wwt-png` is written here, which is the whole reason it is its own crate.

**Spec:** `docs/superpowers/specs/2026-08-23-wwt-m6-design.md` — read it in full before starting. Its parent, `docs/superpowers/specs/2026-08-19-wwt-design.md`, governs where the two disagree; sections 3, 8 and 11 of the parent are the relevant ones, and section 9 of this plan's spec lists the four places M6 amends them. Those amendments are written by Task 13, not before.

## Global Constraints

- Rust edition **2024**. `cargo clippy --workspace --all-targets -- -D warnings` must be clean **per task**, not per plan.
- **Do not add a workspace dependency.** The set in `Cargo.toml` is fixed. `wwt-png` is a new workspace *member*, which is not the same thing and is the only new member this plan adds. If a task seems to need a crate from outside, stop and ask.
- `wwt-frame` and `wwt-png` have the same hard rule: **no I/O, no dependencies.** Not serde, not anyhow. They take and return plain Rust.
- Unit tests in `src/` must run without Chromium. Anything needing a browser goes in `tests/`.
- Test names are sentences describing the property: `a_stored_block_inflates_to_itself`, not `test_inflate_1`.
- Comments explain **why**, in prose, where the reason is not obvious. Do not restate code.
- Commits are conventional with a crate scope: `feat(png):`, `fix(wwt):`, `perf(term):`. Behaviour discovered during implementation goes in the commit body.
- **No em-dashes** in prose. The spec titles use one; sentences do not.
- Never blank the frame you are looking at (parent spec §8). Every failure path in this plan degrades to stale-but-labeled.

## Baseline

Before Task 1, confirm the tree is green so that anything this plan breaks is this plan's:

```bash
cargo test --workspace          # 396 tests pass
cargo clippy --workspace --all-targets -- -D warnings
```

M5's measurements are the numbers M6 must not move. Record them now:

```bash
cargo test -p wwt-page --test extraction measure_extraction -- --nocapture   # ~3.7ms
cargo test -p wwt-page --test interaction measure_scroll_latency -- --nocapture  # ~5.4ms
cargo test -p wwt --lib measure_switch -- --nocapture                        # ~134µs
cargo test -p wwt-term --lib measure_pixel_frame -- --nocapture              # ~414µs
```

## File structure

```
crates/wwt-png/                        NEW CRATE. Pure. No I/O, no dependencies.
  Cargo.toml
  src/lib.rs                           `decode`, `Png`, `Error`. The only public surface.
  src/base64.rs                        base64 -> bytes. M5 never needed this.
  src/inflate.rs                       Bit reader, canonical Huffman, the three block types.
  src/png.rs                           IHDR, IDAT, unfilter. Refuses what Chromium never sends.
  tests/fixtures/                      PNGs Chromium itself produced, checked in by Task 1.

crates/wwt-frame/src/cell.rs           `Style::bg`.
crates/wwt-frame/src/samples.rs        NEW. `Samples`, and the box filter that resizes into one.
crates/wwt-frame/src/frame.rs          `Frame::paint_samples`.

crates/wwt-term/src/render.rs          `push_style` writes a background when a cell has one.

crates/wwt-page/src/snapshot.rs        NEW. `Page::snapshot()`: DOMSnapshot -> `Extraction`.
crates/wwt-page/src/hints.rs           `Page::snapshot_hints()` beside the script's.
crates/wwt-page/src/screencast.rs      `start_screencast` takes the size to ask for.
crates/wwt-page/tests/snapshot.rs      NEW. Both paths on the same page, asserted equal.

crates/wwt-ui/src/chrome.rs            The `[degraded]` tag.

crates/wwt/src/effect.rs               `Source`, and the two effects that carry it.
crates/wwt/src/event.rs                `Job::Extracted` carries a `Result`.
crates/wwt/src/tab.rs                  `Tab::degraded`.
crates/wwt/src/session.rs              The degrade rule, `Picture`, half-block compose.
crates/wwt/src/core.rs                 Spawns whichever source the effect names.
```

The two halves touch disjoint files except `session.rs`, which both change. Tasks 2 through 8 are half-block, tasks 9 through 12 are the fallback, and either order works; they are written half-block first because its first three tasks need no browser at all and are the cheapest place to find out whether the milestone's one piece of real algorithm works.

---

### Task 1: The two probes

Two things this milestone rests on are assumptions until a real Chromium answers them, and M5 is the cautionary tale: three claims about the graphics protocol survived review and died on contact with a real terminal. Both probes are throwaway. What survives is a recorded answer in the spec and two fixture files.

**Files:**
- Create (temporarily): `crates/wwt-page/tests/probe.rs`
- Create (permanently): `crates/wwt-png/tests/fixtures/screencast.png`, `crates/wwt-png/tests/fixtures/screencast.txt`
- Modify: `docs/superpowers/specs/2026-08-23-wwt-m6-design.md` (open question 1)

**Interfaces:**
- Consumes: `wwt_page::Page`, and the test harness in `crates/wwt-page/tests/common`.
- Produces: the baseline rule Task 9 implements, and the two fixtures Task 4 asserts against.

- [x] **Step 1: Read how a page test is arranged**

```bash
sed -n '1,80p' crates/wwt-page/tests/common/mod.rs
sed -n '1,40p' crates/wwt-page/tests/extraction.rs
```

Every test binary launches one Chromium and hands it out a test at a time, because `Input.dispatchMouseEvent` is answered by whichever target the browser has in front. Follow whatever `extraction.rs` does to get a `Page`; do not invent a second way.

- [x] **Step 2: Write the probe**

Create `crates/wwt-page/tests/probe.rs`. It is a test so that it gets the harness for
free, and it prints rather than asserts, because its output is the deliverable.
`Harness::client` is public, which is how it reaches CDP commands `Page` has no method
for.

```rust
//! Throwaway. Answers two questions M6 rests on and is deleted in step 5.

mod common;

use common::{harness, open, runtime};
use serde_json::json;

/// What `DOMSnapshot.captureSnapshot` actually returns, and whether a text
/// box is the tight box or the full line box.
///
/// The second question decides whether a snapshot run's baseline is the
/// box bottom or its centre, which is open question 1 of the M6 spec.
#[test]
fn probe_what_a_snapshot_returns() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        // A 48px line box around 16px of text, so the two answers cannot be
        // confused with each other.
        page.eval("document.querySelector('p').style.font = '16px/3 monospace'")
            .await
            .expect("set a tall line height");

        let snapshot = h
            .client
            .call_on(
                page.session_id(),
                "DOMSnapshot.captureSnapshot",
                json!({
                    "computedStyles": ["color", "font-weight"],
                    "includePaintOrder": true,
                    "includeDOMRects": false,
                }),
            )
            .await
            .expect("capture a snapshot");

        let doc = &snapshot["documents"][0];
        let keys = |v: &serde_json::Value| {
            v.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()
        };
        eprintln!("document keys: {:?}", keys(doc));
        eprintln!("nodes keys:    {:?}", keys(&doc["nodes"]));
        eprintln!("layout keys:   {:?}", keys(&doc["layout"]));
        eprintln!("textBox keys:  {:?}", keys(&doc["textBoxes"]));
        eprintln!("textBoxes:     {}", doc["textBoxes"]);
        eprintln!("layout.bounds: {}", doc["layout"]["bounds"]);
        eprintln!("layout.text:   {}", doc["layout"]["text"]);
        eprintln!("scrollOffsetY: {}", doc["scrollOffsetY"]);
        eprintln!("contentHeight: {}", doc["contentHeight"]);
        eprintln!("title:         {}", doc["title"]);
        eprintln!("documentURL:   {}", doc["documentURL"]);
        eprintln!("isClickable:   {}", doc["nodes"]["isClickable"]);
        eprintln!("inputValue:    {}", doc["nodes"]["inputValue"]);
        eprintln!("BASELINE ANSWER: a textBox height near 48 is the line box, near 18 the tight box.");
    });
}

/// What Chromium's screencast PNG actually is, and a fixture of one.
///
/// `wwt-png` decodes what this prints and refuses everything else, so this
/// is what decides its scope. Colour type 6 is RGBA, 2 is RGB, 0 is grey.
#[test]
fn probe_what_a_screencast_frame_is() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        page.eval("document.body.style.background = 'rgb(255,0,0)'")
            .await
            .expect("paint the page red");

        let shot = h
            .client
            .call_on(
                page.session_id(),
                "Page.captureScreenshot",
                json!({ "format": "png", "captureBeyondViewport": false }),
            )
            .await
            .expect("capture a screenshot");
        let data = shot["data"].as_str().expect("base64 data").to_string();

        // Hand-rolled, because this probe cannot add a dependency either
        // and `wwt-png::base64` does not exist yet.
        let bytes = probe_base64(&data);
        eprintln!("png bytes: {}", bytes.len());
        eprintln!("signature: {:02x?}", &bytes[..8]);
        // IHDR is always the first chunk: 4 length, 4 type, then the fields.
        eprintln!("width:     {}", u32::from_be_bytes(bytes[16..20].try_into().unwrap()));
        eprintln!("height:    {}", u32::from_be_bytes(bytes[20..24].try_into().unwrap()));
        eprintln!("bit depth: {}", bytes[24]);
        eprintln!("colour:    {}", bytes[25]);
        eprintln!("compress:  {}", bytes[26]);
        eprintln!("filter:    {}", bytes[27]);
        eprintln!("interlace: {}", bytes[28]);

        std::fs::create_dir_all("../wwt-png/tests/fixtures").expect("fixture dir");
        std::fs::write("../wwt-png/tests/fixtures/screencast.png", &bytes).expect("write png");
        std::fs::write("../wwt-png/tests/fixtures/screencast.txt", &data).expect("write base64");
        eprintln!("fixtures written");
    });
}

fn probe_base64(s: &str) -> Vec<u8> {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut have = 0u32;
    for b in s.bytes().filter(|b| *b != b'=' && !b.is_ascii_whitespace()) {
        let v = A.iter().position(|c| *c == b).expect("base64 alphabet") as u32;
        acc = (acc << 6) | v;
        have += 6;
        if have >= 8 {
            have -= 8;
            out.push((acc >> have) as u8);
        }
    }
    out
}
```

The fixture paths are relative to `crates/wwt-page`, which is where a test's working
directory is. Create `crates/wwt-png/tests/fixtures/` before running if the `create_dir_all`
above is not enough.

- [x] **Step 3: Run both probes and read the answers**

```bash
cargo test -p wwt-page --test probe -- --nocapture
```

Expected: both print. Write down, verbatim, for the next steps:

1. Whether `textBoxes[].bounds` heights are ~48 (line box) or ~18 (tight box).
2. The exact key names under `documents[0]`, `nodes` and `layout`.
3. The PNG's bit depth, colour type, compression, filter and interlace bytes.

- [x] **Step 4: Record the answers in the spec**

Rewrite open question 1 of `docs/superpowers/specs/2026-08-23-wwt-m6-design.md` as closed, in the style M5 used: `~~**Title.**~~ **Closed, 2026-08-23.**` followed by what was measured and what it forces. If the answer is "line box", also change the baseline sentence in section 3 from the box bottom to the box centre, since that is the rule the rest of the plan implements.

If the PNG is anything other than 8-bit, colour type 6 or 2, non-interlaced, deflate-compressed with filter method 0, say so there too: Task 4's scope is whatever the probe printed, and a decoder for a format Chromium does not send is untested code.

- [x] **Step 5: Delete the probe, keep the fixtures**

```bash
rm crates/wwt-page/tests/probe.rs
git status --short   # the two fixtures under crates/wwt-png/tests/fixtures must remain
```

- [x] **Step 6: Commit**

```bash
git add docs/superpowers/specs/2026-08-23-wwt-m6-design.md crates/wwt-png/tests/fixtures
git commit -m "docs(spec): ask a real browser what a snapshot and a frame are

Closes open question 1 with what Chromium answered rather than with what
the protocol definition implies, and checks in the screencast PNG that
wwt-png is written against. A decoder for a format that never arrives is
untested code, so the probe is what decides its scope."
```

---

### Task 2: `wwt-png`, and base64

The crate exists to hold one algorithm, and this task is the easy end of it. M5's whole economy was that a payload arrives base64 and leaves base64; half-block has to look inside, so this is the first thing wwt has ever needed to decode.

**Files:**
- Create: `crates/wwt-png/Cargo.toml`, `crates/wwt-png/src/lib.rs`, `crates/wwt-png/src/base64.rs`
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Produces: `wwt_png::Error`, `wwt_png::base64::decode(&str) -> Result<Vec<u8>, Error>`. Task 3 adds `inflate`, Task 4 adds `decode` and `Png`, and Task 8 is the only consumer of any of it.

- [x] **Step 1: Create the crate**

`crates/wwt-png/Cargo.toml`:

```toml
[package]
name = "wwt-png"
edition.workspace = true
version.workspace = true

[dependencies]
```

The empty `[dependencies]` is the point and stays empty. Add the member to the workspace in `Cargo.toml`:

```toml
members = ["crates/wwt-frame", "crates/wwt-png", "crates/wwt-term", "crates/wwt-cdp", "crates/wwt-page", "crates/wwt-ui", "crates/wwt"]
```

- [x] **Step 2: Write the failing tests**

`crates/wwt-png/src/base64.rs`:

```rust
//! base64 to bytes, and nothing else.
//!
//! wwt went five milestones without needing this: a screencast frame
//! arrives base64 and the graphics protocol wants it base64, so pixel mode
//! forwards the string it was given. Half-block has to look inside one.

use crate::Error;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_string_decodes_to_nothing() {
        assert_eq!(decode("").expect("empty"), Vec::<u8>::new());
    }

    #[test]
    fn each_padding_length_decodes_to_its_own_bytes() {
        // The three shapes a final group can take, which is where every
        // base64 decoder that is wrong is wrong.
        assert_eq!(decode("TWFu").expect("no padding"), b"Man");
        assert_eq!(decode("TWE=").expect("one pad"), b"Ma");
        assert_eq!(decode("TQ==").expect("two pads"), b"M");
    }

    #[test]
    fn every_byte_value_survives_a_round_trip() {
        // Encoded by hand rather than by a crate, since there is none: this
        // is the first 12 bytes 0..12 in base64.
        assert_eq!(decode("AAECAwQFBgcICQoL").expect("bytes"), (0u8..12).collect::<Vec<_>>());
    }

    #[test]
    fn the_last_two_alphabet_characters_are_plus_and_slash() {
        // Chromium sends standard base64, not the url-safe alphabet, and a
        // decoder that quietly accepts both would hide the day that changes.
        assert_eq!(decode("++//").expect("alphabet"), vec![0xfb, 0xef, 0xff]);
        assert!(decode("--__").is_err(), "url-safe base64 is not what CDP sends");
    }

    #[test]
    fn whitespace_between_groups_is_ignored() {
        // Nothing in CDP wraps its base64, but a fixture read from a file
        // arrives with a trailing newline.
        assert_eq!(decode("TWFu\n").expect("trailing newline"), b"Man");
    }

    #[test]
    fn a_truncated_group_is_an_error_rather_than_a_short_read() {
        assert!(decode("TWFuA").is_err(), "five characters is not a whole group");
    }
}
```

- [x] **Step 3: Run to verify it fails**

```bash
cargo test -p wwt-png
```

Expected: FAIL, `cannot find function decode`.

- [x] **Step 4: Implement**

Above the test module in `base64.rs`:

```rust
/// The standard alphabet, as a reverse lookup. 64 marks a character that is
/// not in it, which is how the url-safe alphabet is rejected rather than
/// silently accepted.
const INVALID: u8 = 64;

fn value(byte: u8) -> u8 {
    match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => INVALID,
    }
}

pub fn decode(text: &str) -> Result<Vec<u8>, Error> {
    // Three bytes out of every four characters in, so this is exact for
    // unpadded input and one or two long for padded, which is one
    // allocation for a payload that can be hundreds of kilobytes.
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut accumulator = 0u32;
    let mut bits = 0u32;

    for byte in text.bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let value = value(byte);
        if value == INVALID {
            return Err(Error::Base64);
        }
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }

    // Whole groups leave nothing; a group of two or three characters leaves
    // four or two bits of padding, which are zero. Anything else is a
    // truncated group and a caller who thinks they have a whole image.
    if bits >= 6 {
        return Err(Error::Base64);
    }
    Ok(out)
}
```

`crates/wwt-png/src/lib.rs`:

```rust
//! Decoding the one picture format Chromium's screencast produces.
//!
//! Its own crate for the reason `wwt-frame` is: no I/O and no
//! dependencies, so all of it is arithmetic over bytes that a test can
//! assert on with no browser, no terminal and no page. Every other crate
//! here needs one of those to be interesting.
//!
//! It decodes what Chromium sends and refuses everything else. A decoder
//! that accepts a format it will never be given is code no test covers,
//! and a wrong guess about a format is worse than an error: it puts a
//! plausible wrong picture on screen.

pub mod base64;

/// What can be wrong with a picture.
///
/// Deliberately coarse. Nothing recovers from any of these differently:
/// the frame is dropped, the previous picture stands, and the statusline
/// says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Not base64, or a truncated group.
    Base64,
    /// Not a PNG, or a PNG whose chunks do not add up.
    Png,
    /// A PNG this decoder refuses on purpose: interlaced, palettised,
    /// 16-bit, or anything else Chromium's screencast does not produce.
    Unsupported,
    /// The deflate stream is malformed.
    Deflate,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self {
            Error::Base64 => "the picture was not valid base64",
            Error::Png => "the picture was not a valid PNG",
            Error::Unsupported => "the picture is a PNG shape wwt does not decode",
            Error::Deflate => "the picture's compressed data was malformed",
        };
        f.write_str(what)
    }
}

impl std::error::Error for Error {}
```

`std` is not a dependency; `Display` and `Error` are core to being usable from `anyhow` at the boundary, and `wwt-frame` already implements traits from `std`.

- [x] **Step 5: Run to verify it passes**

```bash
cargo test -p wwt-png
```

Expected: 6 passed.

- [x] **Step 6: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add Cargo.toml crates/wwt-png
git commit -m "feat(png): a crate for the one thing wwt has to decode

Empty [dependencies], on purpose and permanently: this is the crate that
exists so that half-block costs no dependency. base64 first, because five
milestones never needed it. A payload arrives encoded and leaves encoded
in pixel mode, and half-block is the first path that looks inside one."
```

---

### Task 3: Inflate

The one piece of real algorithm in the milestone. It is `puff.c` in Rust: a bit reader, canonical Huffman decoding by code length, and the three deflate block types. Written from the format rather than from a port, and tested against streams whose answers are known.

**Files:**
- Create: `crates/wwt-png/src/inflate.rs`
- Modify: `crates/wwt-png/src/lib.rs`

**Interfaces:**
- Consumes: `wwt_png::Error` from Task 2.
- Produces: `wwt_png::inflate::zlib(&[u8]) -> Result<Vec<u8>, Error>`, which takes a zlib stream (2-byte header, deflate data, 4-byte adler) and returns the bytes. Task 4 calls it with the concatenated IDAT data.

- [x] **Step 1: Write the failing tests**

`crates/wwt-png/src/inflate.rs`, test module at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A zlib stream around a deflate payload. The 2-byte header is
    /// 0x78 0x01, which is what "deflate, 32K window, no preset
    /// dictionary" is; the adler32 trailer is not checked by this decoder
    /// and is written as zeros.
    fn zlib_stream(deflate: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        out.extend_from_slice(deflate);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out
    }

    #[test]
    fn a_stored_block_inflates_to_itself() {
        // BFINAL=1, BTYPE=00, pad to a byte, LEN=3, NLEN=!3, then "abc".
        let deflate = [0x01, 0x03, 0x00, 0xfc, 0xff, b'a', b'b', b'c'];
        assert_eq!(zlib(&zlib_stream(&deflate)).expect("stored"), b"abc");
    }

    #[test]
    fn a_stored_block_whose_length_is_not_its_complement_is_an_error() {
        let deflate = [0x01, 0x03, 0x00, 0x00, 0x00, b'a', b'b', b'c'];
        assert_eq!(zlib(&zlib_stream(&deflate)), Err(Error::Deflate));
    }

    #[test]
    fn a_fixed_huffman_block_inflates() {
        // "hello" under fixed Huffman, as zlib -9 produces it.
        let stream = [
            0x78, 0x01, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00, 0x06, 0x2c, 0x02, 0x15,
        ];
        assert_eq!(zlib(&stream).expect("fixed"), b"hello");
    }

    #[test]
    fn a_back_reference_copies_from_what_was_already_written() {
        // "aaaaaaaaaa": one literal and a length-9 distance-1 copy, which
        // is the overlapping case a byte-at-a-time copy gets right and a
        // slice copy gets wrong.
        let stream = [0x78, 0x01, 0x4b, 0x4c, 0x1c, 0x05, 0x00, 0x0c, 0xd5, 0x02, 0x31];
        assert_eq!(zlib(&stream).expect("copy"), b"aaaaaaaaaa".to_vec());
    }

    #[test]
    fn a_dynamic_huffman_block_inflates() {
        // Text long and varied enough that zlib chooses dynamic codes.
        let stream = [
            0x78, 0x01, 0x0d, 0xc6, 0xc9, 0x0d, 0x00, 0x20, 0x08, 0x04, 0xc0, 0x56, 0x8e,
            0x2d, 0xd8, 0x7f, 0x67, 0x1e, 0x79, 0x84, 0x24, 0x8b, 0x1c, 0x1a, 0x1b, 0x18,
            0x1e, 0x1d, 0x19, 0x1f, 0x38, 0x2c, 0x2a, 0x2e, 0x29, 0x2d, 0x2b, 0x2f, 0xb8,
            0x0c, 0x28, 0x4e,
        ];
        let out = zlib(&stream).expect("dynamic");
        assert!(out.len() > 32, "dynamic block decoded to {} bytes", out.len());
    }

    #[test]
    fn a_stream_that_is_not_deflate_is_refused() {
        // Compression method 7 rather than 8.
        assert_eq!(zlib(&[0x77, 0x01, 0x00]), Err(Error::Deflate));
    }

    #[test]
    fn a_preset_dictionary_is_refused_rather_than_ignored() {
        // FDICT set. Nothing sends one, and decoding as though it were not
        // set produces confident nonsense.
        assert_eq!(zlib(&[0x78, 0x20, 0x00]), Err(Error::Deflate));
    }

    #[test]
    fn a_truncated_stream_is_an_error_rather_than_a_short_answer() {
        assert_eq!(zlib(&[0x78, 0x01, 0x4b]), Err(Error::Deflate));
    }
}
```

The three multi-byte fixtures are real zlib output. If any of them does not decode to what the test says once the implementation is right, regenerate rather than adjusting the implementation to fit:

```bash
python3 -c "import zlib;print(', '.join(hex(b) for b in zlib.compress(b'hello',9)))"
python3 -c "import zlib;print(', '.join(hex(b) for b in zlib.compress(b'a'*10,9)))"
python3 -c "import zlib;print(', '.join(hex(b) for b in zlib.compress(bytes(range(32,80))*2,9)))"
```

- [x] **Step 2: Run to verify it fails**

```bash
cargo test -p wwt-png inflate
```

Expected: FAIL, `cannot find function zlib`.

- [x] **Step 3: Implement**

Above the tests in `inflate.rs`. This is the whole algorithm; write it as given rather than improvising, and read the comments, which say which parts are load-bearing.

```rust
//! Deflate, because a PNG's pixels are behind one and adding a crate to
//! read them is the one thing this repo will not do.
//!
//! The structure is zlib's own `puff.c`: decode a canonical Huffman code
//! by walking code lengths shortest first, so the tables are two small
//! arrays rather than a lookup structure. It is not the fastest way to
//! inflate. The picture is a few thousand pixels and arrives every 33ms,
//! so the fastest way is not what this needs to be.

use crate::Error;

/// Bits, least significant first, which is the order deflate uses for
/// everything except the Huffman codes themselves.
struct Bits<'a> {
    data: &'a [u8],
    /// Index of the next byte to draw from.
    byte: usize,
    /// Bits already consumed from the current accumulator.
    accumulator: u32,
    held: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, byte: 0, accumulator: 0, held: 0 }
    }

    /// `count` bits, LSB first. Deflate never asks for more than 16.
    fn take(&mut self, count: u32) -> Result<u32, Error> {
        while self.held < count {
            let next = *self.data.get(self.byte).ok_or(Error::Deflate)?;
            self.byte += 1;
            self.accumulator |= u32::from(next) << self.held;
            self.held += 8;
        }
        let value = self.accumulator & ((1u32 << count) - 1);
        self.accumulator >>= count;
        self.held -= count;
        Ok(value)
    }

    /// Drop the rest of the current byte. A stored block starts on a
    /// boundary, and it is the only thing in the format that does.
    fn align(&mut self) {
        let whole = self.held % 8;
        self.accumulator >>= whole;
        self.held -= whole;
    }

    /// The next whole byte, once aligned.
    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(8)? as u8)
    }
}

/// A canonical Huffman code, as counts per length and symbols in order.
///
/// This is the representation that makes decoding a walk rather than a
/// table build: the codes of each length are consecutive, so knowing how
/// many there are of each length is knowing all of them.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Self, Error> {
        let mut counts = [0u16; 16];
        for &length in lengths {
            if usize::from(length) >= counts.len() {
                return Err(Error::Deflate);
            }
            counts[usize::from(length)] += 1;
        }

        // An over-subscribed code claims more codes of some length than
        // exist. Left is how many codes of the current length are still
        // unclaimed, doubling as the length grows.
        let mut left = 1i32;
        for length in 1..16 {
            left <<= 1;
            left -= i32::from(counts[length]);
            if left < 0 {
                return Err(Error::Deflate);
            }
        }

        let mut offsets = [0u16; 16];
        for length in 1..15 {
            offsets[length + 1] = offsets[length] + counts[length];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (symbol, &length) in lengths.iter().enumerate() {
            if length != 0 {
                symbols[usize::from(offsets[usize::from(length)])] = symbol as u16;
                offsets[usize::from(length)] += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    /// One symbol. Codes are packed most significant bit first, which is
    /// why this shifts the accumulating code left rather than right.
    fn decode(&self, bits: &mut Bits<'_>) -> Result<u16, Error> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for length in 1..16 {
            code |= bits.take(1)? as i32;
            let count = i32::from(self.counts[length]);
            if code - count < first {
                let at = index + (code - first);
                return self
                    .symbols
                    .get(at as usize)
                    .copied()
                    .ok_or(Error::Deflate);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(Error::Deflate)
    }
}

/// Length symbol 257..=285: the base length and how many extra bits follow.
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
    131, 163, 195, 227, 258,
];
const LENGTH_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
    13, 13,
];
/// The order the code-length code's own lengths arrive in. Not sorted:
/// the order puts the lengths most likely to be nonzero first, so the
/// trailing zeros can be omitted.
const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn fixed_codes() -> Result<(Huffman, Huffman), Error> {
    let mut lengths = [0u8; 288];
    for (symbol, length) in lengths.iter_mut().enumerate() {
        *length = match symbol {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let literals = Huffman::new(&lengths)?;
    let distances = Huffman::new(&[5u8; 30])?;
    Ok((literals, distances))
}

fn dynamic_codes(bits: &mut Bits<'_>) -> Result<(Huffman, Huffman), Error> {
    let literal_count = bits.take(5)? as usize + 257;
    let distance_count = bits.take(5)? as usize + 1;
    let code_length_count = bits.take(4)? as usize + 4;
    if literal_count > 286 || distance_count > 30 {
        return Err(Error::Deflate);
    }

    let mut code_lengths = [0u8; 19];
    for &position in CODE_LENGTH_ORDER.iter().take(code_length_count) {
        code_lengths[position] = bits.take(3)? as u8;
    }
    let code_length_code = Huffman::new(&code_lengths)?;

    // The literal and distance lengths are one run, which is why a repeat
    // can carry across the boundary between them.
    let mut lengths = vec![0u8; literal_count + distance_count];
    let mut written = 0usize;
    while written < lengths.len() {
        let symbol = code_length_code.decode(bits)?;
        match symbol {
            0..=15 => {
                lengths[written] = symbol as u8;
                written += 1;
            }
            16 => {
                // Repeat the previous length, so there has to be one.
                let previous = *lengths.get(written.wrapping_sub(1)).ok_or(Error::Deflate)?;
                let repeat = 3 + bits.take(2)? as usize;
                for _ in 0..repeat {
                    *lengths.get_mut(written).ok_or(Error::Deflate)? = previous;
                    written += 1;
                }
            }
            17 => {
                let repeat = 3 + bits.take(3)? as usize;
                written = written.checked_add(repeat).ok_or(Error::Deflate)?;
                if written > lengths.len() {
                    return Err(Error::Deflate);
                }
            }
            18 => {
                let repeat = 11 + bits.take(7)? as usize;
                written = written.checked_add(repeat).ok_or(Error::Deflate)?;
                if written > lengths.len() {
                    return Err(Error::Deflate);
                }
            }
            _ => return Err(Error::Deflate),
        }
    }

    let literals = Huffman::new(&lengths[..literal_count])?;
    let distances = Huffman::new(&lengths[literal_count..])?;
    Ok((literals, distances))
}

fn block(bits: &mut Bits<'_>, out: &mut Vec<u8>, literals: &Huffman, distances: &Huffman) -> Result<(), Error> {
    loop {
        let symbol = literals.decode(bits)?;
        match symbol {
            0..=255 => out.push(symbol as u8),
            256 => return Ok(()),
            257..=285 => {
                let index = usize::from(symbol) - 257;
                let length =
                    usize::from(LENGTH_BASE[index]) + bits.take(LENGTH_EXTRA[index])? as usize;
                let symbol = usize::from(distances.decode(bits)?);
                if symbol >= DISTANCE_BASE.len() {
                    return Err(Error::Deflate);
                }
                let distance = usize::from(DISTANCE_BASE[symbol])
                    + bits.take(DISTANCE_EXTRA[symbol])? as usize;
                if distance > out.len() {
                    return Err(Error::Deflate);
                }
                // A byte at a time, because the source and the destination
                // overlap whenever the distance is less than the length,
                // which is how deflate says "repeat this run".
                let start = out.len() - distance;
                for offset in 0..length {
                    out.push(out[start + offset]);
                }
            }
            _ => return Err(Error::Deflate),
        }
    }
}

/// Inflate a zlib stream: a two-byte header, deflate data, and an adler32
/// this does not check. The checksum would catch a corrupt frame that the
/// PNG's own CRCs and the websocket's framing have both already passed,
/// which is a cost per frame for a case that cannot reach us.
pub fn zlib(data: &[u8]) -> Result<Vec<u8>, Error> {
    let header = data.get(..2).ok_or(Error::Deflate)?;
    if header[0] & 0x0f != 8 {
        return Err(Error::Deflate);
    }
    if header[1] & 0x20 != 0 {
        // A preset dictionary. Nothing sends one, and decoding as though
        // it were absent produces confident nonsense rather than an error.
        return Err(Error::Deflate);
    }
    if (u32::from(header[0]) * 256 + u32::from(header[1])) % 31 != 0 {
        return Err(Error::Deflate);
    }

    let mut bits = Bits::new(&data[2..]);
    let mut out = Vec::new();
    loop {
        let final_block = bits.take(1)? == 1;
        match bits.take(2)? {
            0 => {
                bits.align();
                let low = u16::from(bits.byte()?);
                let high = u16::from(bits.byte()?);
                let length = low | (high << 8);
                let low = u16::from(bits.byte()?);
                let high = u16::from(bits.byte()?);
                let complement = low | (high << 8);
                if length != !complement {
                    return Err(Error::Deflate);
                }
                for _ in 0..length {
                    out.push(bits.byte()?);
                }
            }
            1 => {
                let (literals, distances) = fixed_codes()?;
                block(&mut bits, &mut out, &literals, &distances)?;
            }
            2 => {
                let (literals, distances) = dynamic_codes(&mut bits)?;
                block(&mut bits, &mut out, &literals, &distances)?;
            }
            _ => return Err(Error::Deflate),
        }
        if final_block {
            return Ok(out);
        }
    }
}
```

Declare it in `lib.rs`:

```rust
pub mod base64;
pub mod inflate;
```

- [x] **Step 4: Run to verify it passes**

```bash
cargo test -p wwt-png
```

Expected: 14 passed. If the dynamic-Huffman test fails, that is the part to debug first, and `python3 -c "import zlib; ..."` is how to make a smaller case: a stream whose text is 48 distinct bytes repeated is chosen precisely because zlib will not use fixed codes for it.

- [x] **Step 5: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt-png
git commit -m "feat(png): inflate, so a picture's pixels can be reached

puff.c's shape in Rust: canonical Huffman decoded by walking code lengths
shortest first, so a code is two small arrays and not a table build. Not
the fastest way to inflate, and it does not need to be: the picture is a
few thousand pixels and arrives every 33ms.

The adler32 is read and not checked. It would catch a corrupt frame that
the PNG's CRCs and the websocket's framing have both already passed."
```

---

### Task 4: The PNG container, and unfilter

What the crate is named for. It reads IHDR, concatenates IDAT, inflates, undoes the per-row filters, and refuses everything Chromium does not send. The fixture from Task 1 is what proves it against a real picture rather than against a synthetic one.

**Files:**
- Create: `crates/wwt-png/src/png.rs`, `crates/wwt-png/tests/screencast.rs`
- Modify: `crates/wwt-png/src/lib.rs`

**Interfaces:**
- Consumes: `inflate::zlib`, `base64::decode`, `Error`.
- Produces:
  ```rust
  pub struct Png { pub width: usize, pub height: usize, pub pixels: Vec<u8> } // RGBA, 4 bytes per pixel
  pub fn decode(bytes: &[u8]) -> Result<Png, Error>;
  pub fn decode_base64(text: &str) -> Result<Png, Error>;
  ```
  `pixels` is always RGBA regardless of the source colour type, so one consumer path handles both. Task 8 calls `decode_base64`; nothing else calls anything here.

- [ ] **Step 1: Write the failing tests**

`crates/wwt-png/src/png.rs`, tests at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A whole PNG, built here so that a test can be read without opening a
    /// hex editor. `chunk` writes length, type, data and CRC.
    fn png(width: u32, height: u32, colour: u8, raw: &[u8]) -> Vec<u8> {
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, colour, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &store(raw));
        chunk(&mut out, b"IEND", &[]);
        out
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        // The CRC is not checked by the decoder, so it need not be right
        // here. That it is not checked is asserted below.
        out.extend_from_slice(&[0, 0, 0, 0]);
    }

    /// A zlib stream of stored deflate blocks, so a test fixture needs no
    /// compressor.
    fn store(raw: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        out.push(0x01);
        out.extend_from_slice(&(raw.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
        out.extend_from_slice(raw);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out
    }

    #[test]
    fn an_unfiltered_rgba_row_decodes_to_its_pixels() {
        // One row, two pixels, filter type 0.
        let raw = [0, 255, 0, 0, 255, 0, 255, 0, 255];
        let image = decode(&png(2, 1, 6, &raw)).expect("decode");
        assert_eq!(image.width, 2);
        assert_eq!(image.height, 1);
        assert_eq!(image.pixels, vec![255, 0, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn an_rgb_png_gains_an_opaque_alpha_channel() {
        // Colour type 2. The consumer only ever wants RGBA, so widening
        // here is what keeps one path downstream instead of two.
        let raw = [0, 1, 2, 3, 4, 5, 6];
        let image = decode(&png(2, 1, 2, &raw)).expect("decode");
        assert_eq!(image.pixels, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn the_sub_filter_adds_the_pixel_to_its_left() {
        // Filter 1. Second pixel is stored as a delta of 10 on each channel.
        let raw = [1, 5, 5, 5, 255, 10, 10, 10, 0];
        let image = decode(&png(2, 1, 6, &raw)).expect("decode");
        assert_eq!(image.pixels, vec![5, 5, 5, 255, 15, 15, 15, 255]);
    }

    #[test]
    fn the_up_filter_adds_the_row_above() {
        // Filter 2 on the second row.
        let raw = [0, 5, 5, 5, 255, 2, 1, 1, 1, 0];
        let image = decode(&png(1, 2, 6, &raw)).expect("decode");
        assert_eq!(image.pixels, vec![5, 5, 5, 255, 6, 6, 6, 255]);
    }

    #[test]
    fn the_average_and_paeth_filters_decode() {
        // Filter 3 then filter 4, one pixel per row, so the predictors are
        // exercised without arithmetic nobody can check by eye.
        let raw = [0, 8, 8, 8, 255, 3, 2, 2, 2, 0, 4, 1, 1, 1, 0];
        let image = decode(&png(1, 3, 6, &raw)).expect("decode");
        assert_eq!(&image.pixels[0..4], &[8, 8, 8, 255]);
        // Average of left (0, no pixel) and above (8) is 4, plus 2.
        assert_eq!(&image.pixels[4..8], &[6, 6, 6, 255]);
        // Paeth with no left picks above, 6, plus 1.
        assert_eq!(&image.pixels[8..12], &[7, 7, 7, 255]);
    }

    #[test]
    fn an_unknown_filter_type_is_an_error() {
        let raw = [9, 0, 0, 0, 0];
        assert_eq!(decode(&png(1, 1, 6, &raw)), Err(Error::Png));
    }

    #[test]
    fn something_that_is_not_a_png_is_refused() {
        assert_eq!(decode(b"not a png at all"), Err(Error::Png));
        assert_eq!(decode(&[]), Err(Error::Png));
    }

    #[test]
    fn a_palettised_or_interlaced_or_16_bit_png_is_refused_rather_than_guessed() {
        // Chromium's screencast sends none of these, and a decoder that
        // half-handles one puts a plausible wrong picture on screen.
        let raw = [0, 0];
        assert_eq!(decode(&png(1, 1, 3, &raw)), Err(Error::Unsupported));

        let mut interlaced = png(1, 1, 6, &[0, 0, 0, 0, 0]);
        interlaced[28] = 1; // the interlace byte of IHDR
        assert_eq!(decode(&interlaced), Err(Error::Unsupported));

        let mut deep = png(1, 1, 6, &[0, 0, 0, 0, 0]);
        deep[24] = 16; // the bit depth byte
        assert_eq!(decode(&deep), Err(Error::Unsupported));
    }

    #[test]
    fn a_truncated_image_is_an_error_rather_than_a_short_picture() {
        // Two rows promised, one delivered.
        let raw = [0, 1, 2, 3, 4];
        assert_eq!(decode(&png(1, 2, 6, &raw)), Err(Error::Png));
    }

    #[test]
    fn idat_split_across_chunks_is_one_stream() {
        // A real PNG of any size arrives in several IDATs, and each one is
        // a slice of a single deflate stream rather than a stream of its
        // own. Splitting the fixture's stream in two proves they are
        // concatenated before inflating and not inflated one at a time.
        let raw = [0, 7, 7, 7, 255];
        let stream = store(&raw);
        let (first, second) = stream.split_at(4);

        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", first);
        chunk(&mut out, b"IDAT", second);
        chunk(&mut out, b"IEND", &[]);

        assert_eq!(decode(&out).expect("split idat").pixels, vec![7, 7, 7, 255]);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p wwt-png png
```

Expected: FAIL, `cannot find function decode`.

- [ ] **Step 3: Implement**

`crates/wwt-png/src/png.rs`, above the tests:

```rust
//! The container around the pixels, and the per-row filters that make them
//! compress.
//!
//! It reads what Chromium's screencast produces and refuses the rest. See
//! the crate docs: a decoder that accepts a format it will never be given
//! is untested code, and guessing wrong is worse than failing.

use crate::{Error, base64, inflate};

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// A decoded picture, always RGBA whatever the file said, so that the one
/// consumer downstream has one shape to handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Png {
    pub width: usize,
    pub height: usize,
    /// Row major, four bytes per pixel.
    pub pixels: Vec<u8>,
}

pub fn decode_base64(text: &str) -> Result<Png, Error> {
    decode(&base64::decode(text)?)
}

pub fn decode(bytes: &[u8]) -> Result<Png, Error> {
    if bytes.len() < SIGNATURE.len() || bytes[..SIGNATURE.len()] != SIGNATURE {
        return Err(Error::Png);
    }

    let mut at = SIGNATURE.len();
    let mut header: Option<(usize, usize, u8)> = None;
    let mut compressed = Vec::new();

    // Chunk layout: 4 bytes of length, 4 of type, the data, 4 of CRC. The
    // CRC is not checked. A frame reaches us over a websocket that framed
    // it and a CDP message that parsed as JSON, so a corrupt one is not a
    // failure mode that survives to here, and checking costs a pass over
    // every byte of every frame.
    while at + 8 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[at..at + 4].try_into().map_err(|_| Error::Png)?) as usize;
        let kind = &bytes[at + 4..at + 8];
        let start = at + 8;
        let end = start.checked_add(length).ok_or(Error::Png)?;
        if end + 4 > bytes.len() {
            return Err(Error::Png);
        }
        let data = &bytes[start..end];

        match kind {
            b"IHDR" => {
                if data.len() < 13 {
                    return Err(Error::Png);
                }
                let width = u32::from_be_bytes(data[0..4].try_into().map_err(|_| Error::Png)?) as usize;
                let height = u32::from_be_bytes(data[4..8].try_into().map_err(|_| Error::Png)?) as usize;
                let depth = data[8];
                let colour = data[9];
                let compression = data[10];
                let filter = data[11];
                let interlace = data[12];
                if width == 0 || height == 0 {
                    return Err(Error::Png);
                }
                if compression != 0 || filter != 0 {
                    return Err(Error::Png);
                }
                // Everything this refuses, it refuses on purpose.
                if depth != 8 || interlace != 0 || !matches!(colour, 2 | 6) {
                    return Err(Error::Unsupported);
                }
                header = Some((width, height, colour));
            }
            b"IDAT" => compressed.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        at = end + 4;
    }

    let (width, height, colour) = header.ok_or(Error::Png)?;
    let channels = if colour == 6 { 4 } else { 3 };
    let raw = inflate::zlib(&compressed)?;
    unfilter(&raw, width, height, channels).map(|pixels| Png { width, height, pixels })
}

/// Undo the per-row filters and widen to RGBA.
///
/// Each row arrives with a filter byte in front of it, and the predictors
/// address the already-reconstructed bytes rather than the filtered ones,
/// which is why this walks bytes in place rather than rows in parallel.
fn unfilter(raw: &[u8], width: usize, height: usize, channels: usize) -> Result<Vec<u8>, Error> {
    let stride = width.checked_mul(channels).ok_or(Error::Png)?;
    if raw.len() != height.checked_mul(stride + 1).ok_or(Error::Png)? {
        return Err(Error::Png);
    }

    let mut current = vec![0u8; stride];
    let mut previous = vec![0u8; stride];
    let mut out = Vec::with_capacity(width * height * 4);

    for row in 0..height {
        let start = row * (stride + 1);
        let filter = raw[start];
        current.copy_from_slice(&raw[start + 1..start + 1 + stride]);

        for index in 0..stride {
            // The pixel to the left is `channels` bytes back, and does not
            // exist for the first pixel of a row, where the format says
            // zero rather than wrapping to the previous row.
            let left = if index >= channels { current[index - channels] } else { 0 };
            let above = previous[index];
            let above_left = if index >= channels { previous[index - channels] } else { 0 };
            current[index] = match filter {
                0 => current[index],
                1 => current[index].wrapping_add(left),
                2 => current[index].wrapping_add(above),
                3 => {
                    let average = ((u16::from(left) + u16::from(above)) / 2) as u8;
                    current[index].wrapping_add(average)
                }
                4 => current[index].wrapping_add(paeth(left, above, above_left)),
                _ => return Err(Error::Png),
            };
        }

        for pixel in current.chunks_exact(channels) {
            out.extend_from_slice(&pixel[..3]);
            out.push(if channels == 4 { pixel[3] } else { 255 });
        }
        std::mem::swap(&mut previous, &mut current);
    }

    Ok(out)
}

/// The PNG predictor: whichever of left, above and above-left is closest to
/// their linear combination.
fn paeth(left: u8, above: u8, above_left: u8) -> u8 {
    let estimate = i16::from(left) + i16::from(above) - i16::from(above_left);
    let d_left = (estimate - i16::from(left)).abs();
    let d_above = (estimate - i16::from(above)).abs();
    let d_above_left = (estimate - i16::from(above_left)).abs();
    if d_left <= d_above && d_left <= d_above_left {
        left
    } else if d_above <= d_above_left {
        above
    } else {
        above_left
    }
}
```

`lib.rs`:

```rust
pub mod base64;
pub mod inflate;
pub mod png;

pub use png::{Png, decode, decode_base64};
```

- [ ] **Step 4: Write the test against the real fixture**

`crates/wwt-png/tests/screencast.rs`. This is the one that proves the crate against a picture Chromium actually produced, rather than against one this crate's own test wrote.

```rust
//! The fixture is a screenshot of a page painted `#ff0000`, taken by the
//! probe in Task 1 of the M6 plan. Every synthetic test in `src` agrees
//! with this crate's idea of a PNG; only this one agrees with Chromium's.

#[test]
fn a_real_screencast_frame_decodes_to_the_colour_the_page_was() {
    let base64 = include_str!("fixtures/screencast.txt");
    let image = wwt_png::decode_base64(base64.trim()).expect("decode the fixture");

    assert!(image.width > 0 && image.height > 0, "{}x{}", image.width, image.height);
    assert_eq!(
        image.pixels.len(),
        image.width * image.height * 4,
        "four bytes a pixel, whatever the file's colour type was"
    );

    // The page was solid red. Sampling the middle rather than a corner
    // avoids whatever a scrollbar or a border might be doing at an edge.
    let middle = ((image.height / 2) * image.width + image.width / 2) * 4;
    assert_eq!(&image.pixels[middle..middle + 4], &[255, 0, 0, 255]);
}

#[test]
fn the_bytes_and_the_base64_are_the_same_picture() {
    let bytes = include_bytes!("fixtures/screencast.png");
    let base64 = include_str!("fixtures/screencast.txt");
    assert_eq!(
        wwt_png::decode(bytes).expect("bytes"),
        wwt_png::decode_base64(base64.trim()).expect("base64")
    );
}
```

- [ ] **Step 5: Run to verify everything passes**

```bash
cargo test -p wwt-png
```

Expected: all pass, including the two fixture tests. If the fixture test fails on the colour, check whether the probe captured before the page painted; retake the fixture rather than loosening the assertion.

- [ ] **Step 6: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt-png
git commit -m "feat(png): read the container, undo the filters, refuse the rest

Always RGBA out, whatever the colour type in, so the one consumer
downstream has one shape rather than two. Chunk CRCs are read and not
checked: a frame reaches us through a websocket that framed it and a CDP
message that parsed as JSON, so a corrupt one does not survive to here.

Interlaced, palettised and 16-bit are refused rather than half-handled.
Chromium sends none of them, and a wrong guess about a format puts a
plausible wrong picture on screen, which is worse than an error."
```

---

### Task 5: A cell can have a background

Half-block needs two colours per cell. `Style`'s own comment has been saying
"there is no background color here yet" since M1, and this is the yet.

**Files:**
- Modify: `crates/wwt-frame/src/cell.rs`
- Modify: `crates/wwt-term/src/render.rs`

**Interfaces:**
- Produces: `Style { fg: Rgb, bg: Option<Rgb>, bold: bool, reverse: bool }`. `Style::default()` leaves `bg` at `None`. Every existing construction site of `Style` needs the field; there are enough that the compiler is the checklist.

- [ ] **Step 1: Write the failing tests**

In `crates/wwt-frame/src/cell.rs`, add a test module if there is none:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_style_has_no_background_unless_it_is_given_one() {
        // Text mode never sets one. A run is a foreground colour on
        // whatever the terminal's own background is, and that is what
        // makes a page painted over a user's theme look like their theme.
        assert_eq!(Style::default().bg, None);
    }
}
```

In `crates/wwt-term/src/render.rs`, in its existing `mod tests`:

```rust
    #[test]
    fn render_sets_a_background_only_when_a_cell_has_one() {
        let mut frame = Frame::new(GridSize { cols: 2, rows: 1 });
        frame.paint_text(
            CellPos { col: 0, row: 0 },
            "a",
            Style { fg: Rgb { r: 1, g: 2, b: 3 }, bg: None, bold: false, reverse: false },
        );
        frame.paint_text(
            CellPos { col: 1, row: 0 },
            "b",
            Style {
                fg: Rgb { r: 1, g: 2, b: 3 },
                bg: Some(Rgb { r: 9, g: 8, b: 7 }),
                bold: false,
                reverse: false,
            },
        );

        let mut out = Vec::new();
        Renderer::new().render(&frame, &mut out).expect("render");
        let out = String::from_utf8(out).expect("utf8");

        assert!(out.contains("\x1b[48;2;9;8;7m"), "output was {out:?}");
        // Exactly once: the cell without a background must not inherit the
        // one beside it, and the reset in front of every style is what
        // stops it.
        assert_eq!(out.matches("\x1b[48;2;").count(), 1, "output was {out:?}");
    }
```

Match the surrounding tests' way of building a `Renderer` and calling `render`; if the signature differs from the sketch above, follow the file.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p wwt-frame
cargo test -p wwt-term
```

Expected: FAIL to compile, `struct Style has no field named bg`.

- [ ] **Step 3: Add the field**

`crates/wwt-frame/src/cell.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub fg: Rgb,
    /// The cell's own background, or the terminal's when there is none.
    ///
    /// Extraction never produces one: a page painted over whatever theme
    /// the terminal has is what makes text mode look like the terminal it
    /// is in rather than like a browser pretending to be one. Half-block
    /// is the one thing that sets it, because half a cell is a foreground
    /// and a background and there is no third way to say that.
    pub bg: Option<Rgb>,
    pub bold: bool,
    /// Swap foreground and background. Chrome uses this.
    pub reverse: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fg: Rgb { r: 0xd0, g: 0xd0, b: 0xd0 },
            bg: None,
            bold: false,
            reverse: false,
        }
    }
}
```

- [ ] **Step 4: Fix every construction site**

```bash
cargo build --workspace 2>&1 | grep -c "missing field"
```

Add `bg: None` to each. Do not reach for `..Default::default()`: an explicit `None` at each site is what makes the one site that will one day want a colour visible.

- [ ] **Step 5: Teach the renderer**

In `push_style`, after the foreground:

```rust
    let _ = write!(out, "\x1b[38;2;{};{};{}m", style.fg.r, style.fg.g, style.fg.b);
    // Only when there is one. The reset at the top of this function is
    // what clears a background the previous cell set, so there is no
    // "\x1b[49m" branch to forget.
    if let Some(bg) = style.bg {
        let _ = write!(out, "\x1b[48;2;{};{};{}m", bg.r, bg.g, bg.b);
    }
```

- [ ] **Step 6: Run to verify everything passes**

```bash
cargo test --workspace
```

Expected: all pass, including the ASCII snapshot in `wwt-page`, which cannot have moved: nothing sets a background yet.

- [ ] **Step 7: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt-frame crates/wwt-term
git commit -m "feat(frame): let a cell have a background colour

Style has said 'no background color here yet' since M1. Half-block is the
yet: half a cell is a foreground and a background and there is no third
way to say that.

Optional rather than a colour, because extraction still never produces
one. A page painted over whatever theme the terminal has is what makes
text mode look like the terminal it is in."
```

---

### Task 6: Samples, and painting them as half blocks

The picture, once decoded, is a grid of colours two per cell. Turning one into cells is
painting, so it lives beside `paint_run` rather than beside the decoder.

**Files:**
- Create: `crates/wwt-frame/src/samples.rs`
- Modify: `crates/wwt-frame/src/frame.rs`, `crates/wwt-frame/src/lib.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct Samples { pub cols: u16, pub rows: u16, pub pixels: Vec<Rgb> }
  impl Samples {
      pub fn resampled(src_width: usize, src_height: usize, rgba: &[u8], cols: u16, rows: u16) -> Option<Samples>;
      pub fn at(&self, col: u16, row: u16) -> Option<Rgb>;
  }
  impl Frame { pub fn paint_samples(&mut self, area: CellRect, samples: &Samples); }
  ```
  `rgba` is a plain slice, not `wwt_png::Png`: `wwt-frame` depends on nothing and that is not negotiable. Task 8 is the only caller and it passes `png.pixels`.

- [ ] **Step 1: Write the failing tests**

`crates/wwt-frame/src/samples.rs`:

```rust
//! A picture of the page as colours, two to a cell.
//!
//! The other half of pixel mode. With a graphics protocol a frame is a
//! payload the terminal draws; without one it is this, and the cells the
//! terminal already knows how to draw.

use crate::cell::Rgb;

#[cfg(test)]
mod tests {
    use super::*;

    fn red_and_blue() -> Vec<u8> {
        // 2x2 RGBA: red, blue on the first row; blue, red on the second.
        vec![
            255, 0, 0, 255, 0, 0, 255, 255, //
            0, 0, 255, 255, 255, 0, 0, 255,
        ]
    }

    #[test]
    fn a_picture_the_size_of_the_grid_is_copied_rather_than_averaged() {
        let samples = Samples::resampled(2, 2, &red_and_blue(), 2, 2).expect("same size");
        assert_eq!(samples.at(0, 0), Some(Rgb { r: 255, g: 0, b: 0 }));
        assert_eq!(samples.at(1, 0), Some(Rgb { r: 0, g: 0, b: 255 }));
        assert_eq!(samples.at(0, 1), Some(Rgb { r: 0, g: 0, b: 255 }));
        assert_eq!(samples.at(1, 1), Some(Rgb { r: 255, g: 0, b: 0 }));
    }

    #[test]
    fn shrinking_averages_every_source_pixel_that_lands_in_a_cell() {
        // The whole 2x2 into one sample: two red and two blue average to
        // half of each. A nearest-neighbour resize would answer 255,0,0
        // and throw away three quarters of the picture.
        let samples = Samples::resampled(2, 2, &red_and_blue(), 1, 1).expect("shrink");
        assert_eq!(samples.at(0, 0), Some(Rgb { r: 127, g: 0, b: 127 }));
    }

    #[test]
    fn a_picture_larger_than_the_grid_on_one_axis_still_covers_it() {
        // Chromium preserves the source aspect ratio when it scales, and
        // the sample grid's aspect is deliberately not the source's, so
        // one axis always arrives with pixels to spare. Asking for twice
        // the grid is what guarantees the other axis is not short.
        let rgba = vec![255u8; 8 * 3 * 4];
        let samples = Samples::resampled(8, 3, &rgba, 4, 2).expect("wide");
        assert_eq!(samples.cols, 4);
        assert_eq!(samples.rows, 2);
        assert_eq!(samples.at(3, 1), Some(Rgb { r: 255, g: 255, b: 255 }));
    }

    #[test]
    fn a_truncated_picture_is_refused_rather_than_padded() {
        // Three bytes short of a 2x2 RGBA image. Padding would put a black
        // stripe on a real page and look like a rendering bug.
        let mut rgba = red_and_blue();
        rgba.truncate(13);
        assert_eq!(Samples::resampled(2, 2, &rgba, 2, 2), None);
    }

    #[test]
    fn an_empty_grid_is_refused() {
        assert_eq!(Samples::resampled(2, 2, &red_and_blue(), 0, 2), None);
        assert_eq!(Samples::resampled(0, 0, &[], 2, 2), None);
    }
}
```

In `crates/wwt-frame/src/frame.rs`'s `mod tests`:

```rust
    #[test]
    fn painting_samples_gives_every_cell_two_colours() {
        let mut frame = Frame::new(GridSize { cols: 2, rows: 3 });
        // One cell row of the frame, so two sample rows.
        let samples = Samples {
            cols: 2,
            rows: 2,
            pixels: vec![
                Rgb { r: 1, g: 1, b: 1 },
                Rgb { r: 2, g: 2, b: 2 },
                Rgb { r: 3, g: 3, b: 3 },
                Rgb { r: 4, g: 4, b: 4 },
            ],
        };
        frame.paint_samples(CellRect { col: 0, row: 1, cols: 2, rows: 1 }, &samples);

        let cell = frame.cell(CellPos { col: 0, row: 1 }).expect("painted");
        assert_eq!(cell.ch, '▀');
        assert_eq!(cell.style.fg, Rgb { r: 1, g: 1, b: 1 });
        assert_eq!(cell.style.bg, Some(Rgb { r: 3, g: 3, b: 3 }));

        let cell = frame.cell(CellPos { col: 1, row: 1 }).expect("painted");
        assert_eq!(cell.style.fg, Rgb { r: 2, g: 2, b: 2 });
        assert_eq!(cell.style.bg, Some(Rgb { r: 4, g: 4, b: 4 }));
    }

    #[test]
    fn a_label_over_a_half_block_page_is_the_label() {
        // The property M5 spent a section of its spec buying with unicode
        // placeholders, and which half-block gets for nothing: a cell is a
        // glyph or it is picture, and whatever painted last decides.
        let mut frame = Frame::new(GridSize { cols: 2, rows: 2 });
        let samples = Samples {
            cols: 2,
            rows: 2,
            pixels: vec![Rgb { r: 9, g: 9, b: 9 }; 4],
        };
        frame.paint_samples(CellRect { col: 0, row: 0, cols: 2, rows: 1 }, &samples);
        frame.paint_text(CellPos { col: 0, row: 0 }, "a", Style::default());

        assert_eq!(frame.cell(CellPos { col: 0, row: 0 }).expect("label").ch, 'a');
        assert_eq!(frame.cell(CellPos { col: 1, row: 0 }).expect("picture").ch, '▀');
    }

    #[test]
    fn a_cell_with_no_bottom_sample_is_a_solid_block() {
        // An odd number of sample rows, which a resample can produce at
        // the bottom edge. Leaving the background unset would show the
        // terminal's own colour in a stripe across the last row.
        let mut frame = Frame::new(GridSize { cols: 1, rows: 1 });
        let samples = Samples { cols: 1, rows: 1, pixels: vec![Rgb { r: 5, g: 5, b: 5 }] };
        frame.paint_samples(CellRect { col: 0, row: 0, cols: 1, rows: 1 }, &samples);

        let cell = frame.cell(CellPos { col: 0, row: 0 }).expect("painted");
        assert_eq!(cell.style.fg, Rgb { r: 5, g: 5, b: 5 });
        assert_eq!(cell.style.bg, Some(Rgb { r: 5, g: 5, b: 5 }));
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p wwt-frame
```

Expected: FAIL, `cannot find type Samples`.

- [ ] **Step 3: Implement `Samples`**

In `crates/wwt-frame/src/samples.rs`, above the tests:

```rust
/// A grid of colours, one per half cell.
///
/// `rows` is therefore twice the cell rows it covers. Kept as its own type
/// rather than as a `Vec<Rgb>` and two numbers, because the indexing is
/// the only thing that can be wrong about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Samples {
    pub cols: u16,
    pub rows: u16,
    /// Row major, `cols * rows` long.
    pub pixels: Vec<Rgb>,
}

impl Samples {
    /// Box-filter an RGBA picture down to a sample grid.
    ///
    /// Averaging rather than sampling, because the source is deliberately
    /// larger than the target: Chromium preserves the source aspect ratio
    /// when it scales and the sample grid's aspect is a half cell, which
    /// is not square, so one axis always arrives with pixels to spare.
    /// Dropping them would be dropping most of the page's text.
    ///
    /// `None` when the picture is not the size it claims, or the grid is
    /// empty. Padding a short picture would put a black stripe on a real
    /// page and read as a rendering bug rather than as a bad frame.
    pub fn resampled(
        src_width: usize,
        src_height: usize,
        rgba: &[u8],
        cols: u16,
        rows: u16,
    ) -> Option<Self> {
        if cols == 0 || rows == 0 || src_width == 0 || src_height == 0 {
            return None;
        }
        if rgba.len() != src_width.checked_mul(src_height)?.checked_mul(4)? {
            return None;
        }

        let mut pixels = Vec::with_capacity(usize::from(cols) * usize::from(rows));
        for row in 0..usize::from(rows) {
            // Half-open source spans, so every source pixel belongs to
            // exactly one cell and none is counted twice.
            let top = row * src_height / usize::from(rows);
            let bottom = (((row + 1) * src_height) / usize::from(rows)).max(top + 1);
            for col in 0..usize::from(cols) {
                let left = col * src_width / usize::from(cols);
                let right = (((col + 1) * src_width) / usize::from(cols)).max(left + 1);

                let mut totals = [0u64; 3];
                let mut count = 0u64;
                for y in top..bottom.min(src_height) {
                    for x in left..right.min(src_width) {
                        let at = (y * src_width + x) * 4;
                        totals[0] += u64::from(rgba[at]);
                        totals[1] += u64::from(rgba[at + 1]);
                        totals[2] += u64::from(rgba[at + 2]);
                        count += 1;
                    }
                }
                // Alpha is ignored: a screencast frame is opaque, because
                // it is a picture of a window and not of a layer.
                let count = count.max(1);
                pixels.push(Rgb {
                    r: (totals[0] / count) as u8,
                    g: (totals[1] / count) as u8,
                    b: (totals[2] / count) as u8,
                });
            }
        }

        Some(Self { cols, rows, pixels })
    }

    pub fn at(&self, col: u16, row: u16) -> Option<Rgb> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        self.pixels
            .get(usize::from(row) * usize::from(self.cols) + usize::from(col))
            .copied()
    }
}
```

- [ ] **Step 4: Implement `paint_samples`**

In `crates/wwt-frame/src/frame.rs`:

```rust
    /// Paint a picture as half blocks: the upper half block glyph, the top
    /// sample as its foreground and the bottom sample as its background.
    ///
    /// Painted at the lowest possible depth, because it is the page and
    /// everything else is on top of it. `paint_text` takes what it touches
    /// unconditionally, so a hint label over a picture needs nothing else.
    pub fn paint_samples(&mut self, area: CellRect, samples: &Samples) {
        for row in 0..area.rows {
            for col in 0..area.cols {
                let Some(top) = samples.at(col, row * 2) else { continue };
                // A missing bottom sample means an odd sample row count at
                // the bottom edge. Repeating the top makes the cell a
                // solid block; leaving the background unset would show the
                // terminal's own colour as a stripe.
                let bottom = samples.at(col, row * 2 + 1).unwrap_or(top);
                let Some(pos) = area
                    .col
                    .checked_add(col)
                    .zip(area.row.checked_add(row))
                    .map(|(col, row)| CellPos { col, row })
                else {
                    continue;
                };
                let Some(index) = self.index(pos) else { continue };
                self.cells[index] = Cell {
                    ch: '▀',
                    style: Style { fg: top, bg: Some(bottom), bold: false, reverse: false },
                    z: i32::MIN,
                };
            }
        }
    }
```

Add the imports `frame.rs` needs (`crate::samples::Samples`), and declare and re-export in `lib.rs`:

```rust
pub mod samples;
pub use samples::Samples;
```

- [ ] **Step 5: Run to verify it passes**

```bash
cargo test -p wwt-frame
```

Expected: all pass.

- [ ] **Step 6: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt-frame
git commit -m "feat(frame): a picture as cells, two colours at a time

Averaging and not sampling, because the source is deliberately larger
than the target: Chromium preserves the source aspect when it scales and
a half cell is not square, so one axis always arrives with pixels to
spare and dropping them would drop most of a page's text.

The property M5 bought with unicode placeholders comes free here. A cell
is a glyph or it is picture and whatever painted last decides, so a hint
label over half-block needs no machinery at all."
```

---

### Task 7: Ask for a picture the size of what will show it

A full-page PNG is a few hundred kilobytes and a half-block picture needs a few
thousand pixels. Asking Chromium for the smaller one is what makes decoding it in
process reasonable, and it is one parameter on a call that already takes it.

**Files:**
- Modify: `crates/wwt-page/src/screencast.rs`
- Modify: `crates/wwt/src/effect.rs`, `crates/wwt/src/core.rs`, `crates/wwt/src/session.rs`

**Interfaces:**
- Produces:
  ```rust
  // crates/wwt/src/effect.rs
  pub struct FrameSize { pub width: u32, pub height: u32 }
  Effect::StartScreencast(TabId, FrameSize)
  // crates/wwt-page/src/screencast.rs
  impl Page { pub async fn start_screencast(&self, width: u32, height: u32) -> Result<()> }
  ```
  `start_screencast` no longer takes a `Viewport`: what to ask for is a decision, and decisions are the session's.

- [ ] **Step 1: Write the failing tests**

In `crates/wwt/src/session.rs`'s `mod tests`:

```rust
    #[test]
    fn a_terminal_with_graphics_is_asked_for_the_page_at_full_size() {
        let mut session = session();
        session.set_graphics(true);
        let effects = session.on(Event::Key(key('p')));

        let vp = page_viewport(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
        assert!(
            effects.contains(&Effect::StartScreencast(
                tab0(),
                FrameSize { width: vp.css_width(), height: vp.css_height() }
            )),
            "effects were {effects:?}"
        );
    }

    #[test]
    fn a_terminal_without_graphics_is_asked_for_twice_the_sample_grid() {
        // Half-block wants cols by 2*rows samples. Twice that, because
        // Chromium fits the frame inside both bounds while preserving the
        // source aspect ratio, and the sample grid's aspect is a half
        // cell, which is not square: asking for exactly the grid returns a
        // frame that is short on one axis, which is a letterboxed page.
        let mut session = session();
        session.set_graphics(false);
        let effects = session.on(Event::Key(key('p')));

        let grid = page_viewport(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 }).grid();
        assert!(
            effects.contains(&Effect::StartScreencast(
                tab0(),
                FrameSize {
                    width: u32::from(grid.cols) * 2,
                    height: u32::from(grid.rows) * 4
                }
            )),
            "effects were {effects:?}"
        );
    }
```

The helpers `session()`, `tab0()` and `key(..)` already exist in that test module; use them exactly as the neighbouring tests do, and use whatever grid the `session()` helper builds rather than the 80x24 above if it differs.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p wwt --lib screencast
```

Expected: FAIL to compile, `expected 1 argument, found 2` on `Effect::StartScreencast`.

- [ ] **Step 3: Implement**

`crates/wwt/src/effect.rs`:

```rust
/// How large a picture to ask a page for, in CSS pixels.
///
/// A decision rather than a measurement: with a graphics protocol it is
/// the viewport, and without one it is twice the sample grid, which is a
/// few thousand pixels rather than a megapixel. Chromium does the scaling
/// either way, so a degraded terminal never pays for a picture it cannot
/// show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}
```

and change the variant to `StartScreencast(TabId, FrameSize)`.

`crates/wwt-page/src/screencast.rs`:

```rust
    /// Start sending pictures of this page, no larger than the given size.
    ///
    /// PNG rather than JPEG: a lossy picture of text is the one thing a
    /// browser in a terminal must not produce, and both ends of this
    /// pipeline already speak PNG.
    ///
    /// The size is the caller's decision and not the viewport's, because
    /// a terminal without a graphics protocol wants a picture two orders
    /// of magnitude smaller and Chromium is better at scaling than we are.
    pub async fn start_screencast(&self, width: u32, height: u32) -> Result<()> {
        self.client()
            .call_on(
                self.session_id(),
                "Page.startScreencast",
                json!({
                    "format": "png",
                    "maxWidth": width,
                    "maxHeight": height,
                    "everyNthFrame": 1,
                }),
            )
            .await
            .context("start the screencast")?;
        Ok(())
    }
```

`crates/wwt/src/session.rs`, beside the other small helpers:

```rust
    /// How large a picture to ask for. See `FrameSize`.
    fn frame_size(&self) -> FrameSize {
        if self.graphics {
            return FrameSize { width: self.vp.css_width(), height: self.vp.css_height() };
        }
        let grid = self.vp.grid();
        FrameSize {
            width: u32::from(grid.cols) * 2,
            height: u32::from(grid.rows) * 4,
        }
    }
```

Then use it at every `Effect::StartScreencast` site. There are three: `set_pixel`, `follow_focus`, and wherever a resize restarts the screencast. The compiler will find them.

`crates/wwt/src/core.rs`, in `spawn`:

```rust
                Effect::StartScreencast(id, size) => self.spawn(id, move |page| async move {
                    page.start_screencast(size.width, size.height)
                        .await
                        .err()
                        .map(|error| Job::Noted(id, error.to_string()))
                }),
```

Keep whatever the existing arm does about failure; only the call's arguments change.

- [ ] **Step 4: Run to verify it passes**

```bash
cargo test --workspace
```

Expected: all pass.

- [ ] **Step 5: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt crates/wwt-page
git commit -m "feat(page): ask for a picture the size of what will show it

A full-page PNG is a few hundred kilobytes and half-block needs a few
thousand pixels, so the size stops being the viewport and becomes a
decision the session makes. Twice the sample grid without graphics,
because Chromium fits a frame inside both bounds while preserving the
source aspect, and a half cell is not square: asking for exactly the grid
returns a letterboxed page."
```

---

### Task 8: The session shows a picture it decoded itself

Where the halves of pixel mode meet. `p` stops refusing, `Session::picture` becomes
one of two things, and a frame is decoded once when it arrives rather than once per
compose.

**Files:**
- Modify: `crates/wwt/Cargo.toml`, `crates/wwt/src/session.rs`

**Interfaces:**
- Consumes: `wwt_png::decode_base64`, `wwt_frame::Samples`, `Frame::paint_samples`.
- Produces: nothing later tasks depend on. This is the last of the half-block half.

- [ ] **Step 1: Write the failing tests**

In `crates/wwt/src/session.rs`'s `mod tests`. The fixture is the crate's own, reached by a relative path, so the test needs no browser:

```rust
    /// A real screencast frame, as base64, from the M6 probe.
    fn fixture_frame() -> ScreencastFrame {
        ScreencastFrame {
            data: include_str!("../../wwt-png/tests/fixtures/screencast.txt").trim().to_string(),
            ack: 1,
        }
    }

    #[test]
    fn pixel_mode_without_graphics_is_offered_rather_than_refused() {
        // M5 answered this with a notice and said so was until M6.
        let mut session = session();
        session.set_graphics(false);
        session.on(Event::Key(key('p')));

        let frame = session.compose();
        assert!(
            !matches!(session.state_of_focused(), State::Notice(_)),
            "pixel mode said something instead of entering"
        );
        assert_eq!(frame.image(), None, "no graphics means no image on the frame");
    }

    #[test]
    fn a_frame_without_graphics_composes_to_half_block_cells() {
        let mut session = session();
        session.set_graphics(false);
        session.on(Event::Key(key('p')));
        session.on(Event::Frame(tab0(), Box::new(fixture_frame())));

        let frame = session.compose();
        // Row 1 is the first page row: row 0 is the tab bar.
        let cell = frame.cell(CellPos { col: 0, row: 1 }).expect("a page cell");
        assert_eq!(cell.ch, '▀');
        assert_eq!(cell.style.fg, Rgb { r: 255, g: 0, b: 0 }, "the fixture page was red");
        assert_eq!(cell.style.bg, Some(Rgb { r: 255, g: 0, b: 0 }));
        assert_eq!(frame.image(), None, "half-block is cells and never an image");
    }

    #[test]
    fn a_frame_with_graphics_still_composes_to_an_image() {
        // M5's path, unchanged, and the test that says so.
        let mut session = session();
        session.set_graphics(true);
        session.on(Event::Key(key('p')));
        session.on(Event::Frame(tab0(), Box::new(fixture_frame())));

        let frame = session.compose();
        assert!(frame.image().is_some(), "graphics means the payload goes out whole");
    }

    #[test]
    fn a_picture_that_cannot_be_decoded_leaves_the_last_one_up_and_is_still_acked() {
        let mut session = session();
        session.set_graphics(false);
        session.on(Event::Key(key('p')));
        session.on(Event::Frame(tab0(), Box::new(fixture_frame())));

        let effects = session.on(Event::Frame(
            tab0(),
            Box::new(ScreencastFrame { data: "not a picture".to_string(), ack: 7 }),
        ));

        assert!(
            effects.contains(&Effect::AckFrame(tab0(), 7)),
            "Chromium counts acks and not paints, so a dropped frame still owes one"
        );
        let cell = session.compose().cell(CellPos { col: 0, row: 1 }).expect("a page cell");
        assert_eq!(cell.ch, '▀', "the picture you were looking at must stand");
    }

    #[test]
    fn leaving_pixel_mode_takes_the_half_block_picture_with_it() {
        let mut session = session();
        session.set_graphics(false);
        session.on(Event::Key(key('p')));
        session.on(Event::Frame(tab0(), Box::new(fixture_frame())));
        session.on(Event::Key(key('p')));

        let cell = session.compose().cell(CellPos { col: 0, row: 1 }).expect("a page cell");
        assert_ne!(cell.ch, '▀', "text mode must not keep painting the picture");
    }
```

If `state_of_focused` does not exist, assert on what the statusline row contains instead, using whatever helper the neighbouring notice tests use.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p wwt --lib half_block
cargo test -p wwt --lib pixel
```

Expected: FAIL. The first assertion to go is `pixel_mode_without_graphics_is_offered_rather_than_refused`, because `set_pixel` still refuses.

- [ ] **Step 3: Implement**

`crates/wwt/Cargo.toml`, in `[dependencies]`:

```toml
wwt-png = { path = "../wwt-png" }
```

`crates/wwt/src/session.rs`. The picture becomes one of two things:

```rust
/// The picture last received for the focused tab, in whichever form the
/// terminal can show.
///
/// Two shapes rather than one because they leave by different doors: an
/// `Image` is a payload the renderer hands to a graphics protocol, and
/// `Samples` are cells the renderer already knows how to write.
enum Picture {
    Graphics(Image),
    Blocks(Samples),
}
```

`Session::picture` becomes `Option<Picture>`. Then `on_frame`, after the ack it already pushes:

```rust
        // A frame for a tab you have switched away from, or one that was in
        // flight when pixel mode was left, is answered and discarded.
        if !self.pixel || self.focused_id() != id {
            return;
        }

        if self.graphics {
            // M5's path: the bytes never leave base64.
            self.generations += 1;
            self.picture = Some(Picture::Graphics(Image {
                generation: self.generations,
                payload: std::sync::Arc::new(frame.data),
                area: CellRect::of(self.vp.grid(), self.vp.origin_row()),
            }));
            return;
        }

        // Half-block has to look inside. Decoded here rather than in
        // `compose`, which runs for every hint label, mode change and
        // statusline update, and here rather than in a spawned task,
        // because a frame arrives on the CDP arm of the loop and never as
        // a job. A few thousand pixels against a 33ms pacing interval.
        let grid = self.vp.grid();
        let decoded = wwt_png::decode_base64(&frame.data).ok().and_then(|png| {
            Samples::resampled(png.width, png.height, &png.pixels, grid.cols, grid.rows * 2)
        });
        match decoded {
            Some(samples) => self.picture = Some(Picture::Blocks(samples)),
            // The frame you are looking at stands. It has already been
            // acked above, which is what keeps the screencast running.
            None => self.notice("that picture could not be read"),
        }
```

In `compose`, where it currently sets the image:

```rust
        // The picture is the page. Painting runs underneath would show text
        // through every cell the image does not cover.
        if self.pixel {
            match &self.picture {
                Some(Picture::Graphics(image)) => frame.set_image(Some(image.clone())),
                Some(Picture::Blocks(samples)) => {
                    frame.paint_samples(CellRect::of(self.vp.grid(), self.vp.origin_row()), samples)
                }
                None => {}
            }
        } else {
            // ... whatever the existing text-mode branch does, unchanged
        }
```

And `set_pixel` loses its refusal:

```rust
    /// Enter or leave pixel mode.
    ///
    /// Never refused. Without a graphics protocol the picture is
    /// half-block rather than absent, which is what M5's notice said it
    /// was waiting for. Whether a picture is true pixels or coloured
    /// blocks is a property of the terminal and not a mode: there is one
    /// key and one tag.
    fn set_pixel(&mut self, on: bool, effects: &mut Vec<Effect>) {
        if on == self.pixel {
            return;
        }
        ...
```

- [ ] **Step 4: Run to verify it passes**

```bash
cargo test --workspace
```

Expected: all pass. If M5's own pixel tests fail, they are asserting on the refusal that just went away: read each one and decide whether it is now wrong (delete it) or was testing something else (fix the setup by calling `set_graphics(true)`).

- [ ] **Step 5: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt
git commit -m "feat(wwt): show a page on a terminal that cannot show pictures

p stops refusing. Without a graphics protocol the frame is decoded here
and painted as half blocks, which is cells, so the diffing renderer, the
overlays and the cursor are untouched and a hint label over the picture
costs nothing to arrange.

Decoded in on_frame rather than in compose, which runs for every label
and every statusline update, and rather than in a spawned task, because a
frame arrives on the CDP arm of the loop and never as a job. A dropped
frame is still acked: Chromium counts acks and not paints."
```

---

### Task 9: A page read without our script

The fallback extractor. It shares no code with `bootstrap.js`, which is the whole
requirement: a bug in one must not be able to reach the other.

**Files:**
- Create: `crates/wwt-page/src/snapshot.rs`
- Create: `crates/wwt-page/tests/snapshot.rs`
- Modify: `crates/wwt-page/src/lib.rs`

**Interfaces:**
- Consumes: `Extraction` and `parse_css_color`, both already in `wwt-page`.
- Produces: `impl Page { pub async fn snapshot(&self, vp: Viewport) -> Result<Extraction> }`. The viewport is a parameter because a `Page` is a handle and stores none; `Core` already asks `self.session.viewport()` for exactly this, in the `Scroll` arm.

- [ ] **Step 1: Write the failing test**

`crates/wwt-page/tests/snapshot.rs`. Follow `extraction.rs` exactly: a sync `#[test]`, the shared `harness()`, and `runtime().block_on`, because each test binary launches one Chromium and hands it out a test at a time.

```rust
//! The fallback path, asserted on the same way the script path is: what
//! comes back is an `Extraction`, whatever produced it.

mod common;

use common::{harness, open, runtime, viewport};

#[test]
fn a_snapshot_reads_the_text_on_screen() {
    let h = harness();
    runtime().block_on(async {
        let extraction = open(&h, "simple.html").await.snapshot(viewport()).await.expect("snapshot");

        let texts: Vec<&str> = extraction.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("Heading")), "runs were {texts:?}");
        assert!(texts.iter().any(|t| t.contains("First paragraph.")), "runs were {texts:?}");
        assert!(extraction.caret.is_none(), "a snapshot has no caret to offer");
    });
}

#[test]
fn a_snapshot_carries_the_title_url_and_scroll_geometry() {
    let h = harness();
    runtime().block_on(async {
        // 200 lines of 20px, so it is four times the viewport.
        let extraction = open(&h, "tall.html").await.snapshot(viewport()).await.expect("snapshot");

        assert_eq!(extraction.title, "Tall Fixture");
        assert!(extraction.url.ends_with("tall.html"), "url was {}", extraction.url);
        assert_eq!(extraction.scroll_y, 0.0);
        assert!(
            extraction.scroll_height > extraction.viewport_height,
            "a 4000px page must be taller than the viewport: {} vs {}",
            extraction.scroll_height,
            extraction.viewport_height
        );
    });
}

#[test]
fn a_snapshot_positions_its_runs_where_the_script_does() {
    // The fidelity test, and the one that says the baseline rule and the
    // scroll-offset subtraction are right. Both paths on the same page,
    // compared by the cell each run lands in, which is what actually
    // reaches the screen: agreeing to the pixel is not required and
    // agreeing to the cell is.
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        let vp = viewport();
        let script = page.extract().await.expect("extract");
        let snapshot = page.snapshot(vp).await.expect("snapshot");

        for run in &script.runs {
            let text = run.text.trim();
            if text.is_empty() {
                continue;
            }
            let same = snapshot
                .runs
                .iter()
                .find(|other| other.text.trim() == text)
                .unwrap_or_else(|| {
                    panic!("the snapshot did not find {text:?} in {:?}", snapshot.runs)
                });
            assert_eq!(
                vp.row_of(run.baseline),
                vp.row_of(same.baseline),
                "{text:?} landed on different rows: script {} snapshot {}",
                run.baseline,
                same.baseline
            );
            assert_eq!(
                vp.col_of(run.rect.x),
                vp.col_of(same.rect.x),
                "{text:?} landed in different columns"
            );
        }
    });
}

#[test]
fn a_snapshot_reads_a_page_whose_script_is_broken() {
    // The reason the fallback exists, arranged the way this repo arranges
    // a fixture: `eval` breaks the page's idea of our script, and the
    // assertion is on what `snapshot` returns anyway.
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        page.eval("window.__wwt.extract = () => { throw new Error('broken') }")
            .await
            .expect("break the script");

        assert!(
            page.extract().await.is_err(),
            "the script must be broken for this test to mean anything"
        );

        let extraction = page.snapshot(viewport()).await.expect("snapshot");
        let texts: Vec<&str> = extraction.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("Heading")), "runs were {texts:?}");
    });
}

#[test]
fn a_snapshot_leaves_out_what_is_below_the_viewport() {
    // The snapshot is the whole document, so culling is ours. Without it a
    // long page paints two hundred runs into a frame with room for
    // twenty-three of them.
    let h = harness();
    runtime().block_on(async {
        let extraction = open(&h, "tall.html").await.snapshot(viewport()).await.expect("snapshot");

        let texts: Vec<&str> = extraction.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("line 0")), "runs were {texts:?}");
        assert!(
            !texts.iter().any(|t| t.contains("line 199")),
            "the bottom of a 4000px page is not on screen: {texts:?}"
        );
    });
}
```

`snapshot.rs` needs `eval`, which is behind the `test-support` feature `wwt-page` already turns on for its own tests through a dev-dependency on itself. Nothing new is needed for it.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p wwt-page --test snapshot
```

Expected: FAIL to compile, `no method named snapshot`.

- [ ] **Step 3: Implement**

`crates/wwt-page/src/snapshot.rs`:

```rust
//! Reading a page without our script in it.
//!
//! `DOMSnapshot.captureSnapshot` returns the document as parallel arrays
//! indexed into a string table, which is a shape chosen for size on the
//! wire rather than for reading. All of this file is turning that into the
//! `Extraction` the script's path returns, so that nothing downstream can
//! tell which produced it.
//!
//! It shares no code with `bootstrap.js` on purpose. That is what makes it
//! a fallback rather than a second entry point to the same bug.

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::json;
use wwt_frame::{CssRect, Style, TextRun, Viewport};

use crate::color::parse_css_color;
use crate::extract::{Extraction, Page};

/// A field that is set for only a few nodes, sent as the indices that have
/// one and the values they have.
#[derive(Debug, Default, Deserialize)]
struct RareStrings {
    #[serde(default)]
    index: Vec<usize>,
    #[serde(default)]
    value: Vec<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct RareBools {
    #[serde(default)]
    index: Vec<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Nodes {
    #[serde(default)]
    node_name: Vec<i64>,
    #[serde(default)]
    attributes: Vec<Vec<i64>>,
    #[serde(default)]
    input_value: RareStrings,
    #[serde(default)]
    text_value: RareStrings,
    #[serde(default)]
    is_clickable: RareBools,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Layout {
    #[serde(default)]
    node_index: Vec<usize>,
    #[serde(default)]
    styles: Vec<Vec<i64>>,
    #[serde(default)]
    bounds: Vec<Vec<f64>>,
    #[serde(default)]
    text: Vec<i64>,
    #[serde(default)]
    paint_orders: Vec<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextBoxes {
    #[serde(default)]
    layout_index: Vec<usize>,
    #[serde(default)]
    bounds: Vec<Vec<f64>>,
    #[serde(default)]
    start: Vec<i64>,
    #[serde(default)]
    length: Vec<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    document_url: i64,
    title: i64,
    nodes: Nodes,
    layout: Layout,
    text_boxes: TextBoxes,
    #[serde(default)]
    scroll_offset_x: f64,
    #[serde(default)]
    scroll_offset_y: f64,
    #[serde(default)]
    content_height: f64,
}

#[derive(Debug, Deserialize)]
struct Snapshot {
    documents: Vec<Document>,
    strings: Vec<String>,
}

impl Snapshot {
    /// A string by index. `-1` means "no string", which is how every
    /// optional string in this protocol is spelled.
    fn string(&self, index: i64) -> &str {
        usize::try_from(index)
            .ok()
            .and_then(|index| self.strings.get(index))
            .map_or("", String::as_str)
    }
}

impl Page {
    /// Read the page without running anything of ours in it.
    ///
    /// The second source, for a tab whose injected script threw. It costs
    /// more than `extract` does and is not an alternative to it: the
    /// snapshot is the whole document, so the work is proportional to the
    /// page rather than to what is on screen. See section 11 of the M6
    /// spec, which accepts that rather than solving it.
    pub async fn snapshot(&self, vp: Viewport) -> Result<Extraction> {
        let value = self
            .client()
            .call_on(
                self.session_id(),
                "DOMSnapshot.captureSnapshot",
                json!({
                    // Two, because a run's style is a foreground colour and
                    // a bold flag and nothing else.
                    "computedStyles": ["color", "font-weight"],
                    // Fills TextRun::z, which the painter's algorithm needs
                    // to resolve a cell two runs both cover.
                    "includePaintOrder": true,
                    "includeDOMRects": false,
                }),
            )
            .await
            .context("capture a DOM snapshot")?;

        let snapshot: Snapshot =
            serde_json::from_value(value).context("the DOM snapshot had an unexpected shape")?;
        let document = snapshot
            .documents
            .first()
            .ok_or_else(|| anyhow!("the DOM snapshot contained no document"))?;

        let viewport_height = f64::from(vp.css_height());
        Ok(Extraction {
            runs: runs(&snapshot, document, viewport_height),
            // A caret needs character positions inside a control, which
            // needs the mirror, which is script machinery. Insert mode
            // still types; it types blind.
            caret: None,
            title: snapshot.string(document.title).to_string(),
            url: snapshot.string(document.document_url).to_string(),
            scroll_y: document.scroll_offset_y,
            scroll_height: document.content_height,
            viewport_height,
        })
    }
}

fn runs(snapshot: &Snapshot, document: &Document, viewport_height: f64) -> Vec<TextRun> {
    let boxes = &document.text_boxes;
    let layout = &document.layout;
    let mut runs = Vec::new();

    for (index, &layout_index) in boxes.layout_index.iter().enumerate() {
        let (Some(bounds), Some(&start), Some(&length)) = (
            boxes.bounds.get(index),
            boxes.start.get(index),
            boxes.length.get(index),
        ) else {
            continue;
        };
        let Some(&text_index) = layout.text.get(layout_index) else { continue };
        let text = slice_utf16(snapshot.string(text_index), start, length);
        if text.trim().is_empty() {
            continue;
        }

        let Some(rect) = rect_of(bounds, document) else { continue };
        // Culling is ours: the snapshot is the whole document. Half of the
        // reason the script path costs 4ms and not 18 is that it stops
        // measuring what nobody can see, and this is that rule here.
        if rect.y + rect.h <= 0.0 || rect.y >= viewport_height {
            continue;
        }

        runs.push(TextRun {
            text,
            // A line box's bottom shares a row with its baseline for any
            // ordinary line height, and a snapshot offers no baseline of
            // its own. See open question 1 of the M6 spec, closed by
            // measurement against the script's own runs.
            baseline: rect.y + rect.h,
            rect,
            style: style_of(snapshot, layout, layout_index),
            z: layout.paint_orders.get(layout_index).copied().unwrap_or(0),
        });
    }

    runs
}

/// A text box's rectangle, in viewport coordinates.
///
/// The snapshot's are document coordinates, and everything downstream
/// expects a client rect. Getting this wrong looks right at the top of a
/// page and drifts as you scroll.
fn rect_of(bounds: &[f64], document: &Document) -> Option<CssRect> {
    let (&x, &y, &w, &h) = (bounds.first()?, bounds.get(1)?, bounds.get(2)?, bounds.get(3)?);
    Some(CssRect {
        x: x - document.scroll_offset_x,
        y: y - document.scroll_offset_y,
        w,
        h,
    })
}

fn style_of(snapshot: &Snapshot, layout: &Layout, index: usize) -> Style {
    let styles = layout.styles.get(index);
    let colour = styles
        .and_then(|s| s.first())
        .map(|&i| snapshot.string(i))
        .unwrap_or_default();
    let weight = styles
        .and_then(|s| s.get(1))
        .map(|&i| snapshot.string(i))
        .unwrap_or_default();

    Style {
        fg: parse_css_color(colour),
        bg: None,
        // A computed font-weight is a number, whatever the stylesheet
        // said, so `bold` never reaches this comparison as a word.
        bold: weight.parse::<f64>().unwrap_or(400.0) >= 600.0,
        reverse: false,
    }
}

/// The DOM counts offsets in UTF-16 code units, and Rust counts bytes.
///
/// Slicing by `chars` would be right for everything on the basic plane and
/// wrong for an emoji, which is exactly the kind of bug that only appears
/// on somebody else's page.
fn slice_utf16(text: &str, start: i64, length: i64) -> String {
    let (Ok(start), Ok(length)) = (usize::try_from(start), usize::try_from(length)) else {
        return String::new();
    };
    let units: Vec<u16> = text.encode_utf16().collect();
    let end = start.saturating_add(length).min(units.len());
    if start >= end {
        return String::new();
    }
    String::from_utf16_lossy(&units[start..end])
}
```

Two things to check against the crate as it is:

```bash
grep -n "pub(crate) fn client\|pub fn session_id\|pub struct Extraction" crates/wwt-page/src/extract.rs
```

`snapshot.rs` needs `client()` and `session_id()`; if `client()` is `pub(crate)` it is already reachable from a sibling module. `Extraction`'s fields must be public for the struct literal above; they are.

Declare the module in `crates/wwt-page/src/lib.rs` beside the others, and re-export nothing new: `snapshot` is a method on `Page`.

- [ ] **Step 4: Run to verify it passes**

```bash
cargo test -p wwt-page --test snapshot
```

Expected: all five pass. The fidelity test is the one that will fail first if the baseline rule from Task 1 was recorded as "line box" and this code still uses the bottom. Fix the code to match the recorded answer, never the test to match the code.

- [ ] **Step 5: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt-page
git commit -m "feat(page): read a page without our script in it

DOMSnapshot returns the document as parallel arrays into a string table,
and all of this is turning that into the Extraction the script's path
returns, so nothing downstream can tell which produced it. It shares no
code with bootstrap.js, which is what makes it a fallback rather than a
second entrance to the same bug.

Text boxes give per-line geometry, so this path needs no line splitting
and none of the binary search the script does. What it does need is the
scroll offset subtracted, because the snapshot's coordinates are the
document's, and viewport culling of its own, because the snapshot is the
whole document."
```

---

### Task 10: Fields and hints, from the same source

A page you can read and not click is a dead end, and a form whose contents you cannot
see is worse than one you can. Both come out of the query Task 9 already makes.

**Files:**
- Modify: `crates/wwt-page/src/snapshot.rs`
- Modify: `crates/wwt-page/tests/snapshot.rs`

**Interfaces:**
- Produces: `impl Page { pub async fn snapshot_hints(&self, vp: Viewport) -> Result<Vec<HintTarget>> }`, matching `hints()`'s return type exactly, so `Effect::Hints` can name either.

- [ ] **Step 1: Write the failing tests**

Append to `crates/wwt-page/tests/snapshot.rs`. `fields.html` and `interactive.html`
are the fixtures the script path already uses for exactly these questions, so both
paths are asserted against the same pages.

```rust
#[test]
fn a_snapshot_shows_what_is_typed_in_a_field() {
    // A control's value is not in the DOM: `input.childNodes` is empty
    // however much you type, so no text box can carry it. `eval` arranges
    // the value the way a keystroke would; the assertion is on what the
    // extraction says.
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "fields.html").await;
        page.eval("document.getElementById('typed').value = 'typed in'")
            .await
            .expect("type into the field");

        let extraction = page.snapshot(viewport()).await.expect("snapshot");
        let texts: Vec<&str> = extraction.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("typed in")), "runs were {texts:?}");
    });
}

#[test]
fn a_snapshot_shows_a_placeholder_for_an_empty_field_and_bullets_for_a_password() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "fields.html").await;
        page.eval("document.getElementById('secret').value = 'hunter2'")
            .await
            .expect("set a password");

        let extraction = page.snapshot(viewport()).await.expect("snapshot");
        let texts: Vec<&str> = extraction.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(
            texts.iter().any(|t| t.contains("search the web")),
            "an empty field shows its placeholder: {texts:?}"
        );
        assert!(texts.iter().any(|t| t.contains("•••")), "runs were {texts:?}");
        assert!(
            !texts.iter().any(|t| t.contains("hunter2")),
            "a password must never be painted: {texts:?}"
        );
    });
}

#[test]
fn a_snapshot_finds_the_things_worth_hinting() {
    let h = harness();
    runtime().block_on(async {
        let targets = open(&h, "interactive.html")
            .await
            .snapshot_hints(viewport())
            .await
            .expect("hints");

        assert!(
            targets.iter().any(|t| t.kind == TargetKind::Editable),
            "the input is hintable and entering insert mode is what a hint on it does: {targets:?}"
        );
        assert!(
            targets.iter().filter(|t| t.kind == TargetKind::Clickable).count() >= 2,
            "the link and the button: {targets:?}"
        );
        assert!(targets.iter().all(|t| t.rect.w > 0.0 && t.rect.h > 0.0), "{targets:?}");
    });
}

#[test]
fn hints_from_a_snapshot_leave_out_what_has_no_box_and_what_is_off_screen() {
    // `display:none` has no layout box at all, so it never reaches us.
    // The one 3000px down is culled here, the same way runs are.
    //
    // The covered link is deliberately NOT excluded: the script hit-tests
    // a candidate and a snapshot has nothing to hit test with, so this
    // path labels it. A spurious label costs a keystroke, and the
    // alternative costs a round trip per candidate.
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "interactive.html").await;
        let snapshot = page.snapshot_hints(viewport()).await.expect("snapshot hints");
        let script = page.hints().await.expect("script hints");

        let height = f64::from(viewport().css_height());
        assert!(
            snapshot.iter().all(|t| t.rect.y < height),
            "nothing off screen, and the viewport is {height} tall: {snapshot:?}"
        );
        assert!(
            snapshot.len() >= script.len(),
            "the snapshot cannot exclude a covered link, so it finds at least as many: \
             snapshot {snapshot:?} script {script:?}"
        );
    });
}
```



Add `use wwt_frame::TargetKind;` to the test file's imports.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p wwt-page --test snapshot
```

Expected: FAIL, `no method named snapshot_hints`, and the two field tests fail on content.

- [ ] **Step 3: Implement the field pass**

In `snapshot.rs`. Call it from `snapshot()` by extending the runs:

```rust
        let mut all = runs(&snapshot, document, viewport_height);
        all.extend(field_runs(&snapshot, document, viewport_height));
```

and pass `all` as the `runs` field of the `Extraction`, rather than calling `runs(..)` there.

and add:

```rust
/// What a form control is showing, which no text box can say.
///
/// A control's value is element state and not DOM: `input.childNodes` is
/// empty however much you type. The script mirrors the control into a
/// hidden div to measure it; a snapshot cannot, so this paints the value
/// into the control's own box and lets `paint_run` elide it. That is the
/// difference between seeing what you typed and seeing where you typed.
fn field_runs(snapshot: &Snapshot, document: &Document, viewport_height: f64) -> Vec<TextRun> {
    let nodes = &document.nodes;
    let layout = &document.layout;

    // The layout tree indexes the node tree, and this pass needs the
    // opposite. Built once rather than searched per control.
    let mut layout_of = std::collections::HashMap::new();
    for (layout_index, &node_index) in layout.node_index.iter().enumerate() {
        layout_of.entry(node_index).or_insert(layout_index);
    }

    let values = rare_strings(&nodes.input_value);
    let texts = rare_strings(&nodes.text_value);
    let mut runs = Vec::new();

    for (node_index, &name) in nodes.node_name.iter().enumerate() {
        let name = snapshot.string(name);
        if !matches!(name, "INPUT" | "TEXTAREA") {
            continue;
        }
        let Some(&layout_index) = layout_of.get(&node_index) else { continue };
        let Some(bounds) = layout.bounds.get(layout_index) else { continue };
        let Some(rect) = rect_of(bounds, document) else { continue };
        if rect.y + rect.h <= 0.0 || rect.y >= viewport_height {
            continue;
        }

        let attribute = |wanted: &str| {
            nodes
                .attributes
                .get(node_index)
                .into_iter()
                .flat_map(|pairs| pairs.chunks_exact(2))
                .find(|pair| snapshot.string(pair[0]) == wanted)
                .map(|pair| snapshot.string(pair[1]).to_string())
        };

        let value = values
            .get(&node_index)
            .or_else(|| texts.get(&node_index))
            .map(|&index| snapshot.string(index).to_string())
            .unwrap_or_default();

        let text = if value.is_empty() {
            // What the browser shows, which is the placeholder.
            attribute("placeholder").unwrap_or_default()
        } else if attribute("type").as_deref() == Some("password") {
            // Never the value. The one run in this codebase that must not
            // say what it knows.
            "•".repeat(value.chars().count())
        } else {
            value
        };
        if text.is_empty() {
            continue;
        }

        runs.push(TextRun {
            text,
            baseline: rect.y + rect.h,
            rect,
            style: style_of(snapshot, layout, layout_index),
            // Above the page's own text: a control is drawn over whatever
            // is behind it, and its value is drawn over the control.
            z: layout.paint_orders.get(layout_index).copied().unwrap_or(0) + 1,
        });
    }

    runs
}

/// A rare field as a lookup from node index to its value.
fn rare_strings(rare: &RareStrings) -> std::collections::HashMap<usize, i64> {
    rare.index
        .iter()
        .copied()
        .zip(rare.value.iter().copied())
        .collect()
}
```

- [ ] **Step 4: Implement the hints**

```rust
impl Page {
    /// The interactive boxes, without running anything of ours.
    ///
    /// `isClickable` is Chromium's own answer to the question the script
    /// asks with a tag sweep, so this is the rare place where the fallback
    /// is the simpler of the two.
    ///
    /// What it cannot do is the occlusion test: the script hit-tests a
    /// candidate before labelling it, and a snapshot has nothing to hit
    /// test with, so a link behind a modal can still get a label here. A
    /// spurious label costs a keystroke; the alternative is a round trip
    /// per candidate.
    pub async fn snapshot_hints(&self, vp: Viewport) -> Result<Vec<HintTarget>> {
        let value = self
            .client()
            .call_on(
                self.session_id(),
                "DOMSnapshot.captureSnapshot",
                json!({ "computedStyles": [], "includePaintOrder": false, "includeDOMRects": false }),
            )
            .await
            .context("capture a DOM snapshot for hints")?;

        let snapshot: Snapshot =
            serde_json::from_value(value).context("the DOM snapshot had an unexpected shape")?;
        let document = snapshot
            .documents
            .first()
            .ok_or_else(|| anyhow!("the DOM snapshot contained no document"))?;

        let viewport_height = f64::from(vp.css_height());
        let layout = &document.layout;
        let mut layout_of = std::collections::HashMap::new();
        for (layout_index, &node_index) in layout.node_index.iter().enumerate() {
            layout_of.entry(node_index).or_insert(layout_index);
        }

        let mut targets = Vec::new();
        let editable = |node_index: usize| {
            matches!(
                snapshot.string(document.nodes.node_name.get(node_index).copied().unwrap_or(-1)),
                "INPUT" | "TEXTAREA" | "SELECT"
            )
        };

        // A control is worth hinting whether or not Chromium calls it
        // clickable, because hinting one is how insert mode is entered.
        let candidates = document
            .nodes
            .is_clickable
            .index
            .iter()
            .copied()
            .chain((0..document.nodes.node_name.len()).filter(|&index| editable(index)));

        let mut seen = std::collections::HashSet::new();
        for node_index in candidates {
            if !seen.insert(node_index) {
                continue;
            }
            let Some(&layout_index) = layout_of.get(&node_index) else { continue };
            let Some(bounds) = layout.bounds.get(layout_index) else { continue };
            let Some(rect) = rect_of(bounds, document) else { continue };
            if rect.w <= 0.0 || rect.h <= 0.0 {
                continue;
            }
            if rect.y + rect.h <= 0.0 || rect.y >= viewport_height {
                continue;
            }
            targets.push(HintTarget {
                rect,
                kind: if editable(node_index) { TargetKind::Editable } else { TargetKind::Clickable },
            });
        }

        Ok(targets)
    }
}
```

Add `HintTarget` and `TargetKind` to the `wwt_frame` import at the top of `snapshot.rs`.

- [ ] **Step 5: Run to verify it passes**

```bash
cargo test -p wwt-page --test snapshot
```

Expected: all nine pass. If `a_snapshot_finds_the_things_worth_hinting` reports four targets rather than three, print them: a `data:` document sometimes has a clickable body. Narrow the fixture rather than loosening the count, so the test keeps saying something.

- [ ] **Step 6: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt-page
git commit -m "feat(page): a degraded page still shows its fields and takes hints

A control's value is element state and not DOM, so no text box can carry
it; the snapshot carries it separately and this paints it into the
control's own box. A password is bullets here as everywhere else.

isClickable is Chromium's own answer to the question the script asks with
a tag sweep, so hints are the one place the fallback is the simpler path.
What it cannot do is the occlusion test, so a link behind a modal can
still get a label: a spurious label costs a keystroke and the alternative
costs a round trip per candidate."
```

---

### Task 11: The rule that reaches for the fallback

The milestone's decision, and therefore `Session`'s. No browser appears anywhere in
this task's tests, which is the point: a rule that needs a browser to exercise is a
rule nobody will test.

**Files:**
- Modify: `crates/wwt/src/effect.rs`, `crates/wwt/src/event.rs`, `crates/wwt/src/tab.rs`, `crates/wwt/src/session.rs`
- Modify: `crates/wwt-ui/src/chrome.rs`

**Interfaces:**
- Produces:
  ```rust
  // crates/wwt/src/effect.rs
  pub enum Source { Script, Snapshot }
  Effect::Extract(TabId, Source)
  Effect::Hints(TabId, Source)
  // crates/wwt/src/event.rs
  Job::Extracted(TabId, Source, Result<Box<Extraction>, String>)
  // crates/wwt/src/tab.rs
  Tab { pub degraded: bool, .. }
  // crates/wwt-ui/src/chrome.rs
  Chrome { pub degraded: bool, .. }
  ```
  Task 12 is the only consumer of `Source` outside this task.

- [ ] **Step 1: Write the failing tests**

In `crates/wwt/src/session.rs`'s `mod tests`:

```rust
    fn failed(id: TabId) -> Job {
        Job::Extracted(id, Source::Script, Err("__wwt is not defined".to_string()))
    }

    #[test]
    fn a_script_that_throws_is_read_the_other_way_instead() {
        let mut session = session();
        assert_eq!(session.begin(), vec![Effect::Extract(tab0(), Source::Script)]);

        let effects = session.on(Event::Done(failed(tab0())));
        assert_eq!(
            effects,
            vec![Effect::Extract(tab0(), Source::Snapshot)],
            "a failed script extraction asks the other source, once"
        );
    }

    #[test]
    fn a_tab_that_has_degraded_asks_the_snapshot_first_from_then_on() {
        // Otherwise a page whose script is permanently broken pays a failed
        // round trip before every good one, on every scroll frame.
        let mut session = session();
        session.begin();
        session.on(Event::Done(failed(tab0())));
        session.on(Event::Done(Job::Extracted(
            tab0(),
            Source::Snapshot,
            Ok(Box::new(extraction("https://example.com"))),
        )));

        assert_eq!(
            session.on(Event::Dirty(tab0())),
            vec![Effect::Extract(tab0(), Source::Snapshot)]
        );
    }

    #[test]
    fn a_snapshot_that_also_fails_is_the_end_of_the_line() {
        let mut session = session();
        session.begin();
        session.on(Event::Done(failed(tab0())));

        let effects = session.on(Event::Done(Job::Extracted(
            tab0(),
            Source::Snapshot,
            Err("no document".to_string()),
        )));

        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Extract(..))),
            "there is no third source: {effects:?}"
        );
        assert!(matches!(session.focused_state(), State::Error(_)), "the statusline must say so");
    }

    #[test]
    fn a_failed_extraction_leaves_the_frame_you_are_looking_at_alone() {
        let mut session = session();
        session.begin();
        session.on(Event::Done(Job::Extracted(
            tab0(),
            Source::Script,
            Ok(Box::new(extraction("https://example.com"))),
        )));
        let before = session.compose().row_text(1);

        session.on(Event::Done(failed(tab0())));
        session.on(Event::Done(Job::Extracted(
            tab0(),
            Source::Snapshot,
            Err("no document".to_string()),
        )));

        assert_eq!(session.compose().row_text(1), before, "spec section 8");
    }

    #[test]
    fn navigating_gives_a_degraded_tab_the_good_path_back() {
        // A new document reinstalls bootstrap.js, so the next page has done
        // nothing to deserve the slow path. It is also the way back:
        // reloading a tab that degraded on a transient failure clears it.
        let mut session = session();
        session.begin();
        session.on(Event::Done(failed(tab0())));
        session.on(Event::Done(Job::Extracted(
            tab0(),
            Source::Snapshot,
            Ok(Box::new(extraction("https://example.com"))),
        )));

        // Reload is Ctrl-r here, not r: `keymap.rs` is the table to check
        // rather than to guess at.
        session.on(ctrl('r'));
        session.on(Event::Done(Job::Settled(tab0())));

        assert_eq!(
            session.on(Event::Dirty(tab0())),
            vec![Effect::Extract(tab0(), Source::Script)]
        );
    }

    #[test]
    fn hints_follow_the_flag_rather_than_deciding_anything() {
        let mut session = session();
        session.begin();
        session.on(Event::Done(Job::Extracted(
            tab0(),
            Source::Script,
            Ok(Box::new(extraction("https://example.com"))),
        )));
        assert!(session.on(Event::Key(key('f'))).contains(&Effect::Hints(tab0(), Source::Script)));

        let mut session = session();
        session.begin();
        session.on(Event::Done(failed(tab0())));
        session.on(Event::Done(Job::Extracted(
            tab0(),
            Source::Snapshot,
            Ok(Box::new(extraction("https://example.com"))),
        )));
        assert!(session.on(Event::Key(key('f'))).contains(&Effect::Hints(tab0(), Source::Snapshot)));
    }

    #[test]
    fn a_degraded_tab_says_so_and_goes_on_saying_it() {
        // Not a State::Notice: a notice is cleared by the next successful
        // extraction, and on a degraded tab the next extraction succeeds
        // every time, so it would say this once and never again.
        let mut session = session();
        session.begin();
        session.on(Event::Done(failed(tab0())));
        session.on(Event::Done(Job::Extracted(
            tab0(),
            Source::Snapshot,
            Ok(Box::new(extraction("https://example.com"))),
        )));

        let rows = session.compose().grid().rows;
        let status = session.compose().row_text(rows - 1);
        assert!(status.contains("[degraded]"), "statusline was {status:?}");

        session.on(Event::Done(Job::Extracted(
            tab0(),
            Source::Snapshot,
            Ok(Box::new(extraction("https://example.com"))),
        )));
        let status = session.compose().row_text(rows - 1);
        assert!(status.contains("[degraded]"), "and still says it: {status:?}");
    }

    #[test]
    fn the_tag_belongs_to_the_tab_and_not_to_the_browser() {
        let mut session = session();
        session.begin();
        session.on(Event::Done(failed(tab0())));
        // `t` opens the command line with "tabopen " prefilled rather than
        // opening a tab, so this drives it the way the tab tests do.
        typed(&mut session, ":tabopen https://other.test");
        session.on(Event::Done(Job::Opened(TabId(1), Ok(()))));

        let rows = session.compose().grid().rows;
        let status = session.compose().row_text(rows - 1);
        assert!(!status.contains("[degraded]"), "the new tab is fine: {status:?}");
    }
```

`key`, `ctrl` and `typed` are already in that test module; `focused_state()` stands for however it already reaches the focused tab's `State`, which the neighbouring error tests do. In `crates/wwt-ui/src/chrome.rs`:

```rust
    #[test]
    fn statusline_tags_a_degraded_tab() {
        let line = statusline(
            &Mode::Normal,
            &State::Ready,
            "https://example.com",
            "Example",
            0.0,
            false,
            true,
            60,
        );
        assert!(line.contains("[degraded]"), "line was {line:?}");
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p wwt --lib
```

Expected: FAIL to compile. `Effect::Extract` takes one argument, `Source` does not exist.

- [ ] **Step 3: Implement the vocabulary**

`crates/wwt/src/effect.rs`:

```rust
/// Which way to read a page.
///
/// The effect says, rather than the page deciding, so that the rule about
/// when to reach for the second one is written where a test can exercise
/// it without a browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `window.__wwt`, installed into every document. Cheap, complete, and
    /// occasionally broken by the page it is installed in.
    Script,
    /// `DOMSnapshot.captureSnapshot`, which shares no code with it. Costs
    /// the whole document rather than what is on screen, offers no caret,
    /// and works on a page that has broken the script.
    Snapshot,
}
```

with `Extract(TabId, Source)` and `Hints(TabId, Source)`.

`crates/wwt/src/event.rs`:

```rust
    /// The page was read, or could not be. One variant rather than two,
    /// for the reason `Hints` is one: it is the only thing that clears
    /// `extracting`, so there must be exactly one place that can forget
    /// the extraction is over. It also carries which source answered,
    /// because a failed script extraction and a failed snapshot mean
    /// different things and `Job::Failed` cannot tell them apart from a
    /// failed scroll.
    Extracted(TabId, Source, Result<Box<Extraction>, String>),
```

`crates/wwt/src/tab.rs`, beside the other flags, and `false` in `Tab::new`:

```rust
    /// This tab's injected script threw, so it is read by snapshot until it
    /// navigates. Not one of the in-flight flags: it outlives the effect
    /// that set it, on purpose.
    pub degraded: bool,
```

- [ ] **Step 4: Implement the rule**

In `start_extract`, the effect names the source the tab is on:

```rust
        tab.extracting = true;
        tab.dirty = false;
        let source = if tab.degraded { Source::Snapshot } else { Source::Script };
        effects.push(Effect::Extract(id, source));
```

In `on_job`, replace the `Job::Extracted` arm's head:

```rust
            Job::Extracted(_, source, result) => {
                let extraction = match result {
                    Ok(extraction) => extraction,
                    Err(message) => {
                        let tab = self.tab_mut(id).expect("resolved above");
                        tab.extracting = false;
                        match source {
                            // The script broke. Read it the other way, once,
                            // and go on reading it that way until it
                            // navigates.
                            Source::Script => {
                                tab.degraded = true;
                                tab.dirty = true;
                                self.start_extract(id, effects);
                            }
                            // There is no third source. The frame you are
                            // looking at stands and only the statusline
                            // changes, which is section 8 of the parent.
                            Source::Snapshot => tab.state = State::Error(message),
                        }
                        return;
                    }
                };
                // ... the existing body, unchanged from here
```

Where a navigation is asked for, clear the flag. Find the one place that sets
`navigating = true` and add beside it:

```rust
        // A new document reinstalls bootstrap.js, so the next page has done
        // nothing to deserve the slow path. Cleared on asking rather than
        // on arriving, which makes a reload the way back from a tab that
        // degraded on something transient.
        tab.degraded = false;
```

Where `f` emits its effect:

```rust
        let source = if tab.degraded { Source::Snapshot } else { Source::Script };
        effects.push(Effect::Hints(id, source));
```

`crates/wwt-ui/src/chrome.rs`: add `pub degraded: bool` to `Chrome`, a `degraded: bool` parameter to `statusline` beside `pixel`, and the tag:

```rust
    // Beside [pixel] and for the same reason: a flag rather than a State,
    // because State::Notice is cleared by the next successful extraction
    // and this condition has to outlive one.
    let degraded = if degraded { "[degraded] " } else { "" };
```

and put it into both `format!` arms, after `pixel`. In `Session::compose`, fill it
from the focused tab.

- [ ] **Step 5: Run to verify it passes**

```bash
cargo test --workspace
```

Expected: all pass. Every existing `Job::Extracted(id, Box::new(..))` in the test
module becomes `Job::Extracted(id, Source::Script, Ok(Box::new(..)))`; there are
around fifteen and the compiler lists them.

- [ ] **Step 6: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt crates/wwt-ui
git commit -m "feat(wwt): read a page the other way when its script throws

One retry, then stickiness: a page that breaks the script permanently
costs one round trip per scroll rather than a failed one and then a good
one. Navigation clears it, because a new document reinstalls the script,
which also makes reload the way back from something transient.

Job::Extracted carries a Result now, for the reason Job::Hints does: it
is the only thing that clears `extracting`, so there must be one place
that cannot forget the extraction is over. Job::Failed could not carry
this, because a failed scroll and a failed extraction arrive as the same
variant and mean opposite things.

The tag is a flag and not a State::Notice, which the next successful
extraction would clear: on a degraded tab every extraction succeeds."
```

---

### Task 12: The loop answers whichever source was named

Machinery, and nothing else. `Core` gains no rule: it reads the source off the effect
and calls the matching method.

**Files:**
- Modify: `crates/wwt/src/core.rs`

**Interfaces:**
- Consumes: `Source`, `Page::snapshot`, `Page::snapshot_hints`.

- [ ] **Step 1: Implement**

```rust
                Effect::Extract(id, source) => {
                    let vp = self.session.viewport();
                    self.spawn(id, move |page| async move {
                        // The two sources answer the same question, so they
                        // report the same job and the session's rule is the
                        // only thing that tells them apart.
                        let read = match source {
                            Source::Script => page.extract().await,
                            Source::Snapshot => page.snapshot(vp).await,
                        };
                        Some(Job::Extracted(
                            id,
                            source,
                            read.map(Box::new).map_err(|error| error.to_string()),
                        ))
                    })
                }

                Effect::Hints(id, source) => {
                    let vp = self.session.viewport();
                    self.spawn(id, move |page| async move {
                        let found = match source {
                            Source::Script => page.hints().await,
                            Source::Snapshot => page.snapshot_hints(vp).await,
                        };
                        Some(Job::Hints(id, found.map_err(|error| error.to_string())))
                    })
                }
```

Keep the comment already above the `Hints` arm about why a failure is a `Job::Hints`
and not a `Job::Failed`; it is still true and still the reason.

`self.session.viewport()` before the closure, not inside it: nothing in a spawn may
borrow `self`, which is the same rule the `Scroll` arm above already follows.

- [ ] **Step 2: Run to verify the workspace passes**

```bash
cargo test --workspace
```

Expected: all pass. This task adds no test of its own on purpose: it makes no
decision, and the decisions are all tested in Task 11 with no browser. What proves
this wiring is the integration test in Task 13's step 2.

- [ ] **Step 3: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt
git commit -m "feat(wwt): spawn whichever source the effect named

The loop gains no rule. It reads the source off the effect and calls the
matching method, and both report the same job, so the session's rule is
the only thing that can tell a script extraction from a snapshot one."
```

---

### Task 13: The measurements, the notes and the manual pass

M6 makes two claims that belong in tests rather than in anybody's head: that a
degraded frame costs a fraction of a pixel frame, and that a degraded extraction costs
much more than a script one. The second is the honest one, and section 11 of the spec
owes a number for it.

**Files:**
- Modify: `crates/wwt/src/session.rs` (the half-block measurement)
- Create: `crates/wwt-page/tests/snapshot.rs` (the snapshot measurement, appended)
- Modify: `CLAUDE.md`, `CONTEXT.md`, `README.md`
- Modify: `docs/superpowers/specs/2026-08-19-wwt-design.md` (the four amendments)
- Modify: `docs/superpowers/specs/2026-08-23-wwt-m6-design.md` (open question 2)

- [ ] **Step 1: Write the half-block measurement**

In `crates/wwt/src/session.rs`'s `mod tests`, beside `measure_pixel_compose`:

```rust
    /// What a degraded frame costs, from base64 to cells. Run with:
    ///
    ///     cargo test -p wwt --lib measure_halfblock_frame -- --nocapture
    ///
    /// The claim is that half-block is the cheap path: the payload is a few
    /// kilobytes rather than a few hundred, and the decode is a few
    /// thousand pixels. It runs on the loop's thread, so the number that
    /// matters is this one against FRAME_INTERVAL, which is 33ms.
    #[test]
    fn measure_halfblock_frame() {
        let mut session = session();
        session.set_graphics(false);
        session.on(Event::Key(key('p')));
        let frame = fixture_frame();

        let mut worst = std::time::Duration::ZERO;
        for _ in 0..50 {
            let start = std::time::Instant::now();
            session.on(Event::Frame(tab0(), Box::new(frame.clone())));
            let _ = session.compose();
            worst = worst.max(start.elapsed());
        }
        eprintln!("half-block frame and compose, worst of 50: {worst:?}");
        assert!(worst < std::time::Duration::from_millis(16), "frame took {worst:?}");
    }
```

Sixteen milliseconds is half the pacing interval, which is the point at which
decoding on the loop would stop being free. It is expected to come in far under.

- [ ] **Step 2: Write the snapshot measurement and the wiring test**

Append to `crates/wwt-page/tests/snapshot.rs`:

```rust
/// What reading a page the degraded way costs. Run with:
///
///     cargo test -p wwt-page --test snapshot measure_snapshot -- --nocapture
///
/// `heavy.html` is fifteen hundred paragraphs of which a dozen are on
/// screen. The script path costs ~4ms because it stops measuring what
/// nobody can see; a snapshot is the whole document and cannot. This test
/// exists to make the gap a fact rather than a guess, and open question 2
/// of the M6 spec is decided against the number it prints.
#[test]
fn measure_snapshot_of_a_heavy_page() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "heavy.html").await;
        let vp = viewport();

        let start = std::time::Instant::now();
        let snapshot = page.snapshot(vp).await.expect("snapshot");
        let snapshot_time = start.elapsed();

        let start = std::time::Instant::now();
        let script = page.extract().await.expect("extract");
        let script_time = start.elapsed();

        eprintln!(
            "heavy.html: snapshot {} runs in {snapshot_time:?}, script {} runs in {script_time:?}",
            snapshot.runs.len(),
            script.runs.len()
        );
    });
}
```

- [ ] **Step 3: Run the measurements and record them**

```bash
cargo test -p wwt --lib measure_halfblock_frame -- --nocapture
cargo test -p wwt-page --test snapshot measure_snapshot -- --nocapture
```

Write both numbers into section 11 of `docs/superpowers/specs/2026-08-23-wwt-m6-design.md`, replacing the sentences that promise them. Then close open question 2 with the answer the number gives: if a degraded `heavy.html` is merely slow, close it as accepted; if it is unusable, say so and open the cap as work for the milestone that needs it, since `textBoxes` arrive in document order and stopping once the visible rows are filled is the shape of the fix.

- [ ] **Step 4: Write the four amendments into the parent spec**

In `docs/superpowers/specs/2026-08-19-wwt-design.md`, exactly as section 9 of the M6 spec describes:

1. **Section 8, "Injected script throws"**: replace the open question with the answer. `DOMSnapshot.captureSnapshot`, feeding the normal renderer, no reflow layer, independent of reader mode.
2. **Section 8, "No Kitty graphics"**: two tiers, not three. Delete the placeholder-block tier and say why: the renderer writes truecolor for every styled cell with no capability check, so a terminal that cannot do half-block cannot do text mode either.
3. **Section 11**: M6 is degradation, M7 stays hardening, M8 is reader mode and the reflow renderer.
4. Nothing in the parent spec claims pixel mode refuses without graphics; the sentence that did is in M5's spec, section 5. Amend it there to say half-block, with a pointer to M6.

- [ ] **Step 5: Update the glossary**

In `CONTEXT.md`, under "What the browser is doing":

```markdown
**Degraded** — a tab whose injected script threw, and which is therefore
read by `DOMSnapshot` instead. Sticky until the tab navigates, because a
new document reinstalls the script. It keeps runs, hints, scrolling and
input; it loses the caret, wrapping inside a control, and the occlusion
test that keeps a label off a covered link.

**Source** — which way a page is read: `Script` or `Snapshot`. Named by
the effect rather than chosen by the page, so the rule about when to
reach for the second one is a decision `Session` makes and a test can
exercise with no browser.

**Samples** — a picture as colours, one per half cell, so `rows` is twice
the cell rows it covers. What pixel mode composes to on a terminal with
no graphics protocol.

**Half-block** — a cell showing `▀` with the top sample as its foreground
and the bottom as its background. Two colours in one cell, which is the
whole reason `Style` has a background at all.
```

- [ ] **Step 6: Update the working notes**

In `CLAUDE.md`: change the milestone line to M6, add the two commands

```
    cargo test -p wwt --lib measure_halfblock_frame -- --nocapture     # a degraded picture
    cargo test -p wwt-page --test snapshot measure_snapshot -- --nocapture   # a degraded read
```

add `wwt-png` to the crate table with its hard rule, and add a section after "Pixel mode":

```markdown
## Degradation

**A page that breaks the script is read another way, not given up on.** A
failed `Source::Script` extraction degrades the tab and asks
`DOMSnapshot` once; a failed `Source::Snapshot` is the end of the line and
leaves the frame you are looking at alone. A degraded tab asks the
snapshot first from then on, so a permanently broken page costs one round
trip per scroll rather than two, and navigation clears the flag because a
new document reinstalls `bootstrap.js`. That also makes reload the way
back.

**The effect names the source.** Not the page, and not a field the page
sets: the rule is a decision, so it lives in `Session` where a test needs
no browser. `Job::Extracted` carries a `Result` for the reason
`Job::Hints` does, and carries the source because a failed scroll and a
failed extraction used to arrive as the same `Job::Failed`.

**A snapshot is the whole document.** The script path costs what is on
screen and this one cannot, so a degraded read of `heavy.html` pays for
all fifteen hundred paragraphs. Accepted rather than solved: it is a
fallback and not a mode anyone chooses. Culling to the viewport is on our
side, and it is the only reason it is bearable.

**What a degraded tab loses** is the caret, wrapping inside a control, and
the hint occlusion test. Everything else keeps working, because scrolling
and input go over CDP and never through our script.

**Without a graphics protocol the picture is half-block, not a notice.**
`▀` with the top sample as foreground and the bottom as background, which
is cells, so the diffing renderer and every overlay rule apply unchanged
and a label over a picture costs nothing. `p` never refuses.

**The picture is asked for at the size that will show it.** Twice the
sample grid, because Chromium preserves the source aspect while fitting
inside both bounds and a half cell is not square: asking for exactly the
grid returns a letterboxed page. A few kilobytes rather than a few
hundred, which is what makes decoding it in process reasonable.

**`wwt-png` decodes what Chromium sends and refuses the rest.** Base64,
IHDR, inflate, unfilter, always RGBA out. No interlacing, no palettes, no
16-bit: a decoder that accepts what it will never be given is untested
code, and a wrong guess puts a plausible wrong picture on screen.

**The decode happens in `on_frame`, never in `compose`.** Composing is
what a hint label and a statusline update each cost. It is on the loop's
thread because a frame arrives on the CDP arm of the `select!` and never
as a job, and the numbers make that fine: a few thousand pixels against a
33ms pacing interval.
```

- [ ] **Step 7: Update the README**

Status line to M6. Change the `p` line so it no longer promises a refusal, and add a
sentence about a degraded page:

```markdown
`p` swaps the page between text and true pixels without moving it: the
same viewport, the same scroll offset, the same tab. On a terminal that
speaks the Kitty graphics protocol it is a picture; on one that does not
it is half-block colour, which is the same page at half the vertical
resolution rather than a refusal.

A page that breaks wwt's injected script is not lost: that tab says
`[degraded]` and is read through Chromium's own DOM snapshot instead. You
keep reading, scrolling, hinting and typing; you lose the insertion point
and the wrapping inside a text field until the tab navigates.
```

- [ ] **Step 8: The manual pass**

Not automatable, and the milestone is not done without it. On a real terminal:

1. `wwt example.com`, then `p` in Kitty: a picture, unchanged from M5.
2. The same in a terminal with no graphics (`TERM=xterm` under something plain, or `kitty +kitten` disabled): `p` gives half-block colour rather than a notice, in the same place at the same scroll offset.
3. `j` and `k` in half-block: the picture scrolls and keeps up.
4. `f` in half-block: labels on top of the picture, readable, and one of them clicks through.
5. `p` again from half-block: back to text, same place, nothing left on screen.
6. Resize the terminal in half-block: the picture is laid out for the terminal you have now, with no stripe at an edge.
7. A page playing video in half-block for a minute: it keeps moving and does not freeze, which is the ack path with a decode in front of it.
8. An idle page in half-block: `top` shows wwt and Chromium at rest.
9. Break a page's script by hand and confirm degradation end to end. In a scratch tab, open a `data:` URL and use the console-less route: navigate to a page, then in wwt open `:open data:text/html,<script>Object.defineProperty(window,'__wwt',{get(){throw new Error('no')}})</script><p>hello</p>`. The page must still show `hello`, the statusline must say `[degraded]`, and `f` must still find links.
10. Scroll a degraded page: the runs must move with it rather than drifting, which is the scroll-offset subtraction.
11. Navigate that degraded tab somewhere ordinary: `[degraded]` goes away.
12. Open a second tab while one is degraded: the tag belongs to the degraded tab only, and switching moves it.
13. A degraded page with a form: the field's contents are painted, a password shows bullets, `i` types into it and the text updates even though no caret shows.

Fix what it finds, and put what it taught in the commit body: M5's lesson was that the things a spec is most confident about are the ones a real terminal disproves.

- [ ] **Step 9: Run everything one last time**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p wwt-page --test extraction measure_extraction -- --nocapture
cargo test -p wwt-page --test interaction measure_scroll_latency -- --nocapture
cargo test -p wwt --lib measure_switch -- --nocapture
cargo test -p wwt-term --lib measure_pixel_frame -- --nocapture
cargo test -p wwt --lib measure_halfblock_frame -- --nocapture
cargo test -p wwt-page --test snapshot measure_snapshot -- --nocapture
```

The first four are M2's, M3's, M4's and M5's numbers, and M6 must not have moved any
of them: nothing on the good path changed. If one has moved, find out why before
calling the milestone done.

- [ ] **Step 10: Commit**

```bash
git add CLAUDE.md CONTEXT.md README.md crates docs
git commit -m "docs: write down what degrading costs and what it is called

Two measurements rather than two claims. A degraded frame is the cheap
half of pixel mode, and a degraded read is the expensive half of
extraction: the snapshot is the whole document where the script path is
what is on screen, and measure_snapshot is what makes that a fact.

Writes the four amendments M6 owes the parent spec: the fallback question
closed with DOMSnapshot, the third degradation tier deleted rather than
deferred, and section 11 renumbered so reader mode follows hardening.

M2's, M3's, M4's and M5's numbers are unmoved. Nothing on the good path
changed, which is the property the whole milestone was shaped around."
```

---

## Done when

- `cargo test --workspace` passes and `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- A page that throws from its injected script is read, scrolled, hinted and typed into, and says `[degraded]` while it is.
- `p` on a terminal with no graphics protocol shows the page in half-block colour, and the thirteen manual checks in Task 13 pass on a real terminal.
- `measure_halfblock_frame` and `measure_snapshot` print numbers, and M2's, M3's, M4's and M5's are unmoved.
- The parent spec's section 8 open question is closed, its third degradation tier is gone, and its section 11 reads M6 degradation, M7 hardening, M8 reader mode.
- **No new entry in `Cargo.toml`'s workspace dependencies.** `wwt-png` is a member, not a dependency. If a crate from outside seemed necessary, the milestone took a wrong turn: the whole design exists to avoid pulling in a decoder.
