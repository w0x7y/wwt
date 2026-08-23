# wwt M5 — Pixel Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put the page on screen as true pixels on a keypress, without moving it, and put it back.

**Architecture:** `Page.screencastFrame` carries base64 PNG and the Kitty graphics protocol wants base64 PNG, so a frame is forwarded as the string it arrived as: no decode, no re-encode, no new dependency. `Frame` gains an optional image; `Cell` does not change, and the renderer owns every byte of the protocol. Unicode placeholders make placement cell content, so a glyph painted over the page area wins over the image and hint labels need no new machinery. The seam is untouched: `Session` decides when to screencast and what a frame composes to, `Core` spawns and routes, and neither learns anything about escapes.

**Tech Stack:** Rust 2024, tokio, tokio-tungstenite, crossterm (with `event-stream`), futures-util, serde/serde_json, anyhow. Chromium as an external process. The Kitty graphics protocol as the only terminal target.

**Spec:** `docs/superpowers/specs/2026-08-22-wwt-m5-design.md` — read it in full before starting. Its parent, `docs/superpowers/specs/2026-08-19-wwt-design.md`, governs where the two disagree; sections 3, 7 and 8 of the parent are the relevant ones, and section 9 of this plan's spec lists the four places M5 amends them. Those amendments are already committed, so no task in this plan writes them.

## Global Constraints

- Rust edition **2024**, toolchain **1.97+**.
- **Do not add dependencies.** The set is fixed in `Cargo.toml` workspace deps, exact and unchanged from M4: `tokio = "1.53"`, `tokio-tungstenite = "0.30"`, `futures-util = "0.3"`, `serde = "1.0"` (feature `derive`), `serde_json = "1.0"`, `crossterm = "0.29"` (feature `event-stream`), `rustix = "1.1"` (feature `termios`), `anyhow = "1.0"`, `thiserror = "2.0"`, `tempfile = "3"` (dev-dependency). The whole design exists so that no image or compression crate is needed; if a task seems to need one, stop and ask.
- `cargo clippy --workspace --all-targets -- -D warnings` must be clean **per task**, not per plan.
- Unit tests in `src/` must run without Chromium and without a tty. Anything needing a browser goes in `tests/`.
- Test names are sentences describing the property (`a_payload_shorter_than_one_chunk_is_still_terminated`).
- Comments explain *why*, in prose, where the reason is not obvious. Do not restate code.
- Commits are conventional with a crate scope: `feat(term):`, `perf(page):`, `refactor(wwt):`. No em-dashes in commit messages or in prose.
- `wwt-frame` keeps its hard rule: no I/O, no dependencies. Nothing about a terminal protocol may enter it.

## Baseline

M4 is complete: tabs, sessions, the two chrome rows, the origin row in `Viewport`, and the persistent profile. `cargo test --workspace` is 330 tests and passes; `measure_switch` is ~120µs, `measure_extraction` ~4.8ms, `measure_scroll_latency` ~5ms. Nothing in this plan may move the last two.

## File structure

| File | Responsibility |
|---|---|
| `crates/wwt-frame/src/image.rs` | **New.** `Image`: a generation, a base64 payload, and the cell rect it covers. Data only. |
| `crates/wwt-frame/src/frame.rs` | `Frame` gains `image: Option<Image>`, its getter and its setter. |
| `crates/wwt-term/src/graphics/mod.rs` | **New.** The Kitty graphics protocol: what a transmission, a placement and a delete look like as bytes. |
| `crates/wwt-term/src/graphics/diacritics.rs` | **New.** The row/column diacritic table, vendored, and the lookup. |
| `crates/wwt-term/src/graphics/detect.rs` | **New.** The one-shot capability query and its timeout. |
| `crates/wwt-term/src/render.rs` | `Renderer` learns to place an image and to leave it alone when it has not changed. |
| `crates/wwt-page/src/screencast.rs` | **New.** `start_screencast`, `stop_screencast`, `ack_frame`, and recognising a frame event. |
| `crates/wwt/src/session.rs` | `pixel: bool`, what `p` does, what compose does in pixel mode. |
| `crates/wwt/src/effect.rs` | `StartScreencast`, `StopScreencast`, `AckFrame`. |
| `crates/wwt/src/event.rs` | `Event::Frame`, `Job::Screencast`. |
| `crates/wwt/src/core.rs` | Routing frames, dropping late ones, and the resize and switch paths. |
| `crates/wwt-ui/src/command.rs` | `:set pixel on\|off`. |
| `crates/wwt-ui/src/chrome.rs` | `pixel` in the statusline. |
| `crates/wwt/src/keymap.rs` | `p` in normal mode. |

The graphics module is split three ways because the three parts fail differently and are tested differently: the escape sequences are string building, the diacritic table is vendored data that must be checked against its source, and detection is the only part that touches a real terminal.

---

### Task 1: Settle the re-transmission question

Open question 1 of the spec. Section 4 claims a new frame rewrites no cells, which is true only if transmitting new data to an image id that already has a virtual placement updates that placement in place. If it does not, a frame costs a delete and a re-place as well, and the placeholders have to be rewritten. The shape of the design does not change either way, but the cost does, and every later task is written against the answer.

This is a throwaway probe, not code that is kept. It needs a real Kitty (or Ghostty, or another terminal implementing the protocol) and a human looking at it.

**Files:**
- Create: `/tmp/kitty-probe.sh` (throwaway, not committed)

- [x] **Step 1: Write the probe**

Two PNGs and one id. If the second image appears without the placeholder cells being rewritten, re-transmission updates in place.

```bash
cat > /tmp/kitty-probe.sh <<'PROBE'
#!/usr/bin/env bash
# Throwaway. Answers one question: does transmitting new data to a live image
# id update its virtual placement, or must the placement be recreated?
set -euo pipefail

# Two solid PNGs, 1x1, red then blue, base64. Kitty scales them to the
# placement, so 1x1 is enough to see which one is on screen.
RED=$(printf 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==')
BLUE=$(printf 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==')

transmit() { printf '\033_Gq=2,a=t,f=100,t=d,i=99,m=0;%s\033\\' "$1"; }
place()    { printf '\033_Gq=2,a=p,U=1,i=99,c=8,r=4\033\\'; }

clear
transmit "$RED"
place
# Placeholder cells: U+10EEEE with the row and column diacritics for a 4x8
# block, foreground carrying image id 99 in its blue byte.
printf '\033[38;2;0;0;99m'
for row in 0 1 2 3; do
  printf '\033[%d;1H' $((row + 2))
  # First cell of the row spends both diacritics; the rest continue from it.
  python3 -c "
import sys
d = ['̅','̍','̎','̐']
sys.stdout.write('\U0010EEEE' + d[$row] + d[0])
sys.stdout.write('\U0010EEEE' * 7)
"
done
printf '\033[0m\n\n'
read -rp "Red block on screen? Press enter to re-transmit as blue without touching a cell. "

transmit "$BLUE"
read -rp "Did it turn blue WITHOUT the cells being rewritten? (y/n) " answer
echo "answer: $answer"
PROBE
chmod +x /tmp/kitty-probe.sh
```

- [x] **Step 2: Run it in a real Kitty and record the answer**

Run: `/tmp/kitty-probe.sh`

If it turns blue, section 4 of the spec stands as written and Task 5 keeps its "a new frame rewrites no cells" rule. If it does not, amend the spec's section 4 in the same commit as Task 5, per `CLAUDE.md`, to say that a frame is a delete, a transmit, a place and a placeholder rewrite, and change Task 5's `Renderer` accordingly. Either way, write the answer into the spec's open question 1 and mark it closed.

- [x] **Step 3: Delete the probe**

```bash
rm /tmp/kitty-probe.sh
```

Nothing to commit. The spec amendment, if the answer forces one, rides with Task 5.

---

### Task 2: An image on a frame

The smallest possible change to the crate with the strictest rule. `Image` is data: a generation so the renderer can tell one frame from the next without comparing payloads, a base64 payload, and the cell rect it covers. No protocol, no I/O, no dependencies.

**Files:**
- Create: `crates/wwt-frame/src/image.rs`
- Modify: `crates/wwt-frame/src/lib.rs`
- Modify: `crates/wwt-frame/src/frame.rs`

**Interfaces:**
- Produces: `wwt_frame::Image { generation: u64, payload: String, area: CellRect }`, `wwt_frame::CellRect { col: u16, row: u16, cols: u16, rows: u16 }`, `Frame::image(&self) -> Option<&Image>`, `Frame::set_image(&mut self, image: Option<Image>)`.

- [x] **Step 1: Write the failing test**

In a new `crates/wwt-frame/src/image.rs`:

```rust
//! An image on its way to the terminal, and the cells it covers.
//!
//! Data and nothing else. What the payload means and how it reaches a
//! terminal is `wwt-term`'s, because this crate knows about no terminal.

use crate::geom::GridSize;

/// A rectangle of cells, in frame coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRect {
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
}

impl CellRect {
    /// The rectangle a viewport's grid occupies at its origin row.
    pub fn of(grid: GridSize, origin_row: u16) -> Self {
        Self {
            col: 0,
            row: origin_row,
            cols: grid.cols,
            rows: grid.rows,
        }
    }
}

/// A picture of the page, as the terminal will be given it.
///
/// The payload is base64 and stays base64: it arrives from CDP encoded and
/// the graphics protocol wants it encoded, so nothing here ever decodes it.
///
/// `generation` is what a renderer diffs on. Comparing payloads would mean
/// comparing a megabyte of base64 per frame to answer a question a counter
/// answers, and two different frames can encode identically anyway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub generation: u64,
    pub payload: String,
    pub area: CellRect,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_images_area_is_the_page_grid_at_its_origin() {
        let grid = GridSize { cols: 80, rows: 22 };
        assert_eq!(
            CellRect::of(grid, 1),
            CellRect { col: 0, row: 1, cols: 80, rows: 22 }
        );
    }

    #[test]
    fn two_frames_of_the_same_picture_differ_by_generation() {
        // The renderer's whole diffing rule rests on this: identical bytes
        // with a new generation is a new frame and must be sent again.
        let area = CellRect::of(GridSize { cols: 4, rows: 2 }, 1);
        let first = Image { generation: 1, payload: "AAAA".into(), area };
        let second = Image { generation: 2, payload: "AAAA".into(), area };
        assert_ne!(first, second);
    }
}
```

- [x] **Step 2: Run it to verify it fails**

Run: `cargo test -p wwt-frame image::`
Expected: FAIL to compile, `file not found for module` or `unresolved import`, because `lib.rs` does not declare the module yet.

- [x] **Step 3: Declare the module and re-export**

In `crates/wwt-frame/src/lib.rs`, add the module beside the others and the types beside the other re-exports:

