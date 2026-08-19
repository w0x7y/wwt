# Webinal M2 — Navigation and Reading Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn M1's render-once-and-exit binary into a live, read-only browser: the page stays open, scrolls natively, re-renders only what changed when it changes, and navigates somewhere else without restarting.

**Architecture:** One core task owns all state, driven by a single `select!` over terminal events, CDP events, and a debounce timer. Page operations run as spawned tasks reporting back on one channel, so a thirty-second page load never blocks a keypress. The injected script is installed once per document and signals dirtiness through a CDP binding, so an idle page costs no CPU.

**Tech Stack:** Rust 2024, tokio, tokio-tungstenite, crossterm (with `event-stream`), futures-util, serde/serde_json, anyhow. Chromium as an external process.

**Spec:** `docs/superpowers/specs/2026-08-19-webinal-m2-design.md` — read it in full before starting. Its parent, `docs/superpowers/specs/2026-08-19-webinal-design.md`, governs where the two disagree; sections 5, 6, and 8 of the parent are the relevant ones.

## Global Constraints

- Rust edition **2024**, toolchain **1.97+**.
- Dependency versions, exact, unchanged from M1: `tokio = "1.53"`, `tokio-tungstenite = "0.30"`, `futures-util = "0.3"`, `serde = "1.0"` (feature `derive`), `serde_json = "1.0"`, `crossterm = "0.29"`, `rustix = "1.1"` (feature `termios`), `anyhow = "1.0"`, `thiserror = "2.0"`, `tempfile = "3"` (dev-dependency).
- **The only dependency change M2 makes** is adding the `event-stream` feature to the workspace's `crossterm` entry, and adding existing workspace deps (`futures-util`, `serde`) to crates that did not previously use them. No new crates. If a task tempts you to add one, stop and ask.
- `wb-frame` has **no I/O and no dependencies**. This is unchanged and non-negotiable.
- Chromium is located via `WEBINAL_CHROMIUM`, falling back to the first of `chromium`, `chromium-browser`, `google-chrome-stable` on `PATH`. Never download anything.
- `cargo clippy --workspace --all-targets -- -D warnings` must be clean at the end of **every** task, not only at the end of the plan.
- Tests that need a browser live in `tests/`, never in `src/`. Unit tests in `src/` must run without Chromium.
- Follow the existing comment style: explain *why*, in prose, where the reason is not obvious from the code. Do not add comments that restate the code.

## Baseline

Before Task 1, confirm the starting state:

```bash
cargo test --workspace
```

Expected: 46 tests pass — 17 `wb-frame`, 12 `wb-term`, 3 `wb-cdp`, 12 `wb-page`, 2 `webinal`.

---

### Task 1: `Style::reverse` and `Frame::paint_text`

The chrome row needs to paint text at an exact cell and needs reverse video. Both belong in `wb-frame`, which stays dependency-free. Adding a field to `Style` breaks every struct literal in the workspace, so this task fixes them all.

**Files:**
- Modify: `crates/wb-frame/src/cell.rs:10-22` (add `reverse`, update `Default`)
- Modify: `crates/wb-frame/src/frame.rs` (add `paint_text`, add tests)
- Modify: `crates/wb-frame/src/frame.rs:273` (existing `Style` literal)
- Modify: `crates/wb-term/src/render.rs:104,111,119` (existing `Style` literals in tests)
- Modify: `crates/wb-page/src/extract.rs:175-178` (existing `Style` literal)

**Interfaces:**
- Consumes: nothing.
- Produces: `Style { fg: Rgb, bold: bool, reverse: bool }`; `Frame::paint_text(&mut self, pos: CellPos, text: &str, style: Style)`.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block at the bottom of `crates/wb-frame/src/frame.rs`:

```rust
    #[test]
    fn paint_text_writes_at_the_given_cell() {
        let mut f = Frame::new(GridSize { cols: 10, rows: 2 });
        f.paint_text(CellPos { col: 2, row: 1 }, "hi", Style::default());
        assert_eq!(f.row_text(1), "  hi");
        assert_eq!(f.row_text(0), "");
    }

    #[test]
    fn paint_text_clips_at_the_right_edge() {
        let mut f = Frame::new(GridSize { cols: 4, rows: 1 });
        f.paint_text(CellPos { col: 2, row: 0 }, "abcd", Style::default());
        assert_eq!(f.row_text(0), "  ab");
    }

    #[test]
    fn paint_text_off_the_bottom_is_a_no_op() {
        let mut f = Frame::new(GridSize { cols: 4, rows: 1 });
        f.paint_text(CellPos { col: 0, row: 5 }, "abcd", Style::default());
        assert_eq!(f.row_text(0), "");
    }

    #[test]
    fn paint_text_carries_its_style() {
        let mut f = Frame::new(GridSize { cols: 4, rows: 1 });
        let style = Style { fg: Rgb { r: 1, g: 2, b: 3 }, bold: false, reverse: true };
        f.paint_text(CellPos { col: 0, row: 0 }, "x", style);
        assert_eq!(f.cell(CellPos { col: 0, row: 0 }).unwrap().style, style);
    }

    #[test]
    fn paint_text_outranks_any_page_run() {
        // Chrome is painted after the page and must never lose a cell to it,
        // whatever stacking depth the page claimed.
        let mut f = Frame::new(GridSize { cols: 4, rows: 1 });
        f.paint_text(CellPos { col: 0, row: 0 }, "ab", Style::default());
        f.paint_run(
            &Viewport::new(GridSize { cols: 4, rows: 1 }, CellSize { w: 10, h: 20 }),
            &TextRun {
                text: "zz".to_string(),
                rect: CssRect { x: 0.0, y: 0.0, w: 40.0, h: 16.0 },
                baseline: 14.0,
                style: Style::default(),
                z: i32::MAX,
            },
        );
        assert_eq!(f.row_text(0), "ab");
    }
```

Check the `use super::*;` line at the top of that `mod tests` block imports `CellSize`, `CssRect`, and `TextRun`. If not, add them.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p wb-frame
```

Expected: FAIL — `no method named paint_text found`, and `Style` has no field `reverse`.

- [ ] **Step 3: Add `reverse` to `Style`**

In `crates/wb-frame/src/cell.rs`, replace the `Style` struct and its `Default`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub fg: Rgb,
    pub bold: bool,
    /// Swap foreground and background. Chrome uses this; extraction never
    /// produces it, which is why there is no background color here yet.
    pub reverse: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fg: Rgb { r: 0xd0, g: 0xd0, b: 0xd0 },
            bold: false,
            reverse: false,
        }
    }
}
```

- [ ] **Step 4: Implement `paint_text`**

In `crates/wb-frame/src/frame.rs`, add to `impl Frame`, directly after `paint_run`:

```rust
    /// Paint a string starting at one cell, clipped at the right edge.
    ///
    /// Chrome uses this. It paints at the maximum stacking depth so that
    /// nothing the page produces can take a cell back from it.
    pub fn paint_text(&mut self, pos: CellPos, text: &str, style: Style) {
        for (i, ch) in text.chars().enumerate() {
            let Ok(offset) = u16::try_from(i) else { break };
            let Some(col) = pos.col.checked_add(offset) else { break };
            let Some(idx) = self.index(CellPos { col, row: pos.row }) else {
                break;
            };
            self.cells[idx] = Cell { ch, style, z: i32::MAX };
        }
    }
```

Add `Style` to the `use crate::cell::...` line at the top of the file.

- [ ] **Step 5: Fix the four broken `Style` literals**

`crates/wb-frame/src/frame.rs:273`:

```rust
        r.style = Style { fg: Rgb { r: 255, g: 0, b: 0 }, bold: true, reverse: false };
```

`crates/wb-term/src/render.rs`, lines 104, 111, 119 respectively:

```rust
        let style = Style { fg: Rgb { r: 255, g: 128, b: 0 }, bold: false, reverse: false };
```
```rust
        let style = Style { fg: Rgb { r: 0, g: 0, b: 0 }, bold: true, reverse: false };
```
```rust
        let style = Style { fg: Rgb { r: 10, g: 20, b: 30 }, bold: false, reverse: false };
```

`crates/wb-page/src/extract.rs:175-178`:

```rust
                style: Style {
                    fg: parse_css_color(&r.color),
                    bold: r.bold,
                    reverse: false,
                },
```

- [ ] **Step 6: Run the whole workspace**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 51 tests pass (46 + 5 new). Clippy clean.

- [ ] **Step 7: Commit**

```bash
git add crates/wb-frame crates/wb-term crates/wb-page
git commit -m "feat(frame): add reverse styling and paint_text for chrome"
```

---

### Task 2: The diffing renderer

`render` repaints all 8,640 cells every time. Replace it with a `Renderer` that holds the last presented frame and emits only changed cells. The free `render` function stays as the full-repaint path.

**Files:**
- Modify: `crates/wb-term/src/render.rs` (add `Renderer`, teach `write_style` about `reverse`)
- Modify: `crates/wb-term/src/lib.rs` (export `Renderer`)