```rust
pub mod caret;
pub mod cell;
pub mod frame;
pub mod geom;
pub mod image;
pub mod run;
pub mod target;

pub use caret::Caret;
pub use cell::{Cell, Rgb, Style};
pub use frame::Frame;
pub use geom::{CellPos, CellSize, CssPoint, CssRect, GridSize, Viewport};
pub use image::{CellRect, Image};
pub use run::TextRun;
pub use target::{HintTarget, TargetKind};
```

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p wwt-frame image::`
Expected: PASS, 2 tests.

- [x] **Step 5: Write the failing test for the frame carrying one**

In `crates/wwt-frame/src/frame.rs`, in `mod tests`:

```rust
    #[test]
    fn a_frame_carries_no_image_until_it_is_given_one() {
        let frame = Frame::new(GridSize { cols: 10, rows: 4 });
        assert_eq!(frame.image(), None);
    }

    #[test]
    fn an_image_survives_being_put_on_a_frame() {
        let mut frame = Frame::new(GridSize { cols: 10, rows: 4 });
        let image = Image {
            generation: 7,
            payload: "iVBOR".into(),
            area: CellRect { col: 0, row: 1, cols: 10, rows: 2 },
        };
        frame.set_image(Some(image.clone()));
        assert_eq!(frame.image(), Some(&image));
    }

    #[test]
    fn a_frame_with_an_image_still_paints_cells() {
        // Pixel mode leaves the page rows blank but the chrome rows are
        // cells like any other, so an image must not disturb painting.
        let mut frame = Frame::new(GridSize { cols: 10, rows: 4 });
        frame.set_image(Some(Image {
            generation: 1,
            payload: "AAAA".into(),
            area: CellRect { col: 0, row: 1, cols: 10, rows: 2 },
        }));
        let vp = Viewport::with_origin(GridSize { cols: 10, rows: 2 }, CellSize { w: 9, h: 20 }, 1);
        frame.paint_run(&vp, &TextRun::for_test("hi", 0.0, 20.0));
        assert_eq!(frame.cell(CellPos { col: 0, row: 1 }).map(|c| c.ch), Some('h'));
        assert!(frame.image().is_some());
    }
```

If `TextRun::for_test` does not exist, build the `TextRun` the way the neighbouring tests in this file already do and keep the rest of the assertion identical.

- [x] **Step 6: Run it to verify it fails**

Run: `cargo test -p wwt-frame frame::`
Expected: FAIL, `no method named 'image'`.

- [x] **Step 7: Add the field, the getter and the setter**

In `crates/wwt-frame/src/frame.rs`, extend the struct and its constructor, and add the two methods beside `cursor`/`set_cursor`:

```rust
use crate::image::Image;

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    grid: GridSize,
    cells: Vec<Cell>,
    cursor: Option<CellPos>,
    /// The page as a picture, in pixel mode. `None` is text mode, which is
    /// every frame this codebase built before M5.
    image: Option<Image>,
}
```

In `Frame::new`, initialise it to `None` alongside the other fields. Then:

```rust
    /// The picture this frame wants shown behind its cells, if any.
    ///
    /// A frame carries it rather than painting it for the same reason it
    /// carries the cursor: only the terminal can put an image on screen,
    /// and this crate is not allowed to know how.
    pub fn image(&self) -> Option<&Image> {
        self.image.as_ref()
    }

    pub fn set_image(&mut self, image: Option<Image>) {
        self.image = image;
    }
```

- [x] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p wwt-frame`
Expected: PASS, all of them. The existing property tests must be untouched: `to_cell(to_css(c)) == c` and the origin-row roundtrip do not involve an image.

- [x] **Step 9: Clippy**

Run: `cargo clippy -p wwt-frame --all-targets -- -D warnings`
Expected: clean.

- [x] **Step 10: Commit**

```bash
git add crates/wwt-frame/src/image.rs crates/wwt-frame/src/lib.rs crates/wwt-frame/src/frame.rs
git commit -m "feat(frame): let a frame carry a picture as well as cells

Data only, and deliberately so: the payload is base64 on its way through
and this crate never learns what it encodes or how a terminal is told
about it. A generation rather than a payload comparison is what a
renderer diffs on, because two frames can encode identically and
comparing a megabyte of base64 per frame answers a question a counter
answers."
```

---

### Task 3: The diacritic table

Unicode placeholders encode a cell's row and column as combining diacritics, drawn from a fixed list of 297 codepoints the protocol defines. The list is data, it is not derivable, and getting one entry wrong puts one row of the image in the wrong place, which is exactly the kind of bug that is invisible until it is not.

**It must be vendored from the source, not typed from memory.** The authoritative list is `rowcolumn-diacritics.txt` in kitty's repository, also reproduced in the graphics protocol documentation under "Unicode placeholders". Fetch it, convert it, and let the test hold the shape.

**Files:**
- Create: `crates/wwt-term/src/graphics/diacritics.rs`
- Create: `crates/wwt-term/src/graphics/mod.rs`
- Modify: `crates/wwt-term/src/lib.rs`

**Interfaces:**
- Produces: `graphics::diacritics::CODES: [char; 297]`, `graphics::diacritics::for_index(i: u16) -> Option<char>`.

- [x] **Step 1: Fetch the table**

```bash
curl -sSfL https://raw.githubusercontent.com/kovidgoyal/kitty/master/gen/rowcolumn-diacritics.txt \
  -o /tmp/rowcolumn-diacritics.txt
head -5 /tmp/rowcolumn-diacritics.txt
wc -l /tmp/rowcolumn-diacritics.txt
```

The file is one entry per line as `<hex codepoint>; <description>`, with comment lines starting `#`. If the URL has moved, find the current one in the kitty graphics protocol documentation rather than reconstructing the list by hand. **Do not invent entries.** If the table cannot be fetched, stop and say so: every later task in this milestone depends on it and a guessed table is worse than no task.

- [x] **Step 2: Generate the Rust source**

```bash
python3 - <<'GEN' > crates/wwt-term/src/graphics/diacritics.rs
import re

codes = []
for line in open('/tmp/rowcolumn-diacritics.txt'):
    line = line.strip()
    if not line or line.startswith('#'):
        continue
    codes.append(int(line.split(';')[0], 16))

print('//! The row and column diacritics unicode placeholders are addressed with.')
print('//!')
print('//! Vendored from kitty\'s `gen/rowcolumn-diacritics.txt`, which is the')
print('//! protocol\'s own list. It is data rather than arithmetic: the codepoints')
print('//! are chosen for being combining marks that terminals will not reflow, and')
print('//! nothing about the sequence can be computed from an index.')
print()
print(f'pub const CODES: [char; {len(codes)}] = [')
for i in range(0, len(codes), 8):
    row = ', '.join(f"'\\u{{{c:04X}}}'" for c in codes[i:i + 8])
    print(f'    {row},')
print('];')
print('''
/// The diacritic addressing row or column `index`, or `None` past the end of
/// the table.
///
/// Past the end is not an error to shout about: it means the terminal is
/// taller or wider than the protocol can address, and the caller's answer is
/// to stop emitting placeholders rather than to fail a render.
pub fn for_index(index: u16) -> Option<char> {
    CODES.get(usize::from(index)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_the_length_the_protocol_defines() {
        assert_eq!(CODES.len(), 297);
    }

    #[test]
    fn the_table_starts_where_the_protocol_says_it_does() {
        // The first entry is U+0305 COMBINING OVERLINE. If this fails, the
        // vendored file is not the one the protocol means.
        assert_eq!(CODES[0], '\\u{0305}');
    }

    #[test]
    fn every_entry_is_a_distinct_codepoint() {
        // A duplicate would silently address two rows the same way.
        let mut seen: Vec<char> = CODES.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), CODES.len());
    }

    #[test]
    fn an_index_past_the_table_addresses_nothing() {
        assert_eq!(for_index(297), None);
    }

    #[test]
    fn an_index_within_the_table_is_its_entry() {
        assert_eq!(for_index(0), Some(CODES[0]));
        assert_eq!(for_index(296), Some(CODES[296]));
    }
}''')
GEN
```

- [x] **Step 3: Create the module and declare it**

`crates/wwt-term/src/graphics/mod.rs`:

```rust
//! The Kitty graphics protocol: how an image reaches a terminal.
//!
//! Everything here is bytes-from-data. What to send is `Renderer`'s
//! decision and what an image is is `wwt-frame`'s; this module knows only
//! what the protocol looks like, which is why it can be tested with no
//! terminal anywhere.

pub mod diacritics;
```

In `crates/wwt-term/src/lib.rs`, add `pub mod graphics;` beside the existing modules.

- [x] **Step 4: Run the tests**

Run: `cargo test -p wwt-term graphics::diacritics`
Expected: PASS, 5 tests. If `the_table_is_the_length_the_protocol_defines` fails, the fetched file is wrong or the parser dropped lines; fix the fetch, never the assertion.

- [x] **Step 5: Clippy and commit**

```bash
cargo clippy -p wwt-term --all-targets -- -D warnings
git add crates/wwt-term/src/graphics/ crates/wwt-term/src/lib.rs
git commit -m "feat(term): vendor the table a placeholder addresses cells with

Data, not arithmetic: the 297 codepoints are combining marks chosen for
surviving a terminal's own layout, and nothing about the sequence follows
from an index. Vendored from the protocol's own list rather than typed,
and the length and first entry are asserted, because one wrong entry puts
one row of the image somewhere else and looks like a rendering bug
forever."
```

---

### Task 4: The escape sequences

Three sequences and a chunker. All of it is a function from data to bytes, so all of it is tested with data and none of it needs a terminal.

**Files:**
- Create: `crates/wwt-term/src/graphics/protocol.rs`
- Modify: `crates/wwt-term/src/graphics/mod.rs`

**Interfaces:**
- Consumes: `wwt_frame::{CellRect, Image, Rgb}`, `graphics::diacritics::for_index`.
- Produces: `graphics::protocol::IMAGE_ID: u32`, `transmit(payload: &str, out: &mut impl Write) -> io::Result<()>`, `place(area: CellRect, out: &mut impl Write) -> io::Result<()>`, `delete(out: &mut impl Write) -> io::Result<()>`, `placeholders(area: CellRect, out: &mut impl Write) -> io::Result<()>`.

- [x] **Step 1: Write the failing tests**

`crates/wwt-term/src/graphics/protocol.rs`:

```rust
//! The three sequences and the cells that refer to them.
//!
//! Every sequence carries `q=2`, which suppresses the terminal's success and
//! error replies. A reply would arrive on stdin, where a keystroke lives, and
//! the input pump would have to know to throw it away.

use std::io::{self, Write};

use wwt_frame::CellRect;

use super::diacritics;

/// The one image id wwt uses.
///
/// Fixed rather than rotating, because a new frame transmitted to a live id
/// re-renders the placeholders already on screen, which is what keeps a
/// screencast frame from costing a repaint. Settled by Task 1.
pub const IMAGE_ID: u32 = 0x77_77_74;

/// The placeholder character every cell showing image carries.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// Base64 payload per escape sequence. The protocol's limit is 4096.
const CHUNK: usize = 4096;

/// Send the image data, without placing it.
pub fn transmit(payload: &str, out: &mut impl Write) -> io::Result<()> {
    // An empty payload is not a picture. Sending the sequence anyway would
    // leave a transmission open with `m=1` and no terminator.
    if payload.is_empty() {
        return Ok(());
    }

    let mut chunks = payload.as_bytes().chunks(CHUNK).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = u8::from(chunks.peek().is_some());
        if first {
            // f=100 is PNG, t=d means the payload is in the escape itself.
            write!(out, "\x1b_Gq=2,a=t,f=100,t=d,i={IMAGE_ID},m={more};")?;
            first = false;
        } else {
            // Continuations carry only the chunk marker: the terminal is
            // already holding the transmission this belongs to.
            write!(out, "\x1b_Gq=2,m={more};")?;
        }
        out.write_all(chunk)?;
        out.write_all(b"\x1b\\")?;
    }
    Ok(())
}

/// Create the virtual placement unicode placeholders refer to.
pub fn place(area: CellRect, out: &mut impl Write) -> io::Result<()> {
    write!(
        out,
        "\x1b_Gq=2,a=p,U=1,i={IMAGE_ID},c={},r={}\x1b\\",
        area.cols, area.rows
    )
}

/// Forget the image and every placement of it.
pub fn delete(out: &mut impl Write) -> io::Result<()> {
    write!(out, "\x1b_Gq=2,a=d,d=i,i={IMAGE_ID}\x1b\\")
}

/// Fill `area` with placeholder cells addressing the placement.
///
/// The image id rides in the foreground colour, and the row and column in
/// combining diacritics. Only the first cell of a row spends them: a cell
/// with none continues from the one before it, so a full-width row costs two
/// diacritics rather than two per cell.
pub fn placeholders(area: CellRect, out: &mut impl Write) -> io::Result<()> {
    let id = IMAGE_ID;
    write!(
        out,
        "\x1b[38;2;{};{};{}m",
        (id >> 16) & 0xff,
        (id >> 8) & 0xff,
        id & 0xff
    )?;

    let mut buf = [0u8; 4];
    for row in 0..area.rows {
        // A terminal with more rows than the table can address gets no
        // placeholders for them rather than wrong ones.
        let Some(row_mark) = diacritics::for_index(row) else {
            break;
        };
        let Some(col_mark) = diacritics::for_index(0) else {
            break;
        };

        write!(out, "\x1b[{};{}H", area.row + row + 1, area.col + 1)?;
        out.write_all(PLACEHOLDER.encode_utf8(&mut buf).as_bytes())?;
        out.write_all(row_mark.encode_utf8(&mut buf).as_bytes())?;
        out.write_all(col_mark.encode_utf8(&mut buf).as_bytes())?;
        for _ in 1..area.cols {
            out.write_all(PLACEHOLDER.encode_utf8(&mut buf).as_bytes())?;
        }
    }
    write!(out, "\x1b[0m")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(f: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        f(&mut out).expect("writing to a Vec cannot fail");
        String::from_utf8(out).expect("the protocol is ascii and utf-8 payloads")
    }

    #[test]
    fn a_payload_shorter_than_one_chunk_is_one_terminated_sequence() {
        let sent = bytes(|out| transmit("AAAA", out));
        assert_eq!(sent, "\x1b_Gq=2,a=t,f=100,t=d,i=7829364,m=0;AAAA\x1b\\");
    }

    #[test]
    fn a_payload_longer_than_a_chunk_is_split_and_only_the_last_says_stop() {
        let payload = "x".repeat(CHUNK + 10);
        let sent = bytes(|out| transmit(&payload, out));
        assert_eq!(sent.matches("\x1b_G").count(), 2, "two sequences");
        assert!(sent.contains("m=1;"), "the first says more follows");
        assert!(sent.contains("\x1b_Gq=2,m=0;"), "the last says it is done");
        // The continuation carries no format or id: the terminal already
        // knows what transmission this is.
        assert!(!sent[sent.find("\x1b_Gq=2,m=").unwrap()..].contains("f=100"));
    }

    #[test]
    fn a_payload_of_exactly_one_chunk_does_not_send_an_empty_second() {
        let payload = "y".repeat(CHUNK);
        let sent = bytes(|out| transmit(&payload, out));
        assert_eq!(sent.matches("\x1b_G").count(), 1);
        assert!(sent.contains("m=0;"));
    }

    #[test]
    fn an_empty_payload_sends_nothing_at_all() {
        // Not a picture. A sequence with m=1 and no data would leave the
        // terminal holding a transmission that never ends.
        assert_eq!(bytes(|out| transmit("", out)), "");
    }

    #[test]
    fn a_placement_says_how_many_cells_it_covers() {
        let area = CellRect { col: 0, row: 1, cols: 80, rows: 22 };
        assert_eq!(
            bytes(|out| place(area, out)),
            "\x1b_Gq=2,a=p,U=1,i=7829364,c=80,r=22\x1b\\"
        );
    }

    #[test]
    fn deleting_names_the_image_rather_than_the_screen() {
        assert_eq!(bytes(|out| delete(out)), "\x1b_Gq=2,a=d,d=i,i=7829364\x1b\\");
    }

    #[test]
    fn placeholders_spend_diacritics_only_on_the_first_cell_of_a_row() {
        let area = CellRect { col: 0, row: 1, cols: 4, rows: 2 };
        let sent = bytes(|out| placeholders(area, out));
        assert_eq!(sent.matches(PLACEHOLDER).count(), 8, "one per cell");
        assert_eq!(
            sent.matches(diacritics::CODES[0]).count(),
            3,
            "row 0 and column 0 on the first row, column 0 on the second"
        );
    }

    #[test]
    fn placeholders_carry_the_image_id_in_the_foreground_bytes() {
        let area = CellRect { col: 0, row: 0, cols: 1, rows: 1 };
        let sent = bytes(|out| placeholders(area, out));
        assert!(sent.starts_with("\x1b[38;2;119;119;116m"), "{sent:?}");
    }

    #[test]
    fn a_grid_taller_than_the_table_stops_rather_than_addressing_wrongly() {
        let area = CellRect { col: 0, row: 0, cols: 1, rows: 400 };
        let sent = bytes(|out| placeholders(area, out));
        assert_eq!(sent.matches(PLACEHOLDER).count(), 297);
    }

    #[test]
    fn placeholders_are_addressed_from_the_areas_own_origin() {
        // The page does not start at the top of the screen: the tab bar is
        // row 0, and terminal addressing is 1-based on top of that.
        let area = CellRect { col: 0, row: 1, cols: 1, rows: 1 };
        assert!(bytes(|out| placeholders(area, out)).contains("\x1b[2;1H"));
    }
}
```

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p wwt-term graphics::protocol`
Expected: FAIL to compile, `file not found for module 'protocol'`.

- [x] **Step 3: Declare the module**

In `crates/wwt-term/src/graphics/mod.rs`:

```rust
pub mod diacritics;
pub mod protocol;
```

- [x] **Step 4: Run to verify it passes**

Run: `cargo test -p wwt-term graphics::`
Expected: PASS. `a_payload_shorter_than_one_chunk_is_one_terminated_sequence` asserts the literal id `7829364`, which is `0x777774`; if you changed `IMAGE_ID`, change the assertions with it rather than loosening them.

- [x] **Step 5: Clippy and commit**

```bash
cargo clippy -p wwt-term --all-targets -- -D warnings
git add crates/wwt-term/src/graphics/
git commit -m "feat(term): say an image in the protocol a terminal reads

Three sequences and a chunker, all of them functions from data to bytes,
so all of them are tested with data and none of them needs a terminal.

q=2 on every one of them. The terminal's success and error replies would
arrive on stdin, where a keystroke lives, and the input pump would have
to learn to throw away something that is not a key."
```

---

### Task 5: The renderer places an image

`Renderer` already holds the last frame and writes only what changed. It now holds the last image's generation too, and the rule is the one section 4 of the spec states: placeholders are written when the area changes, and a new frame is a transmission and nothing else.

The order matters and is not arbitrary. Cells are written first and the image second, because the transmission is what the placeholders already on screen re-render against; writing it first would show the previous frame's picture through the new frame's cells for one paint.

**Files:**
- Modify: `crates/wwt-term/src/render.rs`

**Interfaces:**
- Consumes: `graphics::protocol::{transmit, place, delete, placeholders}`, `wwt_frame::Image`.
- Produces: no new public API. `Renderer::render` keeps its signature.

- [x] **Step 1: Write the failing tests**

In `crates/wwt-term/src/render.rs`, in `mod tests`. If the module has no helper for building a frame, add one beside these:

```rust
    fn image_at(generation: u64, payload: &str) -> Image {
        Image {
            generation,
            payload: payload.to_string(),
            area: CellRect { col: 0, row: 1, cols: 4, rows: 2 },
        }
    }

    #[test]
    fn the_first_image_is_transmitted_placed_and_given_placeholders() {
        let mut renderer = Renderer::new();
        let mut frame = Frame::new(GridSize { cols: 4, rows: 4 });
        frame.set_image(Some(image_at(1, "AAAA")));

        let mut out = Vec::new();
        renderer.render(&frame, &mut out).expect("render");
        let sent = String::from_utf8(out).expect("utf-8");

        assert!(sent.contains("a=t,f=100"), "transmitted");
        assert!(sent.contains("a=p,U=1"), "placed");
        assert!(sent.contains(graphics::protocol::PLACEHOLDER), "placeholders written");
    }

    #[test]
    fn a_new_frame_of_the_same_size_is_a_transmission_and_no_cells() {
        // The whole latency claim of pixel mode. A scroll must not repaint.
        let mut renderer = Renderer::new();
        let mut first = Frame::new(GridSize { cols: 4, rows: 4 });
        first.set_image(Some(image_at(1, "AAAA")));
        renderer.render(&first, &mut Vec::new()).expect("first");

        let mut second = Frame::new(GridSize { cols: 4, rows: 4 });
        second.set_image(Some(image_at(2, "BBBB")));
        let mut out = Vec::new();
        renderer.render(&second, &mut out).expect("second");
        let sent = String::from_utf8(out).expect("utf-8");

        assert!(sent.contains("BBBB"), "the new data went out");
        assert!(
            !sent.contains(graphics::protocol::PLACEHOLDER),
            "a new frame must not rewrite a single placeholder cell"
        );
        assert!(!sent.contains("a=p,U=1"), "nor re-place it");
    }

    #[test]
    fn an_unchanged_generation_sends_no_image_at_all() {
        let mut renderer = Renderer::new();
        let mut frame = Frame::new(GridSize { cols: 4, rows: 4 });
        frame.set_image(Some(image_at(1, "AAAA")));
        renderer.render(&frame, &mut Vec::new()).expect("first");

        let mut out = Vec::new();
        renderer.render(&frame, &mut out).expect("again");
        let sent = String::from_utf8(out).expect("utf-8");
        assert!(!sent.contains("\x1b_G"), "nothing about graphics was said");
    }

    #[test]
    fn a_changed_area_writes_placeholders_again() {
        // A resize. The placement covers a different number of cells, so the
        // cells that refer to it have to be laid down again.
        let mut renderer = Renderer::new();
        let mut first = Frame::new(GridSize { cols: 4, rows: 4 });
        first.set_image(Some(image_at(1, "AAAA")));
        renderer.render(&first, &mut Vec::new()).expect("first");

        let mut second = Frame::new(GridSize { cols: 8, rows: 6 });
        let mut image = image_at(2, "BBBB");
        image.area = CellRect { col: 0, row: 1, cols: 8, rows: 4 };
        second.set_image(Some(image));
        let mut out = Vec::new();
        renderer.render(&second, &mut out).expect("second");
        let sent = String::from_utf8(out).expect("utf-8");

        assert!(sent.contains("a=p,U=1"), "re-placed at the new size");
        assert!(sent.contains(graphics::protocol::PLACEHOLDER), "and re-addressed");
    }

    #[test]
    fn dropping_the_image_deletes_it_from_the_terminal() {
        // Leaving pixel mode. Nothing is left in the terminal's memory for a
        // mode nobody is in.
        let mut renderer = Renderer::new();
        let mut first = Frame::new(GridSize { cols: 4, rows: 4 });
        first.set_image(Some(image_at(1, "AAAA")));
        renderer.render(&first, &mut Vec::new()).expect("first");

        let second = Frame::new(GridSize { cols: 4, rows: 4 });
        let mut out = Vec::new();
        renderer.render(&second, &mut out).expect("second");
        assert!(String::from_utf8(out).expect("utf-8").contains("a=d,d=i"));
    }

    #[test]
    fn a_text_frame_says_nothing_about_graphics() {
        // Text mode is every frame this codebase built before M5 and must
        // cost exactly what it cost then.
        let mut renderer = Renderer::new();
        let frame = Frame::new(GridSize { cols: 4, rows: 4 });
        let mut out = Vec::new();
        renderer.render(&frame, &mut out).expect("render");
        assert!(!String::from_utf8(out).expect("utf-8").contains("\x1b_G"));
    }