**Interfaces:**
- Consumes: `Style.reverse` from Task 1.
- Produces: `wb_term::Renderer` with `Renderer::new()`, `Renderer::render(&mut self, frame: &Frame, out: &mut impl Write) -> std::io::Result<()>`, `Renderer::invalidate(&mut self)`. The free `wb_term::render` is unchanged in signature.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/wb-term/src/render.rs`:

```rust
    fn diff_to_string(r: &mut Renderer, f: &Frame) -> String {
        let mut buf = Vec::new();
        r.render(f, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn renderer_paints_the_first_frame_in_full() {
        let mut r = Renderer::new();
        let out = diff_to_string(&mut r, &painted("hi", Style::default()));
        assert!(out.starts_with("\x1b[H"), "output was {out:?}");
        assert!(out.contains("hi"), "output was {out:?}");
    }

    #[test]
    fn renderer_emits_nothing_for_an_unchanged_frame() {
        let mut r = Renderer::new();
        let f = painted("hi", Style::default());
        diff_to_string(&mut r, &f);
        assert_eq!(diff_to_string(&mut r, &f), "");
    }

    #[test]
    fn renderer_emits_only_the_changed_cell() {
        let mut r = Renderer::new();
        diff_to_string(&mut r, &painted("hi", Style::default()));
        let out = diff_to_string(&mut r, &painted("ho", Style::default()));

        // Row 0, column 2 in 1-based terminal coordinates.
        assert!(out.contains("\x1b[1;2H"), "output was {out:?}");
        assert!(out.contains('o'), "output was {out:?}");
        assert!(!out.contains('h'), "the unchanged cell was repainted: {out:?}");
    }

    #[test]
    fn renderer_repaints_in_full_when_the_grid_changes() {
        let mut r = Renderer::new();
        diff_to_string(&mut r, &Frame::new(GridSize { cols: 10, rows: 2 }));
        let out = diff_to_string(&mut r, &Frame::new(GridSize { cols: 12, rows: 3 }));
        assert!(out.starts_with("\x1b[H"), "output was {out:?}");
    }

    #[test]
    fn invalidate_forces_the_next_frame_to_repaint_in_full() {
        let mut r = Renderer::new();
        let f = painted("hi", Style::default());
        diff_to_string(&mut r, &f);
        r.invalidate();
        assert!(diff_to_string(&mut r, &f).starts_with("\x1b[H"));
    }

    #[test]
    fn render_sets_reverse_video() {
        let style = Style { fg: Rgb { r: 0, g: 0, b: 0 }, bold: false, reverse: true };
        let out = render_to_string(&painted("hi", style));
        assert!(out.contains("\x1b[7m"), "output was {out:?}");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p wb-term
```

Expected: FAIL — `cannot find type Renderer in this scope`.

- [ ] **Step 3: Teach `write_style` about `reverse`**

In `crates/wb-term/src/render.rs`, replace `write_style`:

```rust
fn write_style(out: &mut impl Write, style: &Style) -> std::io::Result<()> {
    // Reset first so that clearing bold does not need a separate sequence.
    write!(out, "\x1b[0m")?;
    if style.bold {
        write!(out, "\x1b[1m")?;
    }
    if style.reverse {
        write!(out, "\x1b[7m")?;
    }
    write!(
        out,
        "\x1b[38;2;{};{};{}m",
        style.fg.r, style.fg.g, style.fg.b
    )
}
```

- [ ] **Step 4: Implement `Renderer`**

Add to `crates/wb-term/src/render.rs`, above the `mod tests` block:

```rust
/// A renderer that remembers what it last put on screen.
///
/// A page where one counter ticks costs a handful of bytes per update
/// rather than a full repaint, which is the difference between a browser
/// that is pleasant on a slow link and one that is not.
#[derive(Debug, Default)]
pub struct Renderer {
    last: Option<Frame>,
}

impl Renderer {
    pub fn new() -> Self {
        Self { last: None }
    }

    /// Discard the cached frame, so the next render repaints everything.
    /// Used after a resize, and after anything else writes to the terminal
    /// behind our back.
    pub fn invalidate(&mut self) {
        self.last = None;
    }

    pub fn render(&mut self, frame: &Frame, out: &mut impl Write) -> std::io::Result<()> {
        let reusable = self
            .last
            .as_ref()
            .is_some_and(|prev| prev.grid() == frame.grid());

        if reusable {
            self.diff(frame, out)?;
        } else {
            // A diff against a frame of different dimensions is meaningless.
            render(frame, out)?;
        }

        self.last = Some(frame.clone());
        Ok(())
    }

    fn diff(&self, frame: &Frame, out: &mut impl Write) -> std::io::Result<()> {
        let prev = self.last.as_ref().expect("diff runs only with a cached frame");
        let grid = frame.grid();
        let mut wrote = false;

        for row in 0..grid.rows {
            let mut col = 0;
            while col < grid.cols {
                let pos = CellPos { col, row };
                if frame.cell(pos) == prev.cell(pos) {
                    col += 1;
                    continue;
                }

                // Address the start of this changed segment. Terminal
                // coordinates are 1-based.
                write!(out, "\x1b[{};{}H", row + 1, col + 1)?;
                wrote = true;

                let mut active: Option<Style> = None;
                while col < grid.cols {
                    let pos = CellPos { col, row };
                    let cell = frame.cell(pos).expect("cell within the frame's own grid");
                    if Some(cell) == prev.cell(pos) {
                        break;
                    }
                    if active != Some(cell.style) {
                        write_style(out, &cell.style)?;
                        active = Some(cell.style);
                    }
                    let mut buf = [0u8; 4];
                    out.write_all(cell.ch.encode_utf8(&mut buf).as_bytes())?;
                    col += 1;
                }
                write!(out, "\x1b[0m")?;
            }
        }

        if wrote {
            out.flush()?;
        }
        Ok(())
    }
}
```

- [ ] **Step 5: Export it**

`crates/wb-term/src/lib.rs`:

```rust
pub use render::{Renderer, render};
```

- [ ] **Step 6: Run the tests**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 57 tests pass (51 + 6 new). Clippy clean.

- [ ] **Step 7: Commit**

```bash
git add crates/wb-term
git commit -m "feat(term): emit only changed cells between frames"
```

---

### Task 3: The CDP event pump

`read_loop` drops every message without an `id`. Those are protocol events, and everything event-driven in M2 depends on them.

**Files:**
- Modify: `crates/wb-cdp/src/client.rs` (add `Event`, `subscribe`, broadcast in `read_loop`, tests)
- Modify: `crates/wb-cdp/src/lib.rs` (export `Event`)

**Interfaces:**
- Consumes: nothing.
- Produces: `wb_cdp::Event { session_id: Option<String>, method: String, params: serde_json::Value }` deriving `Debug, Clone`; `Client::subscribe(&self) -> tokio::sync::mpsc::UnboundedReceiver<Event>` (synchronous, not `async`).

- [ ] **Step 1: Write the failing tests**

Add a `mod tests` block at the bottom of `crates/wb-cdp/src/client.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use tokio_tungstenite::tungstenite::{Error as WsError, Message};

    fn parts() -> (Pending, Subscribers) {
        (
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(StdMutex::new(Vec::new())),
        )
    }

    fn one(text: &str) -> impl futures_util::Stream<Item = Result<Message, WsError>> + Unpin {
        stream::iter(vec![Ok(Message::text(text.to_string()))])
    }

    #[tokio::test]
    async fn events_reach_subscribers() {
        let (pending, subs) = parts();
        let (tx, mut rx) = mpsc::unbounded_channel();
        subs.lock().unwrap().push(tx);

        read_loop(
            one(r#"{"method":"Page.loadEventFired","sessionId":"S1","params":{"timestamp":1}}"#),
            pending,
            subs,
        )
        .await;

        let event = rx.recv().await.expect("an event");
        assert_eq!(event.method, "Page.loadEventFired");
        assert_eq!(event.session_id.as_deref(), Some("S1"));
        assert_eq!(event.params["timestamp"], 1);
    }

    #[tokio::test]
    async fn an_event_without_a_session_is_still_delivered() {
        let (pending, subs) = parts();
        let (tx, mut rx) = mpsc::unbounded_channel();
        subs.lock().unwrap().push(tx);

        read_loop(one(r#"{"method":"Target.targetCreated","params":{}}"#), pending, subs).await;

        let event = rx.recv().await.expect("an event");
        assert_eq!(event.method, "Target.targetCreated");
        assert!(event.session_id.is_none());
    }

    #[tokio::test]
    async fn responses_still_correlate_while_events_flow() {
        let (pending, subs) = parts();
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(7, tx);

        let messages = vec![
            Ok::<_, WsError>(Message::text(r#"{"method":"Page.loadEventFired","params":{}}"#.to_string())),
            Ok(Message::text(r#"{"id":7,"result":{"ok":true}}"#.to_string())),
        ];
        read_loop(stream::iter(messages), pending, subs).await;

        let response = rx.await.expect("the response");
        assert_eq!(response["result"]["ok"], true);
    }

    #[tokio::test]
    async fn a_dropped_subscriber_is_forgotten() {
        let (pending, subs) = parts();
        let (tx, rx) = mpsc::unbounded_channel::<Event>();
        subs.lock().unwrap().push(tx);
        drop(rx);

        read_loop(one(r#"{"method":"Page.loadEventFired","params":{}}"#), pending, Arc::clone(&subs)).await;

        assert!(subs.lock().unwrap().is_empty(), "the dead sender should be pruned");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p wb-cdp --lib
```

Expected: FAIL — `cannot find type Subscribers in this scope`.

- [ ] **Step 3: Add the event type and the subscriber list**

In `crates/wb-cdp/src/client.rs`, add to the imports:

```rust
use std::sync::Mutex as StdMutex;
```

Add above `type Pending`:

```rust
/// A CDP protocol event: any message the browser sends that is not a
/// response to one of our commands.
#[derive(Debug, Clone)]
pub struct Event {
    /// `None` for browser-level events, `Some` for events from an attached
    /// page session.
    pub session_id: Option<String>,
    pub method: String,
    pub params: Value,
}

/// A plain `std` mutex, not tokio's: nothing awaits while it is held, and
/// `subscribe` is far more useful synchronous than async.
type Subscribers = Arc<StdMutex<Vec<mpsc::UnboundedSender<Event>>>>;
```

- [ ] **Step 4: Wire it into `Client`**

Add the field to the struct:

```rust
pub struct Client {
    next_id: AtomicU64,
    outgoing: mpsc::UnboundedSender<String>,
    pending: Pending,
    subscribers: Subscribers,
}
```

In `connect`, replace the `read_loop` spawn and the `Ok(Self { ... })` tail:

```rust
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let subscribers: Subscribers = Arc::new(StdMutex::new(Vec::new()));
        tokio::spawn(read_loop(
            stream,
            Arc::clone(&pending),
            Arc::clone(&subscribers),
        ));

        Ok(Self {
            next_id: AtomicU64::new(1),
            outgoing: tx,
            pending,
            subscribers,
        })
```

Add the method to `impl Client`:

```rust
    /// Receive every protocol event from now on.
    ///
    /// Subscribe *before* issuing the command whose event you intend to
    /// wait for, or you can miss it.
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<Event> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.subscribers
            .lock()
            .expect("the subscriber list is never held across a panic")
            .push(tx);
        rx
    }
```

- [ ] **Step 5: Broadcast in `read_loop`**

Replace `read_loop` in `crates/wb-cdp/src/client.rs`:

```rust
async fn read_loop<S>(mut stream: S, pending: Pending, subscribers: Subscribers)
where
    S: futures_util::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    while let Some(Ok(msg)) = stream.next().await {
        let Ok(text) = msg.into_text() else { continue };
        let Ok(value): Result<Value, _> = serde_json::from_str(&text) else {
            continue;
        };

        // Messages with an `id` are responses to our commands; everything
        // else is an event.
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            if let Some(tx) = pending.lock().await.remove(&id) {
                let _ = tx.send(value);
            }
            continue;
        }

        let Some(method) = value.get("method").and_then(Value::as_str) else {
            continue;
        };
        let event = Event {
            session_id: value
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_string),
            method: method.to_string(),
            params: value.get("params").cloned().unwrap_or_else(|| json!({})),
        };

        // A subscriber whose receiver is gone is pruned rather than left to
        // accumulate for the life of the connection.
        subscribers
            .lock()
            .expect("the subscriber list is never held across a panic")
            .retain(|tx| tx.send(event.clone()).is_ok());
    }

    // The socket is gone; wake every caller rather than letting them wait out
    // their deadlines.
    pending.lock().await.clear();
    subscribers
        .lock()
        .expect("the subscriber list is never held across a panic")
        .clear();
}
```

- [ ] **Step 6: Export the type**

`crates/wb-cdp/src/lib.rs`:

```rust
pub use client::{Client, Event};
```

- [ ] **Step 7: Run the tests**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 61 tests pass (57 + 4 new). Clippy clean.

- [ ] **Step 8: Commit**

```bash
git add crates/wb-cdp
git commit -m "feat(cdp): deliver protocol events to subscribers"
```

---

### Task 4: `Page` owns its client

`Page<'a>` borrows its `Client`, so a core loop owning both would be self-referential. `Arc` is required for M4's several pages over one connection regardless.

**Files:**
- Modify: `crates/wb-page/src/extract.rs:35-46` (struct and `open`)
- Modify: `crates/wb-page/tests/extraction.rs:18-36` (harness)
- Modify: `crates/webinal/src/lib.rs:15-17`

**Interfaces:**
- Consumes: `wb_cdp::Client` from Task 3.
- Produces: `wb_page::Page` (no lifetime parameter); `Page::open(client: Arc<Client>, url: &str, vp: Viewport) -> Result<Page>`; `Page::session_id(&self) -> &str`.

- [ ] **Step 1: Change the struct**

In `crates/wb-page/src/extract.rs`, add `use std::sync::Arc;` to the imports, then replace lines 35-46:

```rust
pub struct Page {
    client: Arc<Client>,
    session_id: String,
}

impl Page {
    /// Create a target, size it to the viewport, navigate, and wait for load.
    pub async fn open(client: Arc<Client>, url: &str, vp: Viewport) -> Result<Page> {
```

Inside `open`, the body is unchanged except the construction, which becomes:

```rust
        let page = Page { client, session_id };
```

Add to `impl Page`:

```rust
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
```

- [ ] **Step 2: Fix the test harness**

In `crates/wb-page/tests/extraction.rs`, add `use std::sync::Arc;` and replace lines 18-36. The lifetime comment on `Harness` goes away with the lifetime:

```rust
struct Harness {
    _browser: Chromium,
    client: Arc<Client>,
}

async fn harness() -> Harness {
    let browser = Chromium::launch().await.expect("launch chromium");
    let client = Client::connect(browser.ws_url()).await.expect("connect");
    Harness {
        _browser: browser,
        client: Arc::new(client),
    }
}

async fn open(h: &Harness, fixture: &str) -> Page {
    Page::open(Arc::clone(&h.client), &fixture_url(fixture), viewport())
        .await
        .expect("open the fixture")
}
```

- [ ] **Step 3: Fix the binary's wiring**

In `crates/webinal/src/lib.rs`, add `use std::sync::Arc;` and replace lines 15-17:

```rust
    let client = Client::connect(browser.ws_url())
        .await
        .context("connect to chromium")?;
    let page = Page::open(Arc::new(client), url, vp).await?;
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 61 tests pass, unchanged. Clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/wb-page crates/webinal
git commit -m "refactor(page): own the client through an Arc"
```

---

### Task 5: Install the script once per document

M1 re-evaluates the whole extraction script on every extraction. M2 installs it with `Page.addScriptToEvaluateOnNewDocument` so it survives navigation, and extraction becomes a call into it.

**Files:**
- Rename: `crates/wb-page/assets/extract.js` → `crates/wb-page/assets/bootstrap.js`
- Modify: `crates/wb-page/assets/bootstrap.js` (wrap the body in `window.__webinal.extract`)
- Modify: `crates/wb-page/src/extract.rs` (install on open, call the function)

**Interfaces:**
- Consumes: `Page` from Task 4.
- Produces: the page-side global `window.__webinal.extract()` returning `{ runs, title, url, scrollY, scrollHeight, innerHeight }`. `Page::extract` still returns `Result<Vec<TextRun>>` at the end of this task; Task 6 changes that.

- [ ] **Step 1: Rewrite the script as an installer**

Rename the file, then replace its contents. The extraction body is M1's, moved inside a named function unchanged:

```bash
git mv crates/wb-page/assets/extract.js crates/wb-page/assets/bootstrap.js
```

`crates/wb-page/assets/bootstrap.js`:

```js
// Installed once per document via Page.addScriptToEvaluateOnNewDocument, so
// it survives navigation. It defines the extraction entry point; the dirty
// signal listeners are added in the next task.
//
// The extraction body measures each character's rect individually and groups
// by rounded top. That is O(n) ranges per text node and slow on large pages,
// but it is exact and needs no heuristics about where lines break. A later
// task replaces the inner loop with a binary search over character offsets.
(() => {
  if (window.__webinal) return;

  function extract() {
    const runs = [];
    const vw = window.innerWidth;
    const vh = window.innerHeight;

    const walker = document.createTreeWalker(
      document.body,
      NodeFilter.SHOW_TEXT,
      null
    );

    const range = document.createRange();
    let node;

    while ((node = walker.nextNode())) {
      const text = node.nodeValue;
      if (!text || !text.trim()) continue;

      const parent = node.parentElement;
      if (!parent) continue;

      const cs = window.getComputedStyle(parent);
      if (cs.visibility === "hidden" || cs.display === "none" || cs.opacity === "0") {
        continue;
      }
      if (parent.tagName === "SCRIPT" || parent.tagName === "STYLE") continue;

      // Group the node's characters into lines by their rounded top edge.
      const lines = new Map();
      for (let i = 0; i < text.length; i++) {
        range.setStart(node, i);
        range.setEnd(node, i + 1);
        const r = range.getBoundingClientRect();
        if (r.width === 0 && r.height === 0) continue;

        const key = Math.round(r.top);
        let line = lines.get(key);
        if (!line) {
          line = { chars: [], left: r.left, right: r.right, top: r.top, bottom: r.bottom };
          lines.set(key, line);
        }
        line.chars.push(text[i]);
        line.left = Math.min(line.left, r.left);
        line.right = Math.max(line.right, r.right);
        line.bottom = Math.max(line.bottom, r.bottom);
      }

      const fontSize = parseFloat(cs.fontSize) || 16;
      const weight = parseInt(cs.fontWeight, 10) || 400;

      for (const line of lines.values()) {
        const content = line.chars.join("").replace(/\s+/g, " ").trim();
        if (!content) continue;

        // Cull runs entirely outside the viewport.
        if (line.bottom < 0 || line.top > vh || line.right < 0 || line.left > vw) {
          continue;
        }

        runs.push({
          text: content,
          x: line.left,
          y: line.top,
          w: line.right - line.left,
          h: line.bottom - line.top,
          // The descender is roughly a fifth of the font size; close enough to
          // put the baseline in the right cell row.
          baseline: line.bottom - fontSize * 0.21,
          color: cs.color,
          bold: weight >= 600,
          z: 0,
        });
      }
    }

    // Scroll geometry rides along with the runs so the statusline costs no
    // extra round trip.
    const doc = document.documentElement;
    return {
      runs,
      title: document.title,
      url: location.href,
      scrollY: window.scrollY,
      scrollHeight: Math.max(doc.scrollHeight, document.body ? document.body.scrollHeight : 0),
      innerHeight: window.innerHeight,
    };
  }

  window.__webinal = { extract };
})()
```

- [ ] **Step 2: Install it on open and call it on extract**

In `crates/wb-page/src/extract.rs`, change the constant:

```rust
const BOOTSTRAP_JS: &str = include_str!("../assets/bootstrap.js");
```

In `open`, after `let page = Page { client, session_id };` and **before** `page.set_viewport(vp).await?`:

```rust
        page.install_bootstrap().await?;
```

Add to `impl Page`:

```rust
    /// Install the page-side script for every document this target loads,
    /// including ones it navigates to later.
    async fn install_bootstrap(&self) -> Result<()> {
        self.client
            .call_on(
                &self.session_id,
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": BOOTSTRAP_JS }),
            )
            .await
            .context("install the bootstrap script")?;
        Ok(())
    }
```

Note that `Page.enable` currently happens inside `navigate`. Move it into `open`, directly before `install_bootstrap`, since the bootstrap install and the load events both need the domain enabled:

```rust
        page.client
            .call_on(&page.session_id, "Page.enable", json!({}))
            .await
            .context("enable the Page domain")?;
```

and delete the `Page.enable` call from the top of `navigate`.

In `extract`, replace the `"expression"` value:

```rust
                    "expression": "window.__webinal.extract()",
```

- [ ] **Step 3: Run the tests**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 61 tests pass, unchanged — the existing extraction tests are the proof that the move preserved behavior. Clippy clean.

- [ ] **Step 4: Commit**

```bash
git add crates/wb-page
git commit -m "feat(page): install the extraction script once per document"
```

---

### Task 6: The dirty signal and richer extraction

`Runtime.addBinding` gives the page a function it can call to tell us it changed. This is what makes the system event-driven: an idle page costs no CPU.

**Files:**
- Modify: `crates/wb-page/assets/bootstrap.js` (listeners)
- Modify: `crates/wb-page/src/extract.rs` (`Runtime.enable`, `addBinding`, `Extraction` type)
- Modify: `crates/wb-page/tests/extraction.rs` (call sites, new test)
- Create: `crates/wb-page/tests/fixtures/mutating.html`
- Modify: `crates/webinal/src/lib.rs:19` (call site)

**Interfaces:**
- Consumes: `wb_cdp::Event` from Task 3; `Page` from Task 5.
- Produces: `wb_page::Extraction { runs: Vec<TextRun>, title: String, url: String, scroll_y: f64, scroll_height: f64, viewport_height: f64 }` with `Extraction::scroll_progress(&self) -> f64`; `Page::extract(&self) -> Result<Extraction>`; `pub const DIRTY_BINDING: &str = "__webinal_dirty"`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/wb-page/tests/extraction.rs`:

```rust
#[tokio::test]
async fn a_dom_mutation_signals_dirtiness() {
    let h = harness().await;
    let mut events = h.client.subscribe();
    let page = open(&h, "mutating.html").await;

    // The fixture mutates itself 100ms after load, so the signal arrives
    // after we are already subscribed and watching.
    let signalled = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            if event.method == "Runtime.bindingCalled"
                && event.params["name"] == wb_page::DIRTY_BINDING
                && event.session_id.as_deref() == Some(page.session_id())
            {
                return true;
            }
        }
        false
    })
    .await
    .expect("the dirty binding should fire within ten seconds");

    assert!(signalled, "the CDP connection closed before the binding fired");
}

#[tokio::test]
async fn extraction_reports_scroll_geometry() {
    let h = harness().await;
    let extraction = open(&h, "simple.html").await.extract().await.expect("extract");

    assert_eq!(extraction.scroll_y, 0.0);
    assert!(extraction.viewport_height > 0.0);
    assert!(extraction.url.ends_with("simple.html"), "url was {}", extraction.url);
    assert_eq!(extraction.title, "Fixture Page");
}

#[test]
fn scroll_progress_is_zero_when_the_document_fits() {
    let e = wb_page::Extraction {
        runs: Vec::new(),
        title: String::new(),
        url: String::new(),
        scroll_y: 0.0,
        scroll_height: 400.0,
        viewport_height: 400.0,
    };
    assert_eq!(e.scroll_progress(), 0.0);
}

#[test]
fn scroll_progress_is_one_at_the_bottom() {
    let e = wb_page::Extraction {
        runs: Vec::new(),
        title: String::new(),
        url: String::new(),
        scroll_y: 600.0,
        scroll_height: 1000.0,
        viewport_height: 400.0,
    };
    assert_eq!(e.scroll_progress(), 1.0);
}
```

Update the six existing call sites in that file that do `.extract().await.expect("extract")` and then use the result as a `Vec<TextRun>` — lines 42, 52, 64, 81, 104 bind `let runs = ...`; append `.runs`:

```rust
    let runs = open(&h, "simple.html").await.extract().await.expect("extract").runs;
```

The `page.title()` assertion at line 98 becomes:

```rust
    assert_eq!(page.extract().await.expect("extract").title, "Fixture Page");
```

- [ ] **Step 2: Create the mutating fixture**

`crates/wb-page/tests/fixtures/mutating.html`:

```html
<!doctype html>
<meta charset="utf-8">
<title>Mutating Fixture</title>
<style>body { margin: 0; font: 16px/20px monospace; }</style>
<p id="target">before</p>
<script>
  // Mutate after load, so the observer is installed and we are subscribed.
  window.addEventListener("load", () => {
    setTimeout(() => {
      document.getElementById("target").textContent = "after";
    }, 100);
  });
</script>
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p wb-page
```

Expected: FAIL — `cannot find value DIRTY_BINDING in crate wb_page`, `cannot find struct Extraction`.

- [ ] **Step 4: Add the listeners to the bootstrap**

In `crates/wb-page/assets/bootstrap.js`, insert directly after `if (window.__webinal) return;`:

```js
  // Trailing debounces. Mutations are bursty and cheap to coalesce; scroll
  // fires per frame and must not outrun a single extraction.
  const MUTATION_DEBOUNCE_MS = 50;
  const SCROLL_DEBOUNCE_MS = 16;

  function signal() {
    // The binding may not be installed yet on the very first document.
    if (typeof window.__webinal_dirty === "function") {
      try {
        window.__webinal_dirty("");
      } catch (e) {
        // A torn-down context is not worth reporting.
      }
    }
  }

  function debounce(fn, ms) {
    let timer = null;
    return () => {
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(() => {
        timer = null;
        fn();
      }, ms);
    };
  }

  const onMutation = debounce(signal, MUTATION_DEBOUNCE_MS);
  const onScroll = debounce(signal, SCROLL_DEBOUNCE_MS);

  // `document` exists even at document-start, so the observer can be
  // attached before there is a body to observe.
  new MutationObserver(onMutation).observe(document, {
    subtree: true,
    childList: true,
    characterData: true,
    attributes: true,
  });

  // Capture, because scrolling inside a nested scroller does not bubble.
  window.addEventListener("scroll", onScroll, { passive: true, capture: true });
  window.addEventListener("load", signal);
```

- [ ] **Step 5: Register the binding and enable Runtime**

In `crates/wb-page/src/extract.rs`, add near the other constants:

```rust
/// The page-side function the injected script calls to say it changed.
/// Arrives back as a `Runtime.bindingCalled` event.
pub const DIRTY_BINDING: &str = "__webinal_dirty";
```

In `open`, directly after the `Page.enable` call added in Task 5:

```rust
        page.client
            .call_on(&page.session_id, "Runtime.enable", json!({}))
            .await
            .context("enable the Runtime domain")?;
        page.client
            .call_on(
                &page.session_id,
                "Runtime.addBinding",
                json!({ "name": DIRTY_BINDING }),
            )
            .await
            .context("install the dirty-signal binding")?;
```

Order matters: the binding must be registered before the first navigation, so it exists by the time the bootstrap runs.

- [ ] **Step 6: Add the `Extraction` type**

In `crates/wb-page/src/extract.rs`, extend the raw deserialization struct:

```rust
/// The shape `bootstrap.js` returns.
#[derive(Debug, Deserialize)]
struct RawExtraction {
    runs: Vec<RawRun>,
    title: String,
    url: String,
    #[serde(rename = "scrollY")]
    scroll_y: f64,
    #[serde(rename = "scrollHeight")]
    scroll_height: f64,
    #[serde(rename = "innerHeight")]
    inner_height: f64,
}
```

Add the public type:

```rust
/// One pass of the extraction script: everything the renderer and the
/// statusline need, from one round trip.
#[derive(Debug, Clone)]
pub struct Extraction {
    pub runs: Vec<TextRun>,
    pub title: String,
    pub url: String,
    pub scroll_y: f64,
    pub scroll_height: f64,
    pub viewport_height: f64,
}

impl Extraction {
    /// How far down the document we are: 0.0 at the top, 1.0 when the last
    /// line is on screen, and 0.0 when the document fits without scrolling.
    pub fn scroll_progress(&self) -> f64 {
        let scrollable = self.scroll_height - self.viewport_height;
        if scrollable <= 0.0 {
            return 0.0;
        }
        (self.scroll_y / scrollable).clamp(0.0, 1.0)
    }
}
```

Change `extract`'s signature and its tail:

```rust
    /// Run the extraction script and convert its output.
    pub async fn extract(&self) -> Result<Extraction> {
```

```rust
        Ok(Extraction {
            runs: raw
                .runs
                .into_iter()
                .map(|r| TextRun {
                    text: r.text,
                    rect: CssRect { x: r.x, y: r.y, w: r.w, h: r.h },
                    baseline: r.baseline,
                    style: Style {
                        fg: parse_css_color(&r.color),
                        bold: r.bold,
                        reverse: false,
                    },
                    z: r.z,
                })
                .collect(),
            title: raw.title,
            url: raw.url,
            scroll_y: raw.scroll_y,
            scroll_height: raw.scroll_height,
            viewport_height: raw.inner_height,
        })
```

Delete the now-redundant `title` method.

- [ ] **Step 7: Export and fix the binary**

`crates/wb-page/src/lib.rs`:

```rust
pub use extract::{DIRTY_BINDING, Extraction, Page};
```

`crates/webinal/src/lib.rs:19`:

```rust
    let extraction = page.extract().await?;
    let mut frame = Frame::new(vp.grid());
    for run in &extraction.runs {
        frame.paint_run(&vp, run);
    }
```

- [ ] **Step 8: Run the tests**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 65 tests pass (61 + 4 new). Clippy clean.

- [ ] **Step 9: Commit**

```bash
git add crates/wb-page crates/webinal
git commit -m "feat(page): signal dirtiness from the page and report scroll geometry"
```

---

### Task 7: Load events instead of polling

M1 polls `document.readyState` every 50ms. The event pump exists now, so use the real event.

**Files:**
- Modify: `crates/wb-page/src/extract.rs` (`navigate`, `wait_for_load`)

**Interfaces:**
- Consumes: `Client::subscribe` from Task 3.
- Produces: `Page::navigate(&self, url: &str) -> Result<()>` becomes **public**.

- [ ] **Step 1: Replace the polling loop**

In `crates/wb-page/src/extract.rs`, change the imports — `sleep` and `Instant` are no longer needed:

```rust
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use wb_cdp::{Client, Event};
```

Delete the `LOAD_POLL` constant. Replace `navigate` and `wait_for_load`:

```rust
    /// Navigate this page, and wait for its load event.
    pub async fn navigate(&self, url: &str) -> Result<()> {
        // Subscribe before issuing the command: the load event for a fast
        // page can arrive before the navigate response does.
        let mut events = self.client.subscribe();

        let result = self
            .client
            .call_on(&self.session_id, "Page.navigate", json!({ "url": url }))
            .await
            .with_context(|| format!("navigate to {url}"))?;

        if let Some(error) = result.get("errorText").and_then(|v| v.as_str()) {
            bail!("navigation to {url} failed: {error}");
        }

        self.wait_for_load(&mut events).await
    }

    async fn wait_for_load(&self, events: &mut mpsc::UnboundedReceiver<Event>) -> Result<()> {
        let watch = async {
            while let Some(event) = events.recv().await {
                if event.method == "Page.loadEventFired"
                    && event.session_id.as_deref() == Some(self.session_id.as_str())
                {
                    return Ok(());
                }
            }
            Err(anyhow!("the CDP connection closed while the page was loading"))
        };

        match timeout(LOAD_TIMEOUT, watch).await {
            Ok(result) => result,
            Err(_) => bail!("the page did not finish loading within {LOAD_TIMEOUT:?}"),
        }
    }
```

- [ ] **Step 2: Run the tests**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 65 tests pass, unchanged — every existing browser test navigates, so they are the proof. Clippy clean.

- [ ] **Step 3: Commit**

```bash
git add crates/wb-page
git commit -m "feat(page): wait for Page.loadEventFired instead of polling readyState"
```

---

### Task 8: Native scrolling

Scroll by dispatching a wheel event, so Chromium scrolls the page itself and sticky headers, infinite scroll, and virtualized lists work with no special handling.

**Files:**
- Modify: `crates/wb-page/src/extract.rs` (`scroll_by`, `scroll_to_top`, `scroll_to_end`)
- Create: `crates/wb-page/tests/fixtures/tall.html`
- Modify: `crates/wb-page/tests/extraction.rs` (tests)

**Interfaces:**
- Consumes: `Page` from Task 7.
- Produces: `Page::scroll_by(&self, dy: f64, vp: Viewport) -> Result<()>`; `Page::scroll_to_top(&self) -> Result<()>`; `Page::scroll_to_end(&self) -> Result<()>`.

- [ ] **Step 1: Write the failing tests**

`crates/wb-page/tests/fixtures/tall.html`:

```html
<!doctype html>
<meta charset="utf-8">
<title>Tall Fixture</title>
<style>
  body { margin: 0; font: 16px/20px monospace; }
  p { margin: 0; height: 20px; }
</style>
<script>
  for (let i = 0; i < 200; i++) {
    document.write(`<p>line ${i}</p>`);
  }
</script>
```

Add to `crates/wb-page/tests/extraction.rs`:

```rust
/// The wheel event is dispatched to the compositor, so the scroll it causes
/// is not complete when the command returns. Poll for the effect rather than
/// sleeping a fixed amount.
async fn await_scroll_past(page: &Page, floor: f64) -> f64 {
    for _ in 0..100 {
        let y = page.extract().await.expect("extract").scroll_y;
        if y > floor {
            return y;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the page never scrolled past {floor}");
}

#[tokio::test]
async fn scrolling_moves_the_page_and_changes_the_runs() {
    let h = harness().await;
    let page = open(&h, "tall.html").await;

    let before = page.extract().await.expect("extract");
    assert_eq!(before.scroll_y, 0.0);
    let first_before = before.runs.first().expect("a run").text.clone();

    page.scroll_by(200.0, viewport()).await.expect("scroll");
    await_scroll_past(&page, 0.0).await;

    let after = page.extract().await.expect("extract");
    let first_after = after.runs.first().expect("a run").text.clone();
    assert_ne!(
        first_before, first_after,
        "the topmost run should differ after scrolling"
    );
}

#[tokio::test]
async fn scroll_to_end_reaches_the_bottom() {
    let h = harness().await;
    let page = open(&h, "tall.html").await;

    page.scroll_to_end().await.expect("scroll to end");
    let end = page.extract().await.expect("extract");
    assert!(end.scroll_progress() > 0.99, "progress was {}", end.scroll_progress());

    page.scroll_to_top().await.expect("scroll to top");
    let top = page.extract().await.expect("extract");
    assert_eq!(top.scroll_y, 0.0);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p wb-page
```

Expected: FAIL — `no method named scroll_by found`.

- [ ] **Step 3: Implement the scroll methods**

Add to `impl Page` in `crates/wb-page/src/extract.rs`:

```rust
    /// Scroll by a distance in CSS pixels, positive being downward.
    ///
    /// This dispatches a real wheel event rather than calling `scrollBy`, so
    /// Chromium performs the scroll: sticky headers stick, infinite scroll
    /// loads, and virtualized lists virtualize, all with no help from us.
    pub async fn scroll_by(&self, dy: f64, vp: Viewport) -> Result<()> {
        self.client
            .call_on(
                &self.session_id,
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseWheel",
                    "x": f64::from(vp.css_width()) / 2.0,
                    "y": f64::from(vp.css_height()) / 2.0,
                    "deltaX": 0.0,
                    "deltaY": dy,
                    "button": "none",
                    "clickCount": 0,
                    "modifiers": 0,
                }),
            )
            .await
            .context("dispatch a wheel event")?;
        Ok(())
    }

    pub async fn scroll_to_top(&self) -> Result<()> {
        self.scroll_to("0").await
    }

    /// Jump to the end of the document.
    ///
    /// This is the one place M2 does not scroll natively: the distance to the
    /// document's end is not known to us, and on an infinite-scroll page it
    /// changes as we go. The consequence is that this reaches the end of what
    /// has loaded, which is the correct behavior — it is simply not
    /// wheel-driven.
    pub async fn scroll_to_end(&self) -> Result<()> {
        self.scroll_to("document.documentElement.scrollHeight").await
    }

    async fn scroll_to(&self, y_expression: &str) -> Result<()> {
        self.client
            .call_on(
                &self.session_id,
                "Runtime.evaluate",
                json!({
                    "expression": format!("window.scrollTo(0, {y_expression})"),
                    "returnByValue": true,
                }),
            )
            .await
            .context("scroll to a document position")?;
        Ok(())
    }
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 67 tests pass (65 + 2 new). Clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/wb-page
git commit -m "feat(page): scroll natively through wheel events"
```

---

### Task 9: History and reload

History is Chromium's. A `Vec<Url>` of our own would be smaller code that silently diverges the first time a page calls `pushState`.

**Files:**
- Modify: `crates/wb-page/src/extract.rs` (`go`, `back`, `forward`, `reload`)
- Modify: `crates/wb-page/tests/extraction.rs` (test)

**Interfaces:**
- Consumes: `Page::navigate` and `wait_for_load` from Task 7.
- Produces: `Page::back(&self) -> Result<bool>`; `Page::forward(&self) -> Result<bool>`; `Page::reload(&self) -> Result<()>`. The `bool` is `false` when there is no entry to move to, which is not an error.

- [ ] **Step 1: Write the failing test**

Add to `crates/wb-page/tests/extraction.rs`:

```rust
#[tokio::test]
async fn history_moves_back_and_forward() {
    let h = harness().await;
    let page = open(&h, "simple.html").await;
    page.navigate(&fixture_url("tall.html")).await.expect("navigate");

    assert!(page.back().await.expect("back"), "there should be an entry to go back to");
    assert!(
        page.extract().await.expect("extract").url.ends_with("simple.html"),
        "back should land on the first fixture"
    );

    assert!(page.forward().await.expect("forward"), "there should be an entry to go forward to");
    assert!(
        page.extract().await.expect("extract").url.ends_with("tall.html"),
        "forward should land on the second fixture"
    );

    assert!(!page.forward().await.expect("forward"), "there is nothing further forward");
}

#[tokio::test]
async fn reload_keeps_the_same_url() {
    let h = harness().await;
    let page = open(&h, "simple.html").await;
    page.reload().await.expect("reload");
    assert!(page.extract().await.expect("extract").url.ends_with("simple.html"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p wb-page
```

Expected: FAIL — `no method named back found`.

- [ ] **Step 3: Implement history**

Add to `impl Page` in `crates/wb-page/src/extract.rs`:

```rust
    /// Move `delta` entries through the browser's own history.
    ///
    /// Returns `false` when there is no such entry — the end of the history
    /// is a fact about the world, not an error.
    async fn go(&self, delta: i64) -> Result<bool> {
        let history = self
            .client
            .call_on(&self.session_id, "Page.getNavigationHistory", json!({}))
            .await
            .context("read the navigation history")?;

        let index = history["currentIndex"]
            .as_i64()
            .ok_or_else(|| anyhow!("the navigation history has no currentIndex"))?;
        let entries = history["entries"]
            .as_array()
            .ok_or_else(|| anyhow!("the navigation history has no entries"))?;

        let target = index + delta;
        if target < 0 || target >= entries.len() as i64 {
            return Ok(false);
        }
        let entry_id = entries[target as usize]["id"]
            .as_i64()
            .ok_or_else(|| anyhow!("a history entry has no id"))?;

        let mut events = self.client.subscribe();
        self.client
            .call_on(
                &self.session_id,
                "Page.navigateToHistoryEntry",
                json!({ "entryId": entry_id }),
            )
            .await
            .context("navigate to a history entry")?;
        self.wait_for_load(&mut events).await?;
        Ok(true)
    }

    pub async fn back(&self) -> Result<bool> {
        self.go(-1).await
    }

    pub async fn forward(&self) -> Result<bool> {
        self.go(1).await
    }

    pub async fn reload(&self) -> Result<()> {
        let mut events = self.client.subscribe();
        self.client
            .call_on(&self.session_id, "Page.reload", json!({}))
            .await
            .context("reload the page")?;
        self.wait_for_load(&mut events).await
    }
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 69 tests pass (67 + 2 new). Clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/wb-page
git commit -m "feat(page): move through Chromium's own navigation history"
```

---

### Task 10: Make extraction fast enough to scroll with

Every scroll now triggers a re-extraction, so extraction cost *is* scroll latency. The current inner loop calls `getBoundingClientRect` once per character. Replace it with `getClientRects` over the whole node plus a binary search for the line boundaries.

This task begins with a characterization test, so the rewrite is provably behavior-preserving.

**Files:**
- Create: `crates/wb-page/tests/fixtures/heavy.html`
- Create: `crates/wb-page/tests/snapshots/simple.txt`
- Modify: `crates/wb-page/tests/extraction.rs` (snapshot test, timing test)
- Modify: `crates/wb-page/assets/bootstrap.js` (the rewrite)

No manifest changes: `wb-frame`, which the snapshot helper needs, is already a dependency of `wb-page`.

**Interfaces:**
- Consumes: `Extraction` from Task 6.
- Produces: no new Rust API. The script's return shape is unchanged.

- [ ] **Step 1: Write the characterization test**

Add to `crates/wb-page/tests/extraction.rs`:

```rust
/// Paint an extraction into a frame and return it as lines of text. This is
/// the ASCII art that makes snapshot diffs readable in review.
fn snapshot(extraction: &wb_page::Extraction) -> String {
    let vp = viewport();
    let mut frame = wb_frame::Frame::new(vp.grid());
    for run in &extraction.runs {
        frame.paint_run(&vp, run);
    }
    (0..vp.grid().rows)
        .map(|row| frame.row_text(row))
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

#[tokio::test]
async fn simple_page_matches_its_snapshot() {
    let h = harness().await;
    let extraction = open(&h, "simple.html").await.extract().await.expect("extract");
    let got = snapshot(&extraction);

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/simple.txt");
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create the snapshot dir");
        std::fs::write(&path, format!("{got}\n")).expect("write the snapshot");
        return;
    }

    let want = std::fs::read_to_string(&path)
        .expect("missing snapshot; regenerate with UPDATE_SNAPSHOTS=1");
    assert_eq!(got, want.trim_end(), "the rendered page changed");
}
```

Add `wb-frame` to `crates/wb-page/Cargo.toml` dev-dependencies — it is already a normal dependency, so nothing changes; confirm rather than edit.

- [ ] **Step 2: Generate the snapshot from current behavior**

```bash
UPDATE_SNAPSHOTS=1 cargo test -p wb-page --test extraction simple_page_matches_its_snapshot
cargo test -p wb-page --test extraction simple_page_matches_its_snapshot
```

Expected: the second run PASSES against the file just written. Open `crates/wb-page/tests/snapshots/simple.txt` and confirm it reads as the fixture's text laid out on a grid. If it is empty or garbled, stop — the snapshot must be right before it can guard anything.

- [ ] **Step 3: Add the heavy fixture and the measurement**

`crates/wb-page/tests/fixtures/heavy.html`:

```html
<!doctype html>
<meta charset="utf-8">
<title>Heavy Fixture</title>
<style>body { margin: 0; font: 16px/20px sans-serif; width: 720px; }</style>
<script>
  const words = "the quick brown fox jumps over a lazy dog while nobody watches".split(" ");
  let html = "";
  for (let i = 0; i < 1500; i++) {
    const n = 8 + (i % 17);
    const text = Array.from({ length: n }, (_, k) => words[(i + k) % words.length]).join(" ");
    html += `<p>${text}</p>`;
  }
  document.write(html);
</script>
```

Add to `crates/wb-page/tests/extraction.rs`:

```rust
/// Not an assertion — a measurement. Run with `--nocapture` and record the
/// number; it is the floor on how fast a scroll can feel.
#[tokio::test]
async fn measure_extraction_of_a_heavy_page() {
    let h = harness().await;
    let page = open(&h, "heavy.html").await;

    // One warm pass, so the number is steady-state rather than first-run.
    page.extract().await.expect("extract");

    let start = std::time::Instant::now();
    let extraction = page.extract().await.expect("extract");
    let elapsed = start.elapsed();

    println!(
        "heavy.html: {} runs extracted in {elapsed:?}",
        extraction.runs.len()
    );
    assert!(!extraction.runs.is_empty());
}
```

- [ ] **Step 4: Record the before number**

```bash
cargo test -p wb-page --test extraction measure_extraction_of_a_heavy_page -- --nocapture
```

Write the printed duration down. It goes in the commit message at step 8.

- [ ] **Step 5: Rewrite the inner loop**

In `crates/wb-page/assets/bootstrap.js`, replace the per-character grouping loop inside `extract` — everything from `// Group the node's characters into lines by their rounded top edge.` down to the closing brace of that `for` loop — with a call to a new helper, and add the helper above `function extract()`:

```js
  // How far to scan forward past characters with no box (collapsed
  // whitespace) before giving up and using the caller's fallback.
  const EMPTY_SCAN_LIMIT = 8;

  // The top edge of the character at `index`, skipping over characters that
  // have no box of their own.
  function topAt(range, node, index, fallback) {
    const limit = Math.min(node.nodeValue.length, index + EMPTY_SCAN_LIMIT);
    for (let k = index; k < limit; k++) {
      range.setStart(node, k);
      range.setEnd(node, k + 1);
      const rect = range.getBoundingClientRect();
      if (rect.width > 0 || rect.height > 0) return rect.top;
    }
    return fallback;
  }

  // Split a text node into one entry per line box.
  //
  // getClientRects gives us the line boxes directly, so the only unknown is
  // where in the string each line begins. Character tops increase
  // monotonically through the string, so each boundary is a binary search
  // rather than a scan: O(lines * log chars) forced layouts instead of
  // O(chars).
  function linesOf(range, node) {
    const text = node.nodeValue;
    range.selectNodeContents(node);
    const rects = Array.from(range.getClientRects()).filter(
      (r) => r.width > 0 || r.height > 0
    );
    if (rects.length === 0) return [];
    if (rects.length === 1) {
      return [{ rect: rects[0], text }];
    }

    const lines = [];
    let start = 0;
    for (let i = 1; i < rects.length; i++) {
      // The first offset that has moved down to line i.
      const threshold = rects[i].top - 0.5;
      let lo = start;
      let hi = text.length;
      while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (topAt(range, node, mid, rects[i - 1].top) >= threshold) {
          hi = mid;
        } else {
          lo = mid + 1;
        }
      }
      lines.push({ rect: rects[i - 1], text: text.slice(start, lo) });
      start = lo;
    }
    lines.push({ rect: rects[rects.length - 1], text: text.slice(start) });
    return lines;
  }
```

Then the body of the `while ((node = walker.nextNode()))` loop, after the computed-style guards, becomes:

```js
      const fontSize = parseFloat(cs.fontSize) || 16;
      const weight = parseInt(cs.fontWeight, 10) || 400;

      for (const line of linesOf(range, node)) {
        const content = line.text.replace(/\s+/g, " ").trim();
        if (!content) continue;

        const r = line.rect;
        // Cull runs entirely outside the viewport.
        if (r.bottom < 0 || r.top > vh || r.right < 0 || r.left > vw) continue;

        runs.push({
          text: content,
          x: r.left,
          y: r.top,
          w: r.width,
          h: r.height,
          // The descender is roughly a fifth of the font size; close enough to
          // put the baseline in the right cell row.
          baseline: r.bottom - fontSize * 0.21,
          color: cs.color,
          bold: weight >= 600,
          z: 0,
        });
      }
```

Update the file's header comment: the paragraph describing the per-character measurement no longer describes the code. Replace it with a sentence naming `getClientRects` and the binary search.

- [ ] **Step 6: Verify behavior is unchanged**

```bash
cargo test -p wb-page
```

Expected: all `wb-page` tests PASS, **including `simple_page_matches_its_snapshot` against the unmodified snapshot file**. If the snapshot differs, do not regenerate it — the rewrite changed behavior, and you must find out why first. A one-character shift is a real bug in the boundary search, not noise.

- [ ] **Step 7: Record the after number**

```bash
cargo test -p wb-page --test extraction measure_extraction_of_a_heavy_page -- --nocapture
```

If the new number is not clearly better than the one from step 4, **stop and report it**. This task exists for that number; a rewrite that does not deliver it should be reverted rather than kept for tidiness.

- [ ] **Step 8: Commit**

```bash
git add crates/wb-page
git commit -m "perf(page): find line boundaries by binary search

heavy.html (1500 paragraphs): <before> -> <after>."
```

Replace `<before>` and `<after>` with the measured durations.

- [ ] **Step 9: Full workspace check**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 71 tests pass (69 + 2 new). Clippy clean.

---

### Task 11: Commands

Parsing is pure, so it is tested without a browser or a terminal.

**Files:**
- Create: `crates/webinal/src/command.rs`
- Modify: `crates/webinal/src/lib.rs` (declare the module)

**Interfaces:**
- Consumes: nothing.
- Produces: `webinal::command::Command` (`Open(String)`, `Back`, `Forward`, `Reload`, `Quit`), deriving `Debug, Clone, PartialEq`; `command::parse(line: &str) -> Result<Command, String>` where `line` has had its leading `:` stripped by the caller; `command::normalize_url(raw: &str) -> Result<String, String>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/webinal/src/command.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_takes_a_url() {
        assert_eq!(
            parse("open https://example.com"),
            Ok(Command::Open("https://example.com".to_string()))
        );
    }

    #[test]
    fn o_is_short_for_open() {
        assert_eq!(
            parse("o example.com"),
            Ok(Command::Open("https://example.com".to_string()))
        );
    }

    #[test]
    fn a_bare_host_gains_https() {
        assert_eq!(normalize_url("example.com"), Ok("https://example.com".to_string()));
    }

    #[test]
    fn an_explicit_scheme_is_left_alone() {
        assert_eq!(normalize_url("http://example.com"), Ok("http://example.com".to_string()));
        assert_eq!(normalize_url("file:///tmp/a.html"), Ok("file:///tmp/a.html".to_string()));
        assert_eq!(normalize_url("about:blank"), Ok("about:blank".to_string()));
    }

    #[test]
    fn something_that_is_not_a_url_is_an_error_not_a_search() {
        assert!(normalize_url("how tall is everest").is_err());
        assert!(normalize_url("notahost").is_err());
    }

    #[test]
    fn the_short_forms_parse() {
        assert_eq!(parse("q"), Ok(Command::Quit));
        assert_eq!(parse("quit"), Ok(Command::Quit));
        assert_eq!(parse("back"), Ok(Command::Back));
        assert_eq!(parse("forward"), Ok(Command::Forward));
        assert_eq!(parse("reload"), Ok(Command::Reload));
    }

    #[test]
    fn surrounding_whitespace_does_not_matter() {
        assert_eq!(parse("  quit  "), Ok(Command::Quit));
    }

    #[test]
    fn an_unknown_command_names_itself() {
        assert_eq!(parse("frobnicate"), Err("unknown command: frobnicate".to_string()));
    }

    #[test]
    fn open_without_an_argument_is_an_error() {
        assert!(parse("open").is_err());
    }

    #[test]
    fn an_empty_line_is_an_error() {
        assert!(parse("   ").is_err());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Add `pub mod command;` to `crates/webinal/src/lib.rs`, then:

```bash
cargo test -p webinal --lib
```

Expected: FAIL — `cannot find function parse in this scope`.

- [ ] **Step 3: Implement**

Add above the test module in `crates/webinal/src/command.rs`:

```rust
//! The `:` command line.

/// Schemes we pass through untouched. Anything else is treated as a bare
/// host and given `https://`.
const SCHEMES: &[&str] = &["http://", "https://", "file://", "about:", "data:"];

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Open(String),
    Back,
    Forward,
    Reload,
    Quit,
}

/// Parse a command line. The caller has already stripped the leading `:`.
pub fn parse(line: &str) -> Result<Command, String> {
    let line = line.trim();
    let (name, rest) = match line.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (line, ""),
    };

    match name {
        "" => Err("empty command".to_string()),
        "open" | "o" => {
            if rest.is_empty() {
                return Err("open needs a URL".to_string());
            }
            Ok(Command::Open(normalize_url(rest)?))
        }
        "back" | "b" => Ok(Command::Back),
        "forward" | "f" => Ok(Command::Forward),
        "reload" => Ok(Command::Reload),
        "quit" | "q" => Ok(Command::Quit),
        other => Err(format!("unknown command: {other}")),
    }
}

/// Turn what the user typed into a URL, or explain why it is not one.
///
/// There is deliberately no search-engine fallback: choosing a default
/// engine is a configuration question, and there is no configuration yet.
pub fn normalize_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("empty URL".to_string());
    }
    if SCHEMES.iter().any(|scheme| raw.starts_with(scheme)) {
        return Ok(raw.to_string());
    }
    if raw.split_whitespace().count() > 1 {
        return Err(format!("not a URL: {raw}"));
    }
    // A bare host needs at least one dot to be distinguishable from a typo.
    let host = raw.split(['/', '?', '#']).next().unwrap_or(raw);
    if !host.contains('.') {
        return Err(format!("not a URL: {raw}"));
    }
    Ok(format!("https://{raw}"))
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 81 tests pass (71 + 10 new). Clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/webinal
git commit -m "feat(webinal): parse the : command line"
```

---

### Task 12: Chrome and the keymap

Both are pure functions over state, so both are unit-tested without a terminal.

**Files:**
- Create: `crates/webinal/src/chrome.rs`
- Create: `crates/webinal/src/keymap.rs`
- Modify: `crates/webinal/src/lib.rs` (declare the modules)

**Interfaces:**
- Consumes: `Command` from Task 11; `Frame`, `Style`, `CellPos`, `Viewport` from `wb-frame`.
- Produces:
  - `chrome::State` (`Loading`, `Ready`, `Stalled`, `Error(String)`), deriving `Debug, Clone, PartialEq`
  - `chrome::Mode` (`Normal`, `Command(String)`), deriving `Debug, Clone, PartialEq`
  - `chrome::statusline(state: &State, url: &str, title: &str, progress: f64, cols: u16) -> String`
  - `chrome::command_line(buffer: &str, cols: u16) -> String`
  - `chrome::paint(frame: &mut Frame, mode: &Mode, state: &State, url: &str, title: &str, progress: f64)`
  - `keymap::Action` (`Scroll(f64)`, `ScrollTop`, `ScrollEnd`, `Back`, `Forward`, `Reload`, `EnterCommand(String)`, `Quit`), deriving `Debug, Clone, PartialEq`
  - `keymap::action_for(key: KeyEvent, vp: Viewport) -> Option<Action>`

- [ ] **Step 1: Write the failing chrome tests**

Create `crates/webinal/src/chrome.rs` with only its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wb_frame::{Frame, GridSize};

    #[test]
    fn statusline_shows_url_title_and_progress() {
        let line = statusline(&State::Ready, "https://example.com", "Example", 0.5, 40);
        assert!(line.contains("https://example.com"), "line was {line:?}");
        assert!(line.contains("Example"), "line was {line:?}");
        assert!(line.ends_with(" 50%"), "line was {line:?}");
    }

    #[test]
    fn statusline_is_exactly_the_grid_width() {
        for cols in [10u16, 40, 80, 200] {
            let line = statusline(&State::Ready, "https://example.com", "Example", 0.0, cols);
            assert_eq!(line.chars().count(), usize::from(cols), "at {cols} columns");
        }
    }

    #[test]
    fn statusline_tags_a_loading_page() {
        let line = statusline(&State::Loading, "https://example.com", "", 0.0, 60);
        assert!(line.starts_with("[loading]"), "line was {line:?}");
    }

    #[test]
    fn statusline_shows_the_error_text() {
        let state = State::Error("could not resolve host".to_string());
        let line = statusline(&state, "https://exmaple.com", "", 0.0, 60);
        assert!(line.contains("could not resolve host"), "line was {line:?}");
    }

    #[test]
    fn a_long_url_is_truncated_rather_than_overflowing() {
        let url = "https://example.com/".to_string() + &"a".repeat(500);
        let line = statusline(&State::Ready, &url, "", 0.0, 40);
        assert_eq!(line.chars().count(), 40);
    }

    #[test]
    fn the_command_line_shows_what_was_typed() {
        assert!(command_line("open exa", 20).starts_with(":open exa"));
    }

    #[test]
    fn paint_puts_chrome_on_the_last_row() {
        let mut frame = Frame::new(GridSize { cols: 30, rows: 3 });
        paint(&mut frame, &Mode::Normal, &State::Ready, "https://example.com", "", 0.0);
        assert_eq!(frame.row_text(0), "");
        assert!(frame.row_text(2).contains("example.com"), "row 2 was {:?}", frame.row_text(2));
    }

    #[test]
    fn paint_shows_the_command_line_instead_when_in_command_mode() {
        let mut frame = Frame::new(GridSize { cols: 30, rows: 3 });
        let mode = Mode::Command("open exa".to_string());
        paint(&mut frame, &mode, &State::Ready, "https://example.com", "", 0.0);
        assert!(frame.row_text(2).starts_with(":open exa"), "row 2 was {:?}", frame.row_text(2));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Add `pub mod chrome;` to `crates/webinal/src/lib.rs`, then:

```bash
cargo test -p webinal --lib
```

Expected: FAIL — `cannot find function statusline in this scope`.

- [ ] **Step 3: Implement chrome**

Add above the test module in `crates/webinal/src/chrome.rs`:

```rust
//! The bottom row: a statusline, or the `:` command line when one is open.

use wb_frame::{CellPos, Frame, GridSize, Rgb, Style};

/// What the page is doing. Shown in the statusline; never a reason to blank
/// the frame.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Loading,
    Ready,
    Stalled,
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    /// The `:` line is open, holding what has been typed so far.
    Command(String),
}

fn chrome_style() -> Style {
    Style {
        fg: Rgb { r: 0xd0, g: 0xd0, b: 0xd0 },
        bold: false,
        reverse: true,
    }
}

/// Build the statusline, padded or truncated to exactly `cols` characters.
pub fn statusline(state: &State, url: &str, title: &str, progress: f64, cols: u16) -> String {
    let tag = match state {
        State::Ready => String::new(),
        State::Loading => "[loading] ".to_string(),
        State::Stalled => "[stalled] ".to_string(),
        State::Error(message) => format!("[error] {message} — "),
    };

    let left = if title.is_empty() {
        format!("{tag}{url}")
    } else {
        format!("{tag}{url} — {title}")
    };

    let percent = format!("{:>3}%", (progress * 100.0).round() as i64);
    let cols = usize::from(cols);

    // On a very narrow terminal the percentage is what gets dropped, not the
    // URL: knowing where you are matters more than how far down you are.
    if cols <= percent.chars().count() + 1 {
        return fit(&left, cols);
    }

    let room = cols - percent.chars().count() - 1;
    format!("{}{}{}", fit(&left, room), " ", percent)
}

/// The `:` line, padded or truncated to exactly `cols` characters.
pub fn command_line(buffer: &str, cols: u16) -> String {
    fit(&format!(":{buffer}"), usize::from(cols))
}

/// Truncate or pad a string to exactly `width` characters.
fn fit(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count >= width {
        s.chars().take(width).collect()
    } else {
        let mut out = String::with_capacity(width);
        out.push_str(s);
        out.extend(std::iter::repeat_n(' ', width - count));
        out
    }
}

/// Paint the bottom row of the frame.
pub fn paint(
    frame: &mut Frame,
    mode: &Mode,
    state: &State,
    url: &str,
    title: &str,
    progress: f64,
) {
    let GridSize { cols, rows } = frame.grid();
    if rows == 0 {
        return;
    }
    let text = match mode {
        Mode::Normal => statusline(state, url, title, progress, cols),
        Mode::Command(buffer) => command_line(buffer, cols),
    };
    frame.paint_text(CellPos { col: 0, row: rows - 1 }, &text, chrome_style());
}
```

Add `Frame` and `GridSize` to the test module's imports by ensuring `use super::*;` is joined by `use wb_frame::{Frame, GridSize};` inside `mod tests`.

- [ ] **Step 4: Write the failing keymap tests**

Create `crates/webinal/src/keymap.rs` with only its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use wb_frame::{CellSize, GridSize};

    fn vp() -> Viewport {
        // 24 page rows of 20 CSS pixels each.
        Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 })
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn j_and_k_scroll_one_cell() {
        assert_eq!(action_for(key('j'), vp()), Some(Action::Scroll(20.0)));
        assert_eq!(action_for(key('k'), vp()), Some(Action::Scroll(-20.0)));
    }

    #[test]
    fn d_and_u_scroll_half_a_page() {
        assert_eq!(action_for(key('d'), vp()), Some(Action::Scroll(240.0)));
        assert_eq!(action_for(key('u'), vp()), Some(Action::Scroll(-240.0)));
    }

    #[test]
    fn space_and_b_scroll_a_page_less_two_rows_of_overlap() {
        assert_eq!(action_for(key(' '), vp()), Some(Action::Scroll(440.0)));
        assert_eq!(action_for(key('b'), vp()), Some(Action::Scroll(-440.0)));
    }

    #[test]
    fn g_and_shift_g_jump_to_the_ends() {
        assert_eq!(action_for(key('g'), vp()), Some(Action::ScrollTop));
        assert_eq!(action_for(key('G'), vp()), Some(Action::ScrollEnd));
    }

    #[test]
    fn history_and_reload_are_bound() {
        assert_eq!(action_for(key('H'), vp()), Some(Action::Back));
        assert_eq!(action_for(key('L'), vp()), Some(Action::Forward));
        assert_eq!(
            action_for(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL), vp()),
            Some(Action::Reload)
        );
    }

    #[test]
    fn colon_opens_an_empty_command_line_and_o_prefills_open() {
        assert_eq!(action_for(key(':'), vp()), Some(Action::EnterCommand(String::new())));
        assert_eq!(
            action_for(key('o'), vp()),
            Some(Action::EnterCommand("open ".to_string()))
        );
    }

    #[test]
    fn q_quits() {
        assert_eq!(action_for(key('q'), vp()), Some(Action::Quit));
    }

    #[test]
    fn an_unbound_key_does_nothing() {
        assert_eq!(action_for(key('z'), vp()), None);
    }

    #[test]
    fn a_one_row_viewport_never_scrolls_backwards() {
        let tiny = Viewport::new(GridSize { cols: 20, rows: 1 }, CellSize { w: 9, h: 20 });
        // rows - 2 would underflow; a page scroll must still move forward.
        assert_eq!(action_for(key(' '), tiny), Some(Action::Scroll(20.0)));
    }
}
```

- [ ] **Step 5: Run to verify it fails**

Add `pub mod keymap;` to `crates/webinal/src/lib.rs`, then:

```bash
cargo test -p webinal --lib
```

Expected: FAIL — `cannot find type Action in this scope`.

- [ ] **Step 6: Implement the keymap**

Add above the test module in `crates/webinal/src/keymap.rs`:

```rust
//! Normal-mode keys. Pure, so the scroll arithmetic is testable.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use wb_frame::Viewport;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Scroll by a distance in CSS pixels, positive being downward.
    Scroll(f64),
    ScrollTop,
    ScrollEnd,
    Back,
    Forward,
    Reload,
    /// Open the `:` line, pre-filled with this text.
    EnterCommand(String),
    Quit,
}

/// The distance one `space` moves: a screenful, less two rows kept for
/// context so you do not lose your place across the jump.
fn page(vp: Viewport) -> f64 {
    let rows = vp.grid().rows.saturating_sub(2).max(1);
    f64::from(rows) * f64::from(vp.cell().h)
}

fn half_page(vp: Viewport) -> f64 {
    let rows = (vp.grid().rows / 2).max(1);
    f64::from(rows) * f64::from(vp.cell().h)
}

fn line(vp: Viewport) -> f64 {
    f64::from(vp.cell().h)
}

pub fn action_for(key: KeyEvent, vp: Viewport) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('r') => Some(Action::Reload),
            _ => None,
        };
    }

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(Action::Scroll(line(vp))),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::Scroll(-line(vp))),
        KeyCode::Char('d') => Some(Action::Scroll(half_page(vp))),
        KeyCode::Char('u') => Some(Action::Scroll(-half_page(vp))),
        KeyCode::Char(' ') | KeyCode::PageDown => Some(Action::Scroll(page(vp))),
        KeyCode::Char('b') | KeyCode::PageUp => Some(Action::Scroll(-page(vp))),
        KeyCode::Char('g') | KeyCode::Home => Some(Action::ScrollTop),
        KeyCode::Char('G') | KeyCode::End => Some(Action::ScrollEnd),
        KeyCode::Char('H') => Some(Action::Back),
        KeyCode::Char('L') => Some(Action::Forward),
        KeyCode::Char(':') => Some(Action::EnterCommand(String::new())),
        KeyCode::Char('o') => Some(Action::EnterCommand("open ".to_string())),
        KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}
```

- [ ] **Step 7: Run the tests**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 99 tests pass (81 + 8 chrome + 10 keymap). Clippy clean.

- [ ] **Step 8: Commit**

```bash
git add crates/webinal
git commit -m "feat(webinal): add the statusline, command line, and normal-mode keymap"
```

---

### Task 13: The core loop

Everything above meets here. The core owns all state and is the only thing that mutates it. Page operations run as spawned tasks reporting back on one channel, so a thirty-second load never blocks a keypress.

**Files:**
- Modify: `Cargo.toml` (crossterm `event-stream` feature)
- Modify: `crates/webinal/Cargo.toml` (add `futures-util`)
- Create: `crates/webinal/src/core.rs`
- Modify: `crates/webinal/src/lib.rs` (declare the module)

**Interfaces:**
- Consumes: `Renderer` (Task 2), `Event`/`Client` (Task 3), `Page`/`Extraction`/`DIRTY_BINDING` (Tasks 4–9), `Command` (Task 11), `chrome::{State, Mode, paint}` and `keymap::{Action, action_for}` (Task 12).
- Produces: `core::Core` with `Core::new(page: Arc<Page>, client: Arc<Client>, grid: GridSize, cell: CellSize) -> Core`, `Core::run(&mut self, out: &mut impl Write) -> Result<()>`, and `Core::compose(&self) -> Frame`; the free function `core::page_viewport(grid: GridSize, cell: CellSize) -> Viewport`, made `pub` in Task 14.

- [ ] **Step 1: Add the dependencies**

Workspace `Cargo.toml`:

```toml
crossterm = { version = "0.29", features = ["event-stream"] }
```

`crates/webinal/Cargo.toml`, in `[dependencies]`:

```toml
futures-util.workspace = true
```

Verify nothing else broke:

```bash
cargo build --workspace
```

- [ ] **Step 2: Write the failing tests**

Create `crates/webinal/src/core.rs` with only its test module. These test the parts that need no browser: the page viewport arithmetic, mode transitions, and composition.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn the_page_viewport_is_one_row_shorter_than_the_terminal() {
        let vp = page_viewport(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
        assert_eq!(vp.grid(), GridSize { cols: 80, rows: 23 });
        assert_eq!(vp.css_height(), 23 * 20);
    }

    #[test]
    fn a_one_row_terminal_still_leaves_a_page_row() {
        let vp = page_viewport(GridSize { cols: 80, rows: 1 }, CellSize { w: 9, h: 20 });
        assert_eq!(vp.grid().rows, 1, "never zero, or Chromium gets a zero-height window");
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Add `pub mod core;` to `crates/webinal/src/lib.rs`, then:

```bash
cargo test -p webinal --lib
```

Expected: FAIL — `cannot find function page_viewport in this scope`.

- [ ] **Step 4: Implement the core**

Add above the test module in `crates/webinal/src/core.rs`:

```rust
//! The event loop. It owns all state and is the only thing that mutates it.

use std::io::Write;
use std::sync::Arc;

use anyhow::{Context, Result};
use crossterm::event::{Event as TermEvent, EventStream, KeyCode, KeyEvent, KeyEventKind};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, sleep_until};
use wb_cdp::Client;
use wb_frame::{CellSize, Frame, GridSize, TextRun, Viewport};
use wb_page::{DIRTY_BINDING, Extraction, Page};
use wb_term::Renderer;

use crate::chrome::{self, Mode, State};
use crate::command::{self, Command};
use crate::keymap::{Action, action_for};

/// A dragged window edge produces a resize event per frame, and each one
/// would otherwise cost a Chromium relayout and a full extraction.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(100);

/// The result of something that ran off the loop's thread.
enum Job {
    Extracted(Box<Extraction>),
    Failed(String),
    /// A navigation, history move, or reload finished.
    Settled,
}

/// The page viewport: the terminal grid, less the row chrome occupies.
///
/// Chromium is told this is the whole window, so the page genuinely does not
/// know the statusline exists.
fn page_viewport(grid: GridSize, cell: CellSize) -> Viewport {
    let rows = grid.rows.saturating_sub(1).max(1);
    Viewport::new(GridSize { cols: grid.cols, rows }, cell)
}

pub struct Core {
    page: Arc<Page>,
    client: Arc<Client>,
    grid: GridSize,
    cell: CellSize,
    vp: Viewport,
    renderer: Renderer,

    mode: Mode,
    state: State,
    url: String,
    title: String,
    progress: f64,
    runs: Vec<TextRun>,

    /// The page says it changed and we have not caught up yet.
    dirty: bool,
    /// An extraction is in flight; a second would race it.
    extracting: bool,
    /// A navigation is in flight.
    navigating: bool,

    jobs_tx: mpsc::UnboundedSender<Job>,
    jobs_rx: mpsc::UnboundedReceiver<Job>,
}

impl Core {
    pub fn new(page: Arc<Page>, client: Arc<Client>, grid: GridSize, cell: CellSize) -> Self {
        let (jobs_tx, jobs_rx) = mpsc::unbounded_channel();
        Self {
            page,
            client,
            grid,
            cell,
            vp: page_viewport(grid, cell),
            renderer: Renderer::new(),
            mode: Mode::Normal,
            state: State::Loading,
            url: String::new(),
            title: String::new(),
            progress: 0.0,
            runs: Vec::new(),
            dirty: true,
            extracting: false,
            navigating: false,
            jobs_tx,
            jobs_rx,
        }
    }

    /// Paint the page and the chrome into one full-grid frame.
    pub fn compose(&self) -> Frame {
        let mut frame = Frame::new(self.grid);
        for run in &self.runs {
            frame.paint_run(&self.vp, run);
        }
        chrome::paint(
            &mut frame,
            &self.mode,
            &self.state,
            &self.url,
            &self.title,
            self.progress,
        );
        frame
    }

    pub async fn run(&mut self, out: &mut impl Write) -> Result<()> {
        let mut terminal = EventStream::new();
        let mut cdp = self.client.subscribe();
        let mut resize_at: Option<Instant> = None;

        self.start_extract();
        self.present(out)?;

        loop {
            tokio::select! {
                Some(Ok(event)) = terminal.next() => {
                    match event {
                        TermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                            if self.on_key(key) {
                                return Ok(());
                            }
                        }
                        TermEvent::Resize(..) => {
                            resize_at = Some(Instant::now() + RESIZE_DEBOUNCE);
                        }
                        _ => {}
                    }
                }

                Some(event) = cdp.recv() => {
                    let ours = event.session_id.as_deref() == Some(self.page.session_id());
                    if ours
                        && event.method == "Runtime.bindingCalled"
                        && event.params["name"] == DIRTY_BINDING
                    {
                        self.dirty = true;
                        self.start_extract();
                    }
                }

                Some(job) = self.jobs_rx.recv() => {
                    self.on_job(job);
                }

                () = async { sleep_until(resize_at.expect("guarded")).await },
                    if resize_at.is_some() =>
                {
                    resize_at = None;
                    self.on_resize().await?;
                }
            }

            self.present(out)?;
        }
    }

    fn present(&mut self, out: &mut impl Write) -> Result<()> {
        let frame = self.compose();
        self.renderer.render(&frame, out).context("write the frame")?;
        out.flush().context("flush the terminal")?;
        Ok(())
    }

    /// Handle one key. Returns `true` when it is time to quit.
    fn on_key(&mut self, key: KeyEvent) -> bool {
        match &self.mode {
            Mode::Command(buffer) => {
                let mut buffer = buffer.clone();
                match key.code {
                    KeyCode::Esc => self.mode = Mode::Normal,
                    KeyCode::Backspace => {
                        buffer.pop();
                        self.mode = Mode::Command(buffer);
                    }
                    KeyCode::Enter => {
                        self.mode = Mode::Normal;
                        match command::parse(&buffer) {
                            Ok(Command::Quit) => return true,
                            Ok(command) => self.run_command(command),
                            Err(message) => self.state = State::Error(message),
                        }
                    }
                    KeyCode::Char(c) => {
                        buffer.push(c);
                        self.mode = Mode::Command(buffer);
                    }
                    _ => {}
                }
                false
            }
            Mode::Normal => match action_for(key, self.vp) {
                Some(Action::Quit) => true,
                Some(Action::EnterCommand(prefill)) => {
                    self.mode = Mode::Command(prefill);
                    false
                }
                Some(action) => {
                    self.run_action(action);
                    false
                }
                None => false,
            },
        }
    }

    fn run_action(&mut self, action: Action) {
        let page = Arc::clone(&self.page);
        let tx = self.jobs_tx.clone();
        let vp = self.vp;

        match action {
            Action::Scroll(dy) => {
                // Scrolling does not settle the way a navigation does; the
                // page's own scroll listener reports when it has moved.
                tokio::spawn(async move {
                    if let Err(error) = page.scroll_by(dy, vp).await {
                        let _ = tx.send(Job::Failed(error.to_string()));
                    }
                });
            }
            Action::ScrollTop => {
                tokio::spawn(async move {
                    if let Err(error) = page.scroll_to_top().await {
                        let _ = tx.send(Job::Failed(error.to_string()));
                    }
                });
            }
            Action::ScrollEnd => {
                tokio::spawn(async move {
                    if let Err(error) = page.scroll_to_end().await {
                        let _ = tx.send(Job::Failed(error.to_string()));
                    }
                });
            }
            Action::Back => self.navigate_with(move |page| async move { page.back().await.map(|_| ()) }),
            Action::Forward => {
                self.navigate_with(move |page| async move { page.forward().await.map(|_| ()) })
            }
            Action::Reload => self.navigate_with(move |page| async move { page.reload().await }),
            // Handled by the caller.
            Action::Quit | Action::EnterCommand(_) => {}
        }
    }

    fn run_command(&mut self, command: Command) {
        match command {
            Command::Open(url) => {
                self.url = url.clone();
                self.navigate_with(move |page| async move { page.navigate(&url).await });
            }
            Command::Back => self.run_action(Action::Back),
            Command::Forward => self.run_action(Action::Forward),
            Command::Reload => self.run_action(Action::Reload),
            // Handled by the caller.
            Command::Quit => {}
        }
    }

    /// Run something that changes what page we are on, off the loop's thread.
    ///
    /// The previous page stays on screen, marked loading, until the new one
    /// has been extracted. Nothing a page does blanks the frame.
    fn navigate_with<F, Fut>(&mut self, make: F)
    where
        F: FnOnce(Arc<Page>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send,
    {
        if self.navigating {
            return;
        }
        self.navigating = true;
        self.state = State::Loading;

        let page = Arc::clone(&self.page);
        let tx = self.jobs_tx.clone();
        tokio::spawn(async move {
            let job = match make(page).await {
                Ok(()) => Job::Settled,
                Err(error) => Job::Failed(error.to_string()),
            };
            let _ = tx.send(job);
        });
    }

    fn start_extract(&mut self) {
        if self.extracting || !self.dirty {
            return;
        }
        self.extracting = true;
        self.dirty = false;

        let page = Arc::clone(&self.page);
        let tx = self.jobs_tx.clone();
        tokio::spawn(async move {
            let job = match page.extract().await {
                Ok(extraction) => Job::Extracted(Box::new(extraction)),
                Err(error) => Job::Failed(error.to_string()),
            };
            let _ = tx.send(job);
        });
    }

    fn on_job(&mut self, job: Job) {
        match job {
            Job::Extracted(extraction) => {
                self.extracting = false;
                self.progress = extraction.scroll_progress();
                self.runs = extraction.runs;
                self.title = extraction.title;
                self.url = extraction.url;
                if !self.navigating {
                    self.state = State::Ready;
                }
                // The page may have changed again while we were extracting.
                self.start_extract();
            }
            Job::Settled => {
                self.navigating = false;
                self.state = State::Ready;
                self.dirty = true;
                self.start_extract();
            }
            Job::Failed(message) => {
                self.extracting = false;
                self.navigating = false;
                // The frame stays exactly as it was; only the statusline
                // changes. Section 8: never blank the frame you are looking at.
                self.state = State::Error(message);
            }
        }
    }

    async fn on_resize(&mut self) -> Result<()> {
        let (grid, cell) = wb_term::probe().context("re-measure the terminal")?;
        if grid == self.grid && cell == self.cell {
            return Ok(());
        }

        self.grid = grid;
        self.cell = cell;
        self.vp = page_viewport(grid, cell);

        // The page genuinely reflows: it is being told the window changed size.
        self.page
            .set_viewport(self.vp)
            .await
            .context("resize the page viewport")?;

        // A diff against a frame of different dimensions is meaningless.
        self.renderer.invalidate();
        self.dirty = true;
        self.start_extract();
        Ok(())
    }
}
```

Add these imports to the test module so it compiles: `use wb_frame::{CellSize, GridSize};` is already covered by `use super::*;`.

- [ ] **Step 5: Run the tests**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 101 tests pass (99 + 2 new). Clippy clean.

If clippy objects to `Job::Extracted(Box<Extraction>)` or to the `select!` guard, fix the code rather than silencing the lint.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/webinal
git commit -m "feat(webinal): add the core event loop"
```

---

### Task 14: Wire up the binary

**Files:**
- Modify: `crates/webinal/src/main.rs`
- Modify: `crates/webinal/src/lib.rs` (keep `render_url` for the smoke tests)
- Modify: `crates/webinal/tests/smoke.rs` (a modal-flow test)
- Modify: `README.md`
- Modify: `docs/superpowers/specs/2026-08-19-webinal-m2-design.md` (§11, the PTY row)

**Interfaces:**
- Consumes: `Core` from Task 13.
- Produces: the `webinal` binary.

- [ ] **Step 1: Rewrite `main`**

`crates/webinal/src/main.rs`:

```rust
use std::io::{Write, stdout};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use wb_cdp::{Chromium, Client};
use wb_page::Page;
use webinal::command::normalize_url;
use webinal::core::Core;

#[tokio::main]
async fn main() -> Result<()> {
    let Some(argument) = std::env::args().nth(1) else {
        bail!("usage: webinal <url>");
    };
    let url = normalize_url(&argument).map_err(|message| anyhow::anyhow!(message))?;

    let (grid, cell) = wb_term::probe().context("measure the terminal")?;

    // Everything that can fail loudly happens before we touch the terminal,
    // so a failure leaves the user's screen exactly as it was.
    let browser = Chromium::launch().await.context("launch chromium")?;
    let client = Arc::new(
        Client::connect(browser.ws_url())
            .await
            .context("connect to chromium")?,
    );
    let vp = webinal::core::page_viewport(grid, cell);
    let page = Arc::new(Page::open(Arc::clone(&client), &url, vp).await?);

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, cursor::Hide)?;

    let mut core = Core::new(page, client, grid, cell);
    let mut out = stdout();
    let result = core.run(&mut out).await;
    let _ = out.flush();

    execute!(stdout(), cursor::Show, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    result
}
```

Make `page_viewport` public in `crates/webinal/src/core.rs`:

```rust
pub fn page_viewport(grid: GridSize, cell: CellSize) -> Viewport {
```

- [ ] **Step 2: Keep `render_url` working**

`crates/webinal/src/lib.rs` gains the module declarations and keeps `render_url` unchanged from Task 6:

```rust
//! Wiring: browser, page, frame.

pub mod chrome;
pub mod command;
pub mod core;
pub mod keymap;
```

- [ ] **Step 3: Write the modal-flow test**

The spec's testing table names a PTY test. A `Core`-level test is strictly better here: it exercises the same modal flow deterministically, needs no new dependency, and cannot flake on timing. Add to `crates/webinal/tests/smoke.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use webinal::chrome::Mode;

/// Drive the modal flow without a browser or a terminal: `:` opens the
/// command line, typing fills it, `Esc` closes it.
#[test]
fn the_command_line_opens_fills_and_closes() {
    // The keymap decides that `:` opens an empty command line.
    let vp = wb_frame::Viewport::new(
        wb_frame::GridSize { cols: 80, rows: 24 },
        wb_frame::CellSize { w: 9, h: 20 },
    );
    let action = webinal::keymap::action_for(
        KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE),
        vp,
    );
    let Some(webinal::keymap::Action::EnterCommand(prefill)) = action else {
        panic!("`:` should open the command line, got {action:?}");
    };
    let mut mode = Mode::Command(prefill);

    // Typing accumulates, and the chrome row shows it.
    if let Mode::Command(buffer) = &mut mode {
        for c in "open example.com".chars() {
            buffer.push(c);
        }
    }
    let mut frame = wb_frame::Frame::new(wb_frame::GridSize { cols: 40, rows: 3 });
    webinal::chrome::paint(&mut frame, &mode, &webinal::chrome::State::Ready, "", "", 0.0);
    assert!(
        frame.row_text(2).starts_with(":open example.com"),
        "row 2 was {:?}",
        frame.row_text(2)
    );

    // And the command it holds parses to the navigation we expect.
    if let Mode::Command(buffer) = &mode {
        assert_eq!(
            webinal::command::parse(buffer),
            Ok(webinal::command::Command::Open("https://example.com".to_string()))
        );
    }
}
```

Add `wb-frame` and `crossterm` to `crates/webinal/Cargo.toml` — both are already dependencies, so confirm rather than edit.

- [ ] **Step 4: Correct the spec**

In `docs/superpowers/specs/2026-08-19-webinal-m2-design.md`, section 11, replace the `webinal` row's last sentence:

```
| `webinal` | Command parsing (`:open example.com` → `https://example.com`; unparseable input → error). Scroll arithmetic at each key. One test driving the modal flow through `Core`'s own types rather than a PTY: it covers the same `:`-to-command path deterministically, needs no new dependency, and cannot flake on process timing. |
```

- [ ] **Step 5: Update the README**

In `README.md`, replace the status paragraph and the usage section:

```markdown
**Status: M2, navigation and reading.** It renders a page, scrolls it,
follows history, and opens other URLs. Clicking, typing, tabs, and pixel
mode are M3 through M5.
```

```markdown
## Usage

    cargo run -p webinal -- example.com

| Key | |
|---|---|
| `j` `k` | scroll a line |
| `d` `u` | scroll half a screen |
| `space` `b` | scroll a screen |
| `g` `G` | top, bottom |
| `H` `L` | back, forward |
| `Ctrl-r` | reload |
| `o` | open a URL |
| `:` | command line |
| `q` | quit |

Commands: `:open <url>`, `:back`, `:forward`, `:reload`, `:quit`.
```

- [ ] **Step 6: Run everything**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 102 tests pass (101 + 1 new). Clippy clean.

- [ ] **Step 7: Drive it by hand**

This is the step that catches what tests do not.

```bash
cargo run -p webinal -- example.com
```

Confirm, in order:

1. The page's text appears, laid out roughly where a browser would put it.
2. The bottom row shows the URL, the title, and `  0%` in reverse video.
3. `j` and `k` scroll by one line; `space` and `b` by a screen; `G` reaches the bottom and the percentage reads `100%`.
4. `:open news.ycombinator.com` then `Enter` navigates. While it loads, the *old* page stays on screen with `[loading]` in the statusline.
5. `H` goes back to example.com; `L` returns.
6. `:open notarealdomain.invalid` shows `[error]` in the statusline and **leaves the previous page on screen**.
7. Resizing the terminal reflows the page after a short pause, with no corruption.
8. `q` exits and the terminal is exactly as it was — no leftover alternate screen, cursor visible, no stray colors.

Any failure here is a bug to fix before the task is done, not a note to file.

- [ ] **Step 8: Commit**

```bash
git add crates/webinal README.md docs
git commit -m "feat(webinal): drive the browser from the core loop"
```

---

## Definition of done for M2

- `cargo test --workspace` is green, with 102 tests: 22 `wb-frame`, 18 `wb-term`, 7 `wb-cdp`, 22 `wb-page`, 33 `webinal`.
- `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- The heavy-page extraction measurement from Task 10 is recorded in that task's commit message, and the after number is better than the before number.
- `crates/wb-page/tests/snapshots/simple.txt` passes unmodified through the Task 10 rewrite.
- Every item in Task 14's manual checklist behaves as described, including the two that matter most: a failed navigation leaves the previous page on screen, and `q` restores the terminal.

## Known M2 limitations (deliberate, do not "fix")

- No background colors, no images, no text selection. M1's limits, unchanged.
- One page. `:open` navigates the page you are on; there are no tabs until M4.
- No clicking, no typing into the page, no hints. M3.
- `g` and `G` are the only non-native scrolling, for the reason given in Task 8.
- No search-engine fallback: `:open how tall is everest` is an error, not a search.
- No accessibility-tree or `DOMSnapshot` fallback if the injected script throws. See the open question in the M2 spec, section 12.
- The Chromium process is not supervised. If it dies, the binary exits. M7.