```

- [x] **Step 2: Run to verify they fail**

Run: `cargo test -p wwt-term render::`
Expected: FAIL, `cannot find value 'graphics' in this scope` and the image assertions unmet.

- [x] **Step 3: Teach the renderer**

In `crates/wwt-term/src/render.rs`, add the import and the remembered state:

```rust
use wwt_frame::{CellRect, Image};

use crate::graphics::protocol;

pub struct Renderer {
    last: Option<Frame>,
    /// What the terminal is currently holding, so a frame that changed
    /// nothing costs no sequence and a frame that changed only its data
    /// costs no cells.
    shown: Option<(u64, CellRect)>,
}
```

`Renderer::new` initialises `shown: None`, and `invalidate` clears it, because after a resize the terminal has been written to behind our back and nothing can be assumed to still be placed:

```rust
    pub fn invalidate(&mut self) {
        self.last = None;
        self.shown = None;
    }
```

Then, inside `render`, after the cells have been written and before the cursor is placed:

```rust
        // Cells first, image second. The transmission is what the
        // placeholders already on screen re-render against, so sending it
        // first would show this frame's picture through the last frame's
        // cells for one paint.
        let touched_image = self.paint_image(frame.image(), out)?;
```

and include `touched_image` in the existing decision about whether to place the cursor and flush:

```rust
        if wrote || moved || touched_image {
            place_cursor(frame, out)?;
            out.flush()?;
        }
```

The method itself:

```rust
    /// Bring the terminal's idea of the image up to date with this frame's.
    ///
    /// Returns whether anything was written. Three cases and they are the
    /// whole protocol policy: no image is a delete if one was showing, the
    /// same generation is nothing at all, and a new generation is a
    /// transmission plus, only when the area moved, a placement and the
    /// cells that address it.
    fn paint_image(&mut self, image: Option<&Image>, out: &mut impl Write) -> std::io::Result<bool> {
        let Some(image) = image else {
            if self.shown.take().is_some() {
                protocol::delete(out)?;
                return Ok(true);
            }
            return Ok(false);
        };

        if self.shown == Some((image.generation, image.area)) {
            return Ok(false);
        }

        let replaced = self.shown.map(|(_, area)| area) != Some(image.area);
        protocol::transmit(&image.payload, out)?;
        if replaced {
            protocol::place(image.area, out)?;
            protocol::placeholders(image.area, out)?;
        }
        self.shown = Some((image.generation, image.area));
        Ok(true)
    }
```

- [x] **Step 4: Run to verify they pass**

Run: `cargo test -p wwt-term`
Expected: PASS, including every pre-existing diffing test. If `a_new_frame_of_the_same_size_is_a_transmission_and_no_cells` fails, Task 1's answer was that re-transmission does not update in place: amend the spec's section 4 and change `replaced` to always be true, and say so in this task's commit body.

- [x] **Step 5: Clippy and commit**

```bash
cargo clippy -p wwt-term --all-targets -- -D warnings
git add crates/wwt-term/src/render.rs
git commit -m "feat(term): put the picture on screen and leave it there

The renderer holds what the terminal is showing, so the three cases are
the whole policy: no image deletes one that was showing, an unchanged
generation says nothing at all, and a new generation is a transmission
and, only when the area moved, a placement and the cells addressing it.

A scroll in pixel mode therefore costs one escape sequence and not one
cell, which is the latency claim the mode is worth having for. Cells are
written before the image because the transmission is what the
placeholders on screen re-render against."
```

---

### Task 6: Asking the terminal whether it can do this at all

One question, one timeout, once. The arithmetic is separable from the I/O and is tested without a terminal; the I/O is thin enough to read.

The query is a transmission of a one-pixel image with `q=0`, which a terminal that implements the protocol answers with `\x1b_Gi=<id>;OK\x1b\`. A terminal that does not implement it answers nothing, and the unknown escape is swallowed rather than printed.

**Files:**
- Create: `crates/wwt-term/src/graphics/detect.rs`
- Modify: `crates/wwt-term/src/graphics/mod.rs`
- Modify: `crates/wwt/src/main.rs`

**Interfaces:**
- Produces: `graphics::detect::reply_is_support(reply: &str) -> bool`, `graphics::detect::query(timeout: Duration) -> bool`.

- [x] **Step 1: Write the failing test**

`crates/wwt-term/src/graphics/detect.rs`:

```rust
//! Whether this terminal can show an image, asked once.
//!
//! `CLAUDE.md` rejects `supports_keyboard_enhancement` for taking stdin for
//! up to two seconds on every run. The objection is the two seconds and
//! whose the timeout is, not the asking: this asks once, before the first
//! paint, and gives up after a window we choose.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use super::protocol::IMAGE_ID;

/// How long a terminal gets to answer. Long enough for a local terminal and
/// for one at the far end of an ssh connection on a bad day, short enough
/// that a terminal which will never answer does not delay the first frame
/// enough to notice.
pub const WINDOW: Duration = Duration::from_millis(100);

/// A 1x1 PNG, base64. The smallest thing that is a picture, sent only so
/// that a terminal has something to say OK about.
const PROBE_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

/// Whether what came back is a terminal saying it can do this.
///
/// Separated from the reading so the decision can be tested with data. A
/// terminal that answers at all implements the protocol; one that answers
/// with an error still implements it, but not for this image, and for our
/// purposes that is not support.
pub fn reply_is_support(reply: &str) -> bool {
    reply.contains(&format!("\x1b_Gi={IMAGE_ID}")) && reply.contains("OK")
}

/// Ask, and wait `timeout` for an answer.
///
/// Silence is not support. This is the one place that reads stdin outside
/// the input pump, and it runs before the pump exists.
pub fn query(timeout: Duration) -> bool {
    let mut out = std::io::stdout();
    // q=0: we want the reply here, unlike every other sequence we send.
    if write!(
        out,
        "\x1b_Gi={IMAGE_ID},a=q,f=100,t=d,m=0;{PROBE_PNG}\x1b\\"
    )
    .is_err()
        || out.flush().is_err()
    {
        return false;
    }

    let mut reply = String::new();
    let mut buf = [0u8; 64];
    let deadline = Instant::now() + timeout;
    let mut stdin = std::io::stdin();
    while Instant::now() < deadline {
        match stdin.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                reply.push_str(&String::from_utf8_lossy(&buf[..n]));
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
        assert!(!reply_is_support("\x1b_Gi=7829364;ENOTSUPPORTED:whatever\x1b\\"));
    }
}
```

- [x] **Step 2: Run to verify it fails, then declare the module**

Run: `cargo test -p wwt-term graphics::detect`
Expected: FAIL, module not found. Add `pub mod detect;` to `crates/wwt-term/src/graphics/mod.rs`, then re-run and expect PASS, 4 tests.

- [x] **Step 3: Ask once at startup**

In `crates/wwt/src/main.rs`, after the terminal probe and before raw mode is enabled, ask and carry the answer into `Startup`:

```rust
    // Asked before raw mode, before the alternate screen, and before the
    // first paint: the one moment stdin is nobody else's. After the input
    // pump exists, a reply would be a keystroke.
    let graphics = wwt_term::graphics::detect::query(wwt_term::graphics::detect::WINDOW);
```

Add `graphics: bool` to `Startup` and pass it through to `Session`, beside the grid and cell size it already carries. A session that was told the terminal cannot do graphics refuses pixel mode in Task 8.

- [x] **Step 4: Verify the workspace still builds and behaves**

Run: `cargo test --workspace`
Expected: PASS. `Startup` gained a field, so every construction of it in tests needs the field; set it to `false` in existing tests, which is what a test with no terminal should say.

- [x] **Step 5: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt-term/src/graphics/detect.rs crates/wwt-term/src/graphics/mod.rs crates/wwt/src/main.rs
git commit -m "feat(term): ask once whether this terminal can show a picture

Before raw mode, before the alternate screen and before the first paint,
which is the one moment stdin belongs to nobody: after the input pump
exists a reply would be a keystroke.

The objection CLAUDE.md records to supports_keyboard_enhancement is the
two seconds and whose the timeout is, not the asking. This one is ours
and it is a hundred milliseconds. Silence is not support, and deciding
that is a function of a string so it is tested as one."
```

---

### Task 7: A page that screencasts

The page side is four calls and one predicate. The predicate is the same shape as `is_dirty`, and for the same reason: one browser serves every page and they all report on one subscription, so the session id is half the question.

**Files:**
- Create: `crates/wwt-page/src/screencast.rs`
- Modify: `crates/wwt-page/src/lib.rs`
- Create: `crates/wwt-page/tests/screencast.rs`

**Interfaces:**
- Produces: `Page::start_screencast(&self, vp: Viewport) -> Result<()>`, `Page::stop_screencast(&self) -> Result<()>`, `Page::ack_frame(&self, ack: i64) -> Result<()>`, `Page::screencast_frame(&self, event: &Event) -> Option<ScreencastFrame>`, and `pub struct ScreencastFrame { pub data: String, pub ack: i64 }`.

- [x] **Step 1: Write the failing integration test**

`crates/wwt-page/tests/screencast.rs`. It uses the shared harness in `tests/common`, which hands out one Chromium a test at a time:

```rust
//! What a screencast does, against a real browser.

mod common;

use std::time::Duration;

use wwt_frame::{CellSize, GridSize, Viewport};

fn viewport() -> Viewport {
    Viewport::with_origin(GridSize { cols: 80, rows: 22 }, CellSize { w: 9, h: 20 }, 1)
}

#[tokio::test]
async fn a_started_screencast_produces_a_frame() {
    let browser = common::browser().await;
    let page = browser.page("data:text/html,<h1>hello</h1>").await;
    let mut events = browser.client().subscribe();

    page.start_screencast(viewport())
        .await
        .expect("start the screencast");

    let frame = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.expect("the browser is still there");
            if let Some(frame) = page.screencast_frame(&event) {
                return frame;
            }
        }
    })
    .await
    .expect("a frame within five seconds");

    assert!(!frame.data.is_empty(), "a frame carries a picture");
    // Base64 PNG and nothing else: the whole design rests on never having
    // to decode this, so it must arrive already encoded.
    assert!(frame.data.starts_with("iVBOR"), "base64 PNG: {:?}", &frame.data[..8]);

    page.ack_frame(frame.ack).await.expect("ack it");
    page.stop_screencast().await.expect("stop it");
}

#[tokio::test]
async fn a_screencast_keeps_producing_frames_while_they_are_acked() {
    // Chromium stops sending after one unacked frame, so this is the test
    // that catches forgetting the ack: without it the second never arrives.
    let browser = common::browser().await;
    let page = browser
        .page("data:text/html,<body><div id=x>a</div><script>setInterval(()=>{document.getElementById('x').textContent=Math.random()},50)</script></body>")
        .await;
    let mut events = browser.client().subscribe();

    page.start_screencast(viewport()).await.expect("start");

    let mut seen = 0;
    tokio::time::timeout(Duration::from_secs(10), async {
        while seen < 3 {
            let event = events.recv().await.expect("the browser is still there");
            if let Some(frame) = page.screencast_frame(&event) {
                seen += 1;
                page.ack_frame(frame.ack).await.expect("ack it");
            }
        }
    })
    .await
    .expect("three frames within ten seconds");

    page.stop_screencast().await.expect("stop");
}

#[tokio::test]
async fn a_frame_from_another_page_is_not_this_ones() {
    // One browser, one subscription, several pages. Without the session id
    // the wrong tab's picture lands on the tab in front.
    let browser = common::browser().await;
    let one = browser.page("data:text/html,<h1>one</h1>").await;
    let two = browser.page("data:text/html,<h1>two</h1>").await;
    let mut events = browser.client().subscribe();

    two.start_screencast(viewport()).await.expect("start");

    let event = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.expect("the browser is still there");
            if two.screencast_frame(&event).is_some() {
                return event;
            }
        }
    })
    .await
    .expect("a frame within five seconds");

    assert!(
        one.screencast_frame(&event).is_none(),
        "a frame belongs to the page whose session it names"
    );
    two.stop_screencast().await.expect("stop");
}
```

Match `common::browser()` and its `page(...)`/`client()` helpers to whatever the existing `wwt-page/tests/common` module actually exposes; the other test binaries in that directory are the reference. Do not add a second harness.

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p wwt-page --test screencast`
Expected: FAIL to compile, `no method named 'start_screencast'`.

- [x] **Step 3: Implement the page side**

`crates/wwt-page/src/screencast.rs`:

```rust
//! Asking a page for a picture of itself, repeatedly.
//!
//! Four calls and a predicate. Nothing here decodes anything: a frame's
//! data arrives base64 and leaves base64, which is why pixel mode costs no
//! dependency.

use anyhow::{Context, Result};
use serde_json::json;
use wwt_cdp::Event;
use wwt_frame::Viewport;

use crate::extract::Page;

/// One picture of a page, on its way to the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreencastFrame {
    /// Base64 PNG, exactly as CDP sent it and exactly as the graphics
    /// protocol wants it.
    pub data: String,
    /// What the ack must quote back.
    ///
    /// CDP calls this field `sessionId` and it is not a CDP session id: it
    /// is an integer counting screencasts on one target. `wwt-cdp` already
    /// means something else by session, so this is the ack id.
    pub ack: i64,
}

impl Page {
    /// Start sending pictures of this page, at the size it is already laid
    /// out for.
    ///
    /// PNG rather than JPEG: a lossy picture of text is the one thing a
    /// browser in a terminal must not produce, and both ends of this
    /// pipeline already speak PNG.
    pub async fn start_screencast(&self, vp: Viewport) -> Result<()> {
        self.client()
            .call_on(
                self.session_id(),
                "Page.startScreencast",
                json!({
                    "format": "png",
                    "maxWidth": vp.css_width(),
                    "maxHeight": vp.css_height(),
                    "everyNthFrame": 1,
                }),
            )
            .await
            .context("start the screencast")?;
        Ok(())
    }

    pub async fn stop_screencast(&self) -> Result<()> {
        self.client()
            .call_on(self.session_id(), "Page.stopScreencast", json!({}))
            .await
            .context("stop the screencast")?;
        Ok(())
    }

    /// Tell the page the frame arrived.
    ///
    /// Not optional and not batchable: Chromium sends the next frame only
    /// after the last one is acked, so a dropped ack is a screencast that
    /// stops after exactly one picture.
    pub async fn ack_frame(&self, ack: i64) -> Result<()> {
        self.client()
            .call_on(
                self.session_id(),
                "Page.screencastFrameAck",
                json!({ "sessionId": ack }),
            )
            .await
            .context("ack the screencast frame")?;
        Ok(())
    }

    /// Whether a CDP event is a picture of this page, and what is in it.
    ///
    /// The session id is half the question, exactly as it is for the dirty
    /// signal: one browser serves every page and they all report on one
    /// subscription.
    pub fn screencast_frame(&self, event: &Event) -> Option<ScreencastFrame> {
        if event.session_id.as_deref() != Some(self.session_id())
            || event.method != "Page.screencastFrame"
        {
            return None;
        }
        Some(ScreencastFrame {
            data: event.params["data"].as_str()?.to_string(),
            ack: event.params["sessionId"].as_i64()?,
        })
    }
}
```

`Page`'s fields are private to `extract.rs`. Add whatever crate-visible accessors this needs there (`pub(crate) fn client(&self) -> &Arc<Client>` and `pub(crate) fn session_id(&self) -> &str`) rather than making the fields public, and declare `pub mod screencast;` plus `pub use screencast::ScreencastFrame;` in `crates/wwt-page/src/lib.rs`.

- [x] **Step 4: Run to verify it passes**

Run: `cargo test -p wwt-page --test screencast`
Expected: PASS, 3 tests. If `a_started_screencast_produces_a_frame` times out, the target is not the one the browser has in front: M4's rule that switching activates applies here too, and the harness may need `page.activate().await` before starting.

- [x] **Step 5: Clippy and commit**

```bash
cargo clippy -p wwt-page --all-targets -- -D warnings
git add crates/wwt-page/src/screencast.rs crates/wwt-page/src/lib.rs crates/wwt-page/src/extract.rs crates/wwt-page/tests/screencast.rs
git commit -m "feat(page): ask a page for a picture of itself

Four calls and a predicate, and nothing that decodes: a frame's data
arrives base64 and leaves base64, which is the reason this milestone
adds no dependency.

The ack is not optional. Chromium sends the next frame only once the
last is acked, so the test that proves the ack is wired is the one
that waits for a third frame rather than a first."
```

---

### Task 8: The session learns pixel mode

One flag, one key, one command, and what compose does with it. All of it is decisions, so all of it is tested with no browser and no terminal.

**Files:**
- Modify: `crates/wwt/src/session.rs`
- Modify: `crates/wwt/src/effect.rs`
- Modify: `crates/wwt/src/event.rs`
- Modify: `crates/wwt/src/keymap.rs`
- Modify: `crates/wwt-ui/src/command.rs`
- Modify: `crates/wwt-ui/src/chrome.rs`

**Interfaces:**
- Consumes: `wwt_frame::{CellRect, Image}`, `wwt_page::ScreencastFrame`.
- Produces: `Effect::StartScreencast(TabId)`, `Effect::StopScreencast(TabId)`, `Effect::AckFrame(TabId, i64)`, `Event::Frame(TabId, Box<ScreencastFrame>)`, `Setting::Pixel(bool)`, `Action::TogglePixel`.

- [x] **Step 1: Add the vocabulary**

In `crates/wwt/src/effect.rs`, inside `enum Effect`:

```rust
    /// Start sending pictures of this tab. Only ever the focused one: a
    /// background tab is idle, which is the same rule extraction follows.
    StartScreencast(TabId),
    StopScreencast(TabId),
    /// Tell the page a picture arrived, so it sends the next one. Carries
    /// the ack id from the frame, which is not a CDP session id.
    AckFrame(TabId, i64),
```

In `crates/wwt/src/event.rs`, inside `enum Event`:

```rust
    /// A picture of a page. Which page matters for the same reason a dirty
    /// signal's does: one browser serves all of them.
    Frame(TabId, Box<ScreencastFrame>),
```

Boxed because `Event` is moved on every keystroke and a frame's payload is the largest thing that can be in one.

In `crates/wwt-ui/src/command.rs`, inside `enum Setting` and beside the `mouse` arms:

```rust
                ("pixel", "on") => Ok(Command::Set(Setting::Pixel(true))),
                ("pixel", "off") => Ok(Command::Set(Setting::Pixel(false))),
                ("pixel", other) => Err(format!("set pixel takes on or off, not {other:?}")),
```

In `crates/wwt/src/keymap.rs`, in `fn normal`, beside the other single letters:

```rust
        KeyCode::Char('p') => Some(Action::TogglePixel),
```

- [x] **Step 2: Write the failing tests**

In `crates/wwt/src/session.rs`, in `mod tests`:

```rust
    #[test]
    fn p_turns_pixel_mode_on_and_asks_for_pictures() {
        let mut session = ready_with_graphics();
        let effects = session.on(key('p'));
        assert!(session.pixel);
        assert!(matches!(effects.as_slice(), [Effect::StartScreencast(_)]));
    }

    #[test]
    fn p_again_turns_it_off_and_stops_them() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let effects = session.on(key('p'));
        assert!(!session.pixel);
        assert!(matches!(effects.as_slice(), [Effect::StopScreencast(_)]));
    }

    #[test]
    fn p_without_a_terminal_that_can_show_pictures_is_a_notice() {
        // Never blank the frame you are looking at, and never emit escapes
        // a terminal cannot read. Section 5 of the M5 spec.
        let mut session = ready();
        let effects = session.on(key('p'));
        assert!(!session.pixel);
        assert!(effects.is_empty());
        assert!(session.notice.is_some(), "it says why");
    }

    #[test]
    fn a_frame_becomes_the_image_on_the_next_compose() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let id = session.tabs[0].id;
        session.on(Event::Frame(id, Box::new(frame_data("AAAA"))));

        let frame = session.compose();
        let image = frame.image().expect("pixel mode composes an image");
        assert_eq!(image.payload, "AAAA");
        assert_eq!(image.area.row, 1, "the page starts below the tab bar");
    }

    #[test]
    fn every_frame_is_acked_even_when_it_is_not_shown() {
        // Chromium sends the next only once the last is acked, so a frame
        // that arrives for the wrong tab still has to be answered.
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let id = session.tabs[0].id;
        let effects = session.on(Event::Frame(id, Box::new(frame_data("AAAA"))));
        assert!(effects.iter().any(|e| matches!(e, Effect::AckFrame(_, 7))));
    }

    #[test]
    fn a_frame_for_a_tab_that_is_not_focused_is_acked_and_dropped() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        open_two_more(&mut session);
        let background = session.tabs[1].id;
        let effects = session.on(Event::Frame(background, Box::new(frame_data("BBBB"))));

        assert!(effects.iter().any(|e| matches!(e, Effect::AckFrame(_, 7))));
        assert!(
            session.compose().image().is_none_or(|i| i.payload != "BBBB"),
            "the tab you are not looking at does not paint"
        );
    }

    #[test]
    fn each_frame_composes_a_new_generation() {
        // The renderer diffs on this. Two frames that encode identically
        // are still two frames and both must reach the terminal.
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let id = session.tabs[0].id;
        session.on(Event::Frame(id, Box::new(frame_data("SAME"))));
        let first = session.compose().image().expect("an image").generation;
        session.on(Event::Frame(id, Box::new(frame_data("SAME"))));
        let second = session.compose().image().expect("an image").generation;
        assert_ne!(first, second);
    }

    #[test]
    fn pixel_mode_paints_no_runs() {
        // The picture is the page. Painting runs underneath would show text
        // through every cell the image does not cover.
        let mut session = ready_with_graphics();
        session.tabs[0].runs = vec![run("hello")];
        session.on(key('p'));
        let id = session.tabs[0].id;
        session.on(Event::Frame(id, Box::new(frame_data("AAAA"))));

        let frame = session.compose();
        assert_eq!(frame.cell(CellPos { col: 0, row: 1 }).map(|c| c.ch), Some(' '));
    }

    #[test]
    fn a_hint_label_is_painted_over_the_picture() {
        // Unicode placeholders make placement cell content, so a glyph in
        // the page area wins. This is why f keeps working in pixel mode.
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let id = session.tabs[0].id;
        session.on(Event::Frame(id, Box::new(frame_data("AAAA"))));
        session.on(Event::Done(Job::Hints(id, Ok(vec![hint_at(0.0, 20.0)]))));

        let frame = session.compose();
        assert!(frame.image().is_some(), "the picture is still there");
        assert_ne!(
            frame.cell(CellPos { col: 0, row: 1 }).map(|c| c.ch),
            Some(' '),
            "and the label is on top of it"
        );
    }

    #[test]
    fn insert_mode_over_a_picture_shows_the_pages_caret_and_not_ours() {
        // Section 6 of the M5 spec. The page drew its own caret into the
        // picture; a second one placed by us would disagree with it.
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let id = session.focused().id;
        session.on(Event::Frame(id, Box::new(frame_data("AAAA"))));
        session.tabs[0].caret = Some(caret_at(0.0, 20.0));
        session.on(key('i'));
        assert_eq!(session.compose().cursor(), None);
    }

    #[test]
    fn the_command_line_keeps_its_caret_in_pixel_mode() {
        // It is painted into a chrome row, which no image ever covers.
        let mut session = ready_with_graphics();
        session.on(key('p'));
        session.on(key(':'));
        assert!(session.compose().cursor().is_some());
    }

    #[test]
    fn text_mode_composes_no_image_at_all() {
        let mut session = ready_with_graphics();
        assert!(session.compose().image().is_none());
    }

    #[test]
    fn set_pixel_off_leaves_pixel_mode() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        run_command(&mut session, "set pixel off");
        assert!(!session.pixel);
        assert!(session.compose().image().is_none(), "and drops the picture");
    }
```

Add the three helpers beside the existing ones in that module, following whatever `ready()`, `run()`, `hint_at()` and `run_command()` already look like:

```rust
    /// A ready session on a terminal that can show pictures.
    fn ready_with_graphics() -> Session {
        let mut session = ready();
        session.graphics = true;
        session
    }

    fn frame_data(payload: &str) -> ScreencastFrame {
        ScreencastFrame { data: payload.to_string(), ack: 7 }
    }

    /// A caret on the first line of the page, for asserting where the
    /// terminal's cursor does and does not go.
    fn caret_at(x: f64, baseline: f64) -> Caret {
        Caret { x, baseline, offset: 0 }
    }
```

`Caret`'s fields are `wwt-frame`'s; build it the way `caret.rs` and the existing insert-mode tests in `session.rs` already do rather than assuming these three names.

- [x] **Step 3: Run to verify they fail**

Run: `cargo test -p wwt --lib session::`
Expected: FAIL, `no field 'pixel' on type 'Session'`.

- [x] **Step 4: Implement**

In `Session`:

```rust
pub struct Session {
    // ... existing fields ...
    /// Whether the terminal can show a picture at all, asked once at
    /// startup. A session that was told no refuses pixel mode rather than
    /// emitting escapes into a terminal that would print them.
    graphics: bool,
    /// Global rather than per-tab: only the focused tab screencasts either
    /// way, so per-tab would buy a preference rather than a cost, and it
    /// would have to be remembered in the session file, which is a snapshot
    /// version bump and a rejected file for everyone who upgrades.
    pixel: bool,
    /// The picture last received for the focused tab, and its generation.
    /// Not on `Tab`: a background tab does not screencast, so there is
    /// never a second one to hold.
    picture: Option<Image>,
    generations: u64,
}
```

`Action::TogglePixel` is interpreted where the other actions are:

```rust
            Action::TogglePixel => self.set_pixel(!self.pixel, effects),
```

and `Setting::Pixel(on)` calls the same method, so the key and the command cannot drift:

```rust
    /// Enter or leave pixel mode.
    ///
    /// Refusing is a notice and nothing else: the mode does not change and
    /// the frame you are looking at stands, per section 8 of the parent
    /// spec. Half-block would have been the third answer here and is M6's.
    fn set_pixel(&mut self, on: bool, effects: &mut Vec<Effect>) {
        if on && !self.graphics {
            self.notice("pixel mode needs a terminal that can show images");
            return;
        }
        if on == self.pixel {
            return;
        }
        self.pixel = on;
        let id = self.focused().id;
        if on {
            effects.push(Effect::StartScreencast(id));
        } else {
            // The picture goes with the mode, so the next compose carries
            // none and the renderer deletes it from the terminal.
            self.picture = None;
            effects.push(Effect::StopScreencast(id));
        }
    }
```

A frame is handled in `on`:

```rust
            Event::Frame(id, frame) => {
                // Acked whatever happens to it. Chromium sends the next only
                // once this one is answered, so a frame we drop still has to
                // be answered or the screencast stops.
                effects.push(Effect::AckFrame(id, frame.ack));
                if self.pixel && self.focused().id == id {
                    self.generations += 1;
                    self.picture = Some(Image {
                        generation: self.generations,
                        payload: frame.data,
                        area: CellRect::of(self.vp.grid(), self.vp.origin_row()),
                    });
                }
            }
```

And `compose` gains the fork:

```rust
    pub fn compose(&self) -> Frame {
        let mut frame = Frame::new(self.grid);
        let tab = self.focused();

        // The picture is the page. Runs painted underneath would show text
        // through every cell the image does not cover, and in pixel mode
        // every cell it covers is every cell of the page.
        if self.pixel {
            frame.set_image(self.picture.clone());
        } else {
            frame.paint_runs(&self.vp, &tab.runs);
        }

        // ... the hint block and the chrome block are unchanged. Labels are
        // painted after either branch and win over both, which is what
        // makes f work in pixel mode.
```

The cursor block gains one condition, per section 6 of the spec. In pixel mode the page
draws its own caret into the picture, so placing the terminal's on top of it is two
carets disagreeing about where the insertion point is:

```rust
        frame.set_cursor(match &self.mode {
            // In pixel mode the page's own caret is in the picture already.
            Mode::Insert if self.pixel => None,
            Mode::Insert => tab.caret.and_then(|caret| caret.cell(&self.vp)),
            Mode::Command(buffer) => chrome::command_caret(buffer, self.grid),
            Mode::Normal | Mode::Hint(_) => None,
        });
```

The command line keeps its caret in both modes: it is painted into a chrome row, which
no image ever covers.

The statusline says so: pass `pixel: self.pixel` into `Chrome` and have `chrome::paint` show `pixel` beside the mode when it is on, and nothing when it is off, because text is the default and a statusline that names the normal case wastes the row.

- [x] **Step 5: Run to verify they pass**

Run: `cargo test -p wwt --lib`
Expected: PASS. `measure_switch` must still pass untouched: it runs in text mode and nothing about it changed.

- [x] **Step 6: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt/src/session.rs crates/wwt/src/effect.rs crates/wwt/src/event.rs crates/wwt/src/keymap.rs crates/wwt-ui/src/command.rs crates/wwt-ui/src/chrome.rs
git commit -m "feat(wwt): let p show the page as it really looks

One flag, and compose forks on it: the picture is the page, so runs are
not painted underneath where they would show through every cell the
image does not cover. Labels are painted after either branch and win
over both, which is the whole of why f keeps working in pixel mode.

Every frame is acked whatever becomes of it, including one for a tab
you have already switched away from. Chromium sends the next only once
the last is answered, so a frame dropped without an ack is a screencast
that stops after one picture."
```

---

### Task 9: The loop carries frames

`Core` gains three spawns and one more question to ask of a CDP event. It still decides nothing: which tab screencasts and what a frame becomes are Task 8's, and this task is the adapter.

The one piece of policy that belongs here rather than in `Session` is dropping a late frame. It is not a decision about browsing, it is a fact about a channel: if two frames are waiting, the older is already wrong.

**Files:**
- Modify: `crates/wwt/src/core.rs`

**Interfaces:**
- Consumes: `Effect::{StartScreencast, StopScreencast, AckFrame}`, `Event::Frame`, `Page::{start_screencast, stop_screencast, ack_frame, screencast_frame}`.

- [x] **Step 1: Route the frame event**

In `Core::run`'s `select!`, the CDP arm asks two questions of an event today. It now asks three, cheapest first, which is the order every pass in this codebase uses:

```rust
                // Three questions of one event, in this order. A target we
                // never asked for belongs to no page yet, so asking the
                // pages about it first would only ever answer no; and a
                // frame is much more frequent than either, so it is asked
                // before the dirty signal it would otherwise queue behind.
                Some(event) = cdp.recv() => match Client::opened_by_a_page(&event) {
                    Some(attached) => Some(Incoming::Event(Event::TargetOpened(attached))),
                    None => self
                        .pages
                        .iter()
                        .find_map(|(id, page)| {
                            page.screencast_frame(&event)
                                .map(|frame| Incoming::Event(Event::Frame(*id, Box::new(frame))))
                        })
                        .or_else(|| {
                            self.pages
                                .iter()
                                .find(|(_, page)| page.is_dirty(&event))
                                .map(|(id, _)| Incoming::Event(Event::Dirty(*id)))
                        }),
                },
```

- [x] **Step 2: Spawn the three effects**

In `Core::apply`, beside the existing arms. Each says what its failure means by choosing the job it reports:

```rust
            Effect::StartScreencast(id) => {
                let vp = self.session.page_vp();
                self.spawn(id, move |page| async move {
                    match page.start_screencast(vp).await {
                        Ok(()) => None,
                        // A screencast that will not start is worth saying
                        // out loud: the mode is on and no picture is coming.
                        Err(error) => Some(Job::Noted(id, error.to_string())),
                    }
                });
            }
            Effect::StopScreencast(id) => {
                self.spawn(id, move |page| async move {
                    // Failing to stop is not worth a word. The mode is
                    // already off, the image is already deleted, and the
                    // frames that keep arriving are acked and dropped.
                    let _ = page.stop_screencast().await;
                    None
                });
            }
            Effect::AckFrame(id, ack) => {
                self.spawn(id, move |page| async move {
                    // A failed ack stops the screencast, which is visible
                    // as a picture that stopped moving rather than as an
                    // error, so it is worth naming.
                    match page.ack_frame(ack).await {
                        Ok(()) => None,
                        Err(error) => Some(Job::Noted(id, error.to_string())),
                    }
                });
            }
```

`page_vp()` is whatever accessor `Session` already exposes for the page viewport; if there is none, add `pub fn page_vp(&self) -> Viewport { self.vp }` beside the other accessors.

- [x] **Step 3: Nothing coalesces, and here is why**

The spec's section 3 says a late frame is dropped rather than queued. Reading the ack
protocol closely, there is nothing to drop: Chromium sends the next frame only once the
last is acked, so **at most one frame is ever in flight**. The ack is the backpressure,
and a coalescing buffer here would be machinery guarding against a case the protocol
already prevents.

Sixty frames a second would cost sixty composes, and a compose is ~40µs against a frame
that is hundreds of kilobytes to write, so the loop is not where that time goes either.

**Write no coalescing code.** Sections 3 and 10 of the spec already say this; they were
corrected when the plan was written, so there is no amendment owing here. If the manual
pass in Task 11 finds a page that genuinely outruns the loop, the knob is
`everyNthFrame` in `start_screencast` and not a buffer in the loop.

- [x] **Step 4: Write the test that catches a dropped ack**

The one property worth holding down here is that every frame is answered, whatever
becomes of the picture in it. In `crates/wwt/src/session.rs`, in `mod tests`, because
this is the session's rule and needs no browser:

```rust
    #[test]
    fn a_frame_is_acked_even_when_pixel_mode_has_already_been_left() {
        // Stopping is not instant: the frame in flight when p was pressed
        // still arrives. Chromium counts acks and not paints, so failing to
        // answer it leaves the screencast stopped in a way that only shows
        // up as a picture that never moves the next time p is pressed.
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let id = session.focused().id;
        session.on(key('p'));

        let effects = session.on(Event::Frame(id, Box::new(frame_data("LATE"))));
        assert!(
            effects.iter().any(|e| matches!(e, Effect::AckFrame(_, 7))),
            "a frame that arrives after the mode is off is still answered"
        );
        assert!(session.compose().image().is_none(), "and is not shown");
    }

    #[test]
    fn a_frame_for_a_tab_that_has_been_closed_is_dropped_without_a_panic() {
        // Every other job naming a gone tab is dropped; a frame is no
        // different, and the ack has nowhere to go because Core drops an
        // effect naming a page it does not hold.
        let mut session = ready_with_graphics();
        open_two_more(&mut session);
        session.on(key('p'));
        let doomed = session.tabs[1].id;
        session.on(key('x'));

        let effects = session.on(Event::Frame(doomed, Box::new(frame_data("GONE"))));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::AckFrame(id, _) if *id == doomed)),
            "nothing is asked of a tab that is gone"
        );
    }
```

The second test requires the frame handler in Task 8 to check that the tab still exists
before pushing the ack. If Task 8's version pushes the ack unconditionally, tighten it:

```rust
            Event::Frame(id, frame) => {
                // A tab that is gone is asked for nothing, like every other
                // job naming one. A tab that is merely not focused is still
                // acked, or its screencast stops mid-switch.
                if !self.tabs.iter().any(|tab| tab.id == id) {
                    return effects;
                }
                effects.push(Effect::AckFrame(id, frame.ack));
                ...
```

- [x] **Step 5: Run everything**

Run: `cargo test --workspace`
Expected: PASS.

- [x] **Step 6: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt/src/core.rs
git commit -m "feat(wwt): carry pictures through the loop

Three spawns and one more question of a CDP event, asked before the
dirty signal because a frame is much the more frequent of the two.

No coalescing, and the spec is amended to say why rather than leaving
the machinery looking forgotten: Chromium sends the next frame only
once the last is acked, so at most one is ever in flight and there is
nothing to drop. The ack is the backpressure. If a page ever does
outrun the loop the knob is everyNthFrame, because a frame we have
already been sent has cost us the frame either way."
```

---

### Task 10: Resize, switching, and putting it away

Three paths that already exist and now have a screencast in them. None of them is new machinery; each is an existing path learning one more thing.

**Files:**
- Modify: `crates/wwt/src/session.rs`
- Modify: `crates/wwt/src/core.rs`

- [x] **Step 1: Write the failing tests**

In `crates/wwt/src/session.rs`, in `mod tests`:

```rust
    #[test]
    fn switching_tabs_moves_the_screencast_with_the_focus() {
        let mut session = ready_with_graphics();
        open_two_more(&mut session);
        session.on(key('p'));
        let leaving = session.focused().id;

        let effects = session.on(key('J'));
        let arriving = session.focused().id;
        assert_ne!(leaving, arriving);
        assert!(effects.iter().any(|e| matches!(e, Effect::StopScreencast(id) if *id == leaving)));
        assert!(effects.iter().any(|e| matches!(e, Effect::StartScreencast(id) if *id == arriving)));
    }

    #[test]
    fn the_previous_picture_stays_up_until_the_new_tabs_first_frame() {
        // Never blank the frame you are looking at. A switch in pixel mode
        // is a round trip, and until it lands the old picture with the new
        // chrome around it is better than nothing at all.
        let mut session = ready_with_graphics();
        open_two_more(&mut session);
        session.on(key('p'));
        let id = session.focused().id;
        session.on(Event::Frame(id, Box::new(frame_data("OLD"))));

        session.on(key('J'));
        let frame = session.compose();
        assert_eq!(
            frame.image().map(|i| i.payload.as_str()),
            Some("OLD"),
            "the picture stands until a new one arrives"
        );
    }

    #[test]
    fn a_resize_restarts_the_screencast_at_the_new_size() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let effects = session.on(Event::Resized(
            GridSize { cols: 100, rows: 30 },
            CellSize { w: 9, h: 20 },
        ));
        assert!(effects.iter().any(|e| matches!(e, Effect::StopScreencast(_))));
        assert!(effects.iter().any(|e| matches!(e, Effect::StartScreencast(_))));
    }

    #[test]
    fn a_resize_composes_the_image_at_the_new_area() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let id = session.focused().id;
        session.on(Event::Frame(id, Box::new(frame_data("AAAA"))));
        session.on(Event::Resized(
            GridSize { cols: 100, rows: 30 },
            CellSize { w: 9, h: 20 },
        ));

        // Whatever picture is still up covers the page area it has now, or
        // the placeholders address a placement of the wrong shape.
        if let Some(image) = session.compose().image() {
            assert_eq!(image.area.cols, 100);
            assert_eq!(image.area.rows, 30 - CHROME_ROWS);
        }
    }

    #[test]
    fn quitting_from_pixel_mode_stops_the_screencast() {
        let mut session = ready_with_graphics();
        session.on(key('p'));
        let effects = session.on(key('q'));
        assert!(effects.iter().any(|e| matches!(e, Effect::StopScreencast(_))));
        assert!(effects.iter().any(|e| matches!(e, Effect::Quit)));
    }

    #[test]
    fn closing_the_focused_tab_moves_the_screencast_to_the_next_one() {
        let mut session = ready_with_graphics();
        open_two_more(&mut session);
        session.on(key('p'));
        let effects = session.on(key('x'));
        assert!(effects.iter().any(|e| matches!(e, Effect::StartScreencast(_))));
    }
```

- [x] **Step 2: Run to verify they fail**

Run: `cargo test -p wwt --lib session::`
Expected: FAIL on each of the six.

- [x] **Step 3: Implement**

Every one of these is the same one-line question in an existing path: *if pixel mode is on, the screencast follows the focus*. Add a helper and call it from the four places focus changes or the geometry does:

```rust
    /// Move the screencast to whatever tab is focused now.
    ///
    /// Called wherever focus changes or the viewport does. The picture is
    /// deliberately not cleared: a switch in pixel mode is a round trip,
    /// and the old picture with the new chrome around it is what "never
    /// blank the frame you are looking at" means here.
    fn follow_focus(&mut self, leaving: Option<TabId>, effects: &mut Vec<Effect>) {
        if !self.pixel {
            return;
        }
        if let Some(leaving) = leaving {
            effects.push(Effect::StopScreencast(leaving));
        }
        effects.push(Effect::StartScreencast(self.focused().id));
    }
```

Call it from the tab-switch path, the tab-close path, and the resize path (which passes `Some(current)` because the same tab's screencast has to be restarted at the new size). On quit, push `Effect::StopScreencast` before `Effect::Quit`.

In the resize path, the stored picture's area is recomputed against the new viewport rather than dropped, so the frame composed between the resize and the next screencast frame addresses a placement of the right shape:

```rust
        if let Some(image) = &mut self.picture {
            image.area = CellRect::of(self.vp.grid(), self.vp.origin_row());
            // A new generation, because the renderer diffs on it and this
            // image has to be placed again at its new size.
            self.generations += 1;
            image.generation = self.generations;
        }
```

- [x] **Step 4: Run to verify they pass, then the whole workspace**

Run: `cargo test -p wwt --lib session:: && cargo test --workspace`
Expected: PASS.

- [x] **Step 5: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt/src/session.rs crates/wwt/src/core.rs
git commit -m "feat(wwt): let the picture follow the focus and the size

Four existing paths learn one question each: with pixel mode on, the
screencast is wherever the focus is. Switching, closing, resizing and
quitting are the four, and one helper answers for all of them.

The picture is not cleared on a switch. A switch in pixel mode is a
round trip, so clearing it would blank the frame you are looking at for
as long as the new tab takes to paint, and the old picture under the new
tab's chrome is the honest version of waiting."
```

---

### Task 11: The measurement, the notes and the manual pass

M5's claim is that a pixel frame is one escape sequence and not a repaint, and that an idle page in pixel mode still costs nothing. Both belong in a test rather than in anybody's head, like extraction's and the switch's before them. The spec also owes a measurement it named in section 3.

**Files:**
- Modify: `crates/wwt-term/src/render.rs` (the measurement)
- Modify: `CLAUDE.md`
- Modify: `CONTEXT.md`
- Modify: `README.md`
- Modify: `docs/superpowers/specs/2026-08-22-wwt-m5-design.md` (closing open questions 1 and 3)

- [x] **Step 1: Write the measurement**

In `crates/wwt-term/src/render.rs`, in `mod tests`. It needs no terminal, which is the point: the claim is about what is written, not about what a terminal does with it.

```rust
    /// What a pixel frame costs to write. Run with:
    ///
    ///     cargo test -p wwt-term --lib measure_pixel_frame -- --nocapture
    ///
    /// The claim in section 4 of the M5 spec is that a new frame is a
    /// transmission and no cells, so this asserts the absence of a
    /// placeholder as well as printing the time. A realistic payload: a
    /// 1080p-ish PNG of a text page is a few hundred kilobytes, which is
    /// about 400KB of base64.
    #[test]
    fn measure_pixel_frame() {
        let mut renderer = Renderer::new();
        let payload = "A".repeat(400 * 1024);
        let area = CellRect { col: 0, row: 1, cols: 120, rows: 38 };
        let mut out = Vec::with_capacity(payload.len() * 2);

        let mut first = Frame::new(GridSize { cols: 120, rows: 40 });
        first.set_image(Some(Image { generation: 1, payload: payload.clone(), area }));
        renderer.render(&first, &mut out).expect("the first frame");

        let mut worst = std::time::Duration::ZERO;
        for generation in 2..102 {
            let mut frame = Frame::new(GridSize { cols: 120, rows: 40 });
            frame.set_image(Some(Image {
                generation,
                payload: payload.clone(),
                area,
            }));
            out.clear();

            let start = std::time::Instant::now();
            renderer.render(&frame, &mut out).expect("a later frame");
            worst = worst.max(start.elapsed());

            assert!(
                !String::from_utf8_lossy(&out).contains(protocol::PLACEHOLDER),
                "a frame must not rewrite a placeholder cell"
            );
        }
        eprintln!("pixel frame, worst of 100: {worst:?}");
        assert!(worst < std::time::Duration::from_millis(10), "frame took {worst:?}");
    }
```

Run it and record the number in the commit message.

- [x] **Step 2: Answer the measurement section 3 of the spec owes**

Measure pixel-mode CPU on an animated page with `--disable-frame-rate-limit` and without it:

```bash
cargo run -p wwt -- 'https://example.com' &
# In a real terminal: press p, open something that animates continuously,
# and watch the Chromium and wwt processes:
top -p "$(pgrep -d, -f 'chromium|wwt')"
```

Then remove `--disable-frame-rate-limit` from the launch flags in `crates/wwt-cdp/src/launch.rs`, rebuild, and repeat. Record both numbers. If the flag costs materially more CPU in pixel mode than it saves in scroll latency, the answer is `everyNthFrame` in `start_screencast` rather than dropping the flag, because the flag is what M2's scroll latency rests on. Write the numbers and the decision into the spec's open question 3, and mark it closed.

- [x] **Step 3: Close open question 1**

Task 1's probe answered whether re-transmission updates a placement in place. Write the answer into the spec's open question 1 and mark it closed, along with any amendment to section 4 that the answer forced.

- [x] **Step 4: Update the glossary**

In `CONTEXT.md`, add to "What the browser is doing":

```markdown
**Pixel mode** — the page shown as a picture of itself rather than
reconstructed from its runs. Global rather than per-tab, and only the
focused tab screencasts. The viewport, the scroll offset and the focus are
the same in both modes, which is what makes the toggle instant.

**Frame (screencast)** — one picture of a page, base64 PNG, exactly as CDP
sent it and exactly as the graphics protocol wants it. Not to be confused
with a `Frame`, which is the grid of cells; this one is `ScreencastFrame`
and is a field on one.

**Ack id** — the integer a screencast frame must be answered with. CDP calls
it `sessionId` and it is not a CDP session id: it counts screencasts on one
target. Chromium sends the next frame only once the last is acked.

**Placeholder** — a cell carrying U+10EEEE, which shows part of an image
rather than a glyph. The image id rides in its foreground colour and its
position in combining diacritics. A cell holding a real glyph instead shows
the glyph, which is how a hint label lands on top of a picture.
```

- [x] **Step 5: Update the working notes**

In `CLAUDE.md`, change the milestone line to M5 and add a section after "Tabs and sessions":

```markdown
## Pixel mode

**The bytes never leave base64.** `Page.screencastFrame` carries base64 PNG
and the Kitty graphics protocol wants base64 PNG, so a frame is forwarded
from the websocket to stdout as the string it arrived as. This is why M5
adds no dependency, and it is why half-block degradation is M6's: half a
cell needs real samples, which needs an inflate and an unfilter that
nothing else here would ever use.

**The grid wins over the image.** Unicode placeholders make placement cell
content, so a cell holding a glyph shows the glyph and a cell holding a
placeholder shows the picture. Hint labels over a pixel page therefore cost
nothing to arrange, and `compose` paints labels after either branch.

**`Frame` carries the image; `Cell` does not.** A cell that had to hold
combining diacritics would put a terminal protocol inside the one crate
whose hard rule is that it knows about nothing. The renderer synthesizes
placeholder cells as it writes and owns every byte of the protocol.

**A new frame rewrites no cells.** The image id is fixed, so a frame is one
transmission and the placeholders already on screen re-render against it.
Placeholders are written on entering pixel mode, on a resize, and on a tab
switch, and never on a scroll. `measure_pixel_frame` holds that down.

**Every frame is acked, including the ones dropped.** Chromium counts acks
and not paints, so a coalesced burst still owes one ack per frame. Skipping
one stops the screencast and the picture freezes with no error to say why.

**A switch in pixel mode is a round trip.** M4's repaint guarantee, and
`measure_switch`, are text mode's. The previous picture stays up under the
new tab's chrome until the first frame arrives, because the alternative is
blanking the frame you are looking at.

**Pixel mode is global and is not saved.** Only the focused tab screencasts
either way, so per-tab would buy a preference rather than a cost, and a new
field in `Snapshot` is a version bump that costs every existing session file
its tabs on upgrade.
```

Also add to the Commands block:

```
    cargo test -p wwt-term --lib measure_pixel_frame -- --nocapture     # pixel frame cost
```

- [x] **Step 6: Update the README**

Change the status line to M5, add the key and the command, and say what happens without graphics:

```markdown
| `p` | show the page as it really looks |
```

```markdown
`p` swaps the page between text and true pixels without moving it: the same
viewport, the same scroll offset, the same tab. It needs a terminal that
speaks the Kitty graphics protocol, which wwt asks about once at startup;
without one, `p` says so and leaves the text where it was. `:set pixel on`
and `:set pixel off` do the same thing from the command line.
```

- [ ] **Step 7: The manual pass**

Not automatable, and the milestone is not done without it. Work through it on a real terminal and fix what it finds:

1. `wwt example.com`, then `p`: the page as a picture, in the same place, at the same scroll offset.
2. `p` again: back to text, same place, and nothing left on screen from the picture.
3. `j` and `k` in pixel mode: the picture scrolls and keeps up.
4. `f` in pixel mode: labels on top of the picture, readable, and one of them clicks through.
5. `i` in pixel mode on a text field: the page's own caret is the one you see, and typing lands.
6. `J` and `K` around three tabs in pixel mode: each shows its own page, and the previous picture is what stands while the next arrives rather than a blank.
7. Resize the terminal in pixel mode: the picture is laid out for the terminal you have now, with no torn placement.
8. `x` the focused tab in pixel mode: the picture follows to the next tab.
9. A page playing video, in pixel mode, for a minute: it keeps moving and does not freeze, which is the ack path.
10. An idle page in pixel mode: `top` shows wwt and Chromium at rest.
11. `q` from pixel mode: the terminal is left clean, with no image and no escape residue.
12. Run it in a terminal with no graphics support (`TERM=xterm` under something plain): `p` says so, and the text page is undisturbed.

- [x] **Step 8: Run everything one last time**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p wwt-term --lib measure_pixel_frame -- --nocapture
cargo test -p wwt --lib measure_switch -- --nocapture
cargo test -p wwt-page --test extraction measure_extraction -- --nocapture
cargo test -p wwt-page --test interaction measure_scroll_latency -- --nocapture
```

The last three are M2's, M3's and M4's numbers. M5 must not have moved them; if any has, find out why before calling the milestone done. Extraction runs in pixel mode exactly as it does in text mode, so `measure_extraction` in particular should be untouched.

- [x] **Step 9: Commit**

```bash
git add CONTEXT.md CLAUDE.md README.md crates/wwt-term/src/render.rs docs/superpowers/specs/2026-08-22-wwt-m5-design.md
git commit -m "docs: write down what a picture costs and what it is called

The measurement is in the tests rather than in anybody's head, like
extraction's, the scroll's and the switch's before it: a frame is a
transmission and no cells, and measure_pixel_frame asserts the absence of
the placeholder as well as printing the time.

Closes the spec's open questions 1 and 3 with the answers the probe and
the CPU measurement gave."
```

---

## Done when

- `cargo test --workspace` passes and `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- The twelve manual checks in Task 11 all pass on a real terminal with Kitty graphics, and check 12 passes on one without.
- `measure_pixel_frame` prints a number and asserts no placeholder is rewritten; M2's, M3's and M4's measurements are unmoved.
- The spec's open questions 1 and 3 are closed with real answers, and any amendment the answers forced is in the spec.
- No new entry in `Cargo.toml`'s workspace dependencies. If one seemed necessary, the milestone took a wrong turn: the whole design exists to avoid decoding anything.
