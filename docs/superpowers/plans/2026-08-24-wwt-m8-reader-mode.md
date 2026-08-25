# wwt M8 — Reader Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a per-tab reader view that selects the dominant readable subtree, reflows it to terminal cells, scrolls locally, follows links, and returns to the untouched page at the same scroll offset.

**Architecture:** The page produces semantic data with no geometry. A new pure `wwt-reader` crate turns that data into terminal rows, source anchors and link ranges. `Session` owns which per-tab view is active and interprets inputs locally while it is; `Core` adds one page read and no decisions. Tasks 1 through 3 build the pure half, task 4 makes hints geometry-neutral, task 5 builds the browser query, and tasks 6 through 10 join them at the event/effect seam. Task 11 measures, documents and drives the feature.

**Tech Stack:** Rust 2024, the existing hand-rolled CDP client and injected JavaScript, no new crates.io dependencies. `wwt-reader` is a new workspace crate depending only on `wwt-frame`.

**Spec:** `docs/superpowers/specs/2026-08-24-wwt-m8-design.md`. It is a draft and must be approved before Task 1 begins. The parent design is `docs/superpowers/specs/2026-08-19-wwt-design.md`; where the two disagree the parent wins, except for the amendments in M8 section 10, applied in task 11.

## Global Constraints

- **No new crates.io dependency.** `wwt-reader` is a workspace member. If wrapping or selection seems to need another crate, stop and ask.
- **`wwt-reader` is pure.** It depends on `wwt-frame` only, performs no I/O, and knows no CDP, page, terminal, session or serde type.
- **`wwt-frame` remains unchanged.** Reader geometry never enters `Viewport`; synthetic CSS coordinates are forbidden.
- **`wwt-ui` still depends on `wwt-frame` only.** It may accept cells and return indices; it may not learn reader documents, page targets or navigation.
- **The real page never scrolls in reader mode.** Local scrolling emits no page scroll, input or save.
- **Never blank the frame being viewed.** First-entry failure keeps the real page; refresh failure keeps the old reader layout; late answers never reverse a later key.
- **Ordinary paths stay ordinary.** Until `r` is pressed, extraction, compose, scrolling, hints, mouse, resize, tabs and pixel mode retain their behaviour and measurements.
- **Nothing blocks the loop.** The query is an effect and one job; reflow is pure over memory.
- **Nothing in a `select!` arm touches `self`.**
- **Unit tests in `src/` run without Chromium.** DOM tests go in `wwt-page/tests/` and share one browser per test binary.
- **Clippy is clean per task:** `cargo clippy --workspace --all-targets -- -D warnings`.
- **No em-dashes** in prose, comments or commit messages.
- Comments explain why. Test names are sentences describing the property.
- Commits use a conventional crate scope and end with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- Tick steps as they finish and include this plan in the task commit, or in one `docs(plan):` checkpoint after a completed group. Never tick work that only exists in the plan.

## Baseline

Before Task 1, approve the M8 spec and commit the spec and this plan as documentation. Begin implementation from a clean worktree, then run:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS, clean. Record the current test count.

## File structure

| File | Responsibility after M8 |
|---|---|
| `crates/wwt-reader/src/document.rs` | Semantic document, blocks, spans, links and typed ids |
| `crates/wwt-reader/src/layout.rs` | Wrapping, presentation, anchors, link ranges and painting |
| `crates/wwt-page/assets/reader.js` | Dominant-subtree selection and serialization |
| `crates/wwt-page/src/reader.rs` | Reader wire shape and `ReaderExtraction` |
| `crates/wwt-ui/src/hint.rs` | Labels cells and resolves an index |
| `crates/wwt-ui/src/chrome.rs` | `[reader]` and selected progress |
| `crates/wwt/src/tab.rs` | Per-tab reader cache, view, dirtiness and position |
| `crates/wwt/src/effect.rs` | `Effect::ReadReader` |
| `crates/wwt/src/event.rs` | `Job::Reader` |
| `crates/wwt/src/keymap.rs` | `ToggleReader` and view-neutral scroll amounts |
| `crates/wwt/src/session.rs` | Every reader rule |
| `crates/wwt/src/core.rs` | Turns the read effect into `Page::reader()` |

---

### Task 1: The semantic reader document

Build data with no layout, page or session so later tasks depend on names already real.

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/wwt-reader/Cargo.toml`
- Create: `crates/wwt-reader/src/lib.rs`
- Create: `crates/wwt-reader/src/document.rs`
- Test: `crates/wwt-reader/src/document.rs`

**Interfaces:**
- Consumes: `wwt-frame` as the crate's only dependency; task 1 does not use it yet.
- Produces: `LinkId`, `Document`, `Block`, `BlockKind`, `Span`, `Link`, and a builder that normalizes adjacent spans.

- [x] **Step 1: Add the member and crate**

Add `crates/wwt-reader` to workspace members. Its manifest depends only on `wwt-frame = { path = "../wwt-frame" }`. No serde.

- [x] **Step 2: Write failing tests**

Assert adjacent text with the same link becomes one span, different links retain a boundary, empty spans disappear, and finish refuses a span naming a missing link.

Run: `cargo test -p wwt-reader`
Expected: FAIL, missing types.

- [x] **Step 3: Implement the types**

Use the design's shapes:

```rust
pub struct LinkId(pub usize);
pub struct Document { pub blocks: Vec<Block>, pub links: Vec<Link> }
pub struct Block { pub kind: BlockKind, pub spans: Vec<Span> }
pub struct Span { pub text: String, pub link: Option<LinkId> }
pub struct Link { pub url: String, pub new_tab: bool }
```

`BlockKind` has heading level, paragraph, ordered/unordered list item with depth and ordinal, quote depth, and preformatted text. Derive `Debug, Clone, PartialEq, Eq`. The builder owns joining and link validation; it does not collapse whitespace because preformatted text must survive.

- [x] **Step 4: Export only public vocabulary**

Declare `document` and re-export its public types. Keep builder helpers private where callers do not need them.

- [x] **Step 5: Verify and commit**

```bash
cargo test -p wwt-reader
cargo clippy --workspace --all-targets -- -D warnings
```

Commit: `feat(reader): name the document before laying it out`

---

### Task 2: Paragraphs become terminal rows

Build minimum honest reflow: paragraphs and preformatted text, wrapping without elision, and painting a visible window.

**Files:**
- Create: `crates/wwt-reader/src/layout.rs`
- Modify: `crates/wwt-reader/src/lib.rs`
- Test: `crates/wwt-reader/src/layout.rs`

**Interfaces:**
- Consumes: task 1's semantic types and `wwt_frame::{CellPos, Frame, Style}`.
- Produces: `SourcePos`, `Layout::new`, `Layout::rows`, and `Layout::paint`.

- [x] **Step 1: Write failing wrapping tests**

Test wrapping at spaces, a word wider than the terminal, one blank row between paragraphs, preserved preformatted spaces/breaks, hard-wrapped preformatted lines, and a one-column terminal.

Run: `cargo test -p wwt-reader layout::`
Expected: FAIL, no layout.

- [x] **Step 2: Implement character-based wrapping**

Count Rust `char`s, matching `Frame::paint_text`. Prefer the last fitting space, omit the breaking space, split a too-long word, make progress at width one, never exceed width, and carry block/character source position per row. Never use byte indices as source offsets.

Until task 3 adds presentation, headings, lists and quotes use paragraph wrapping and preformatted keeps its own path. That temporary rendering is internal to the pure crate and no reader view can reach it yet.

- [x] **Step 3: Paint the visible window**

`Layout::paint(frame, top_row, origin_row, page_rows)` copies only visible rows through `Frame::paint_text`, never touches chrome rows and never constructs a second frame.

Add tests that painting begins at `top_row` and is clipped to the page area.

- [x] **Step 4: Verify and commit**

```bash
cargo test -p wwt-reader
cargo clippy --workspace --all-targets -- -D warnings
```

Commit: `feat(reader): reflow words into terminal rows`

---

### Task 3: Structure, anchors and links in the layout

Finish the pure crate before introducing Chromium.

**Files:**
- Modify: `crates/wwt-reader/src/layout.rs`, `crates/wwt-reader/src/lib.rs`
- Test: `crates/wwt-reader/src/layout.rs`

**Interfaces:**
- Produces `LinkRange`, `Layout::source_at`, `Layout::top_for`, `Layout::visible_links`, and `Layout::link_at`.

- [x] **Step 1: Write structural golden tests**

One document covers every block kind. Assert heading bold/rules and spacing, list markers and continuation alignment, capped indentation, repeated quote prefixes, preformatted wrapping, and bold links with default foreground/background.

- [x] **Step 2: Write source-anchor tests**

Build one document at two widths. Assert `source_at(old_top)` maps through `top_for` to the same source or the nearest row immediately before it, and positions past a shortened document clamp.

- [x] **Step 3: Write link-geometry tests**

Assert a link wrapping across three rows has three ranges and one visible hint, scrolling selects its first visible range, and hit-testing accepts only `start <= col < end` inside page rows.

- [x] **Step 4: Implement presentation and generated prefixes**

Generated list/quote prefixes consume width but do not consume source offsets. Heading rules inherit the heading's offset-zero source; blank separator rows inherit the following block's source, or the preceding one at the document end. Every row can therefore answer `source_at` without an `Option`. Keep row fragments private. Do not widen `Style`.

- [x] **Step 5: Build ranges while wrapping**

Do not search rendered strings afterward. Preserve document order, deduplicate hints by `LinkId`, and translate terminal rows with `origin_row` only at the public boundary.

- [x] **Step 6: Verify and commit**

```bash
cargo test -p wwt-reader
cargo clippy --workspace --all-targets -- -D warnings
```

Commit: `feat(reader): keep structure and destinations through reflow`

---

### Task 4: Hint mode labels cells and returns an index

Remove the assumption that every hint began as CSS geometry. This is behaviour-preserving for pages.

**Files:**
- Modify: `crates/wwt-ui/src/hint.rs`, `crates/wwt-ui/src/chrome.rs`, `crates/wwt/src/session.rs`
- Test: `crates/wwt-ui/src/hint.rs`, `crates/wwt/src/session.rs`

**Interfaces:**
- Produces `HintSession::new(Vec<CellPos>)`, `HintSession::paint(&mut Frame)`, and `Filtered::Activate(usize)`.

- [x] **Step 1: Rewrite hint tests first**

Use distinct `CellPos` values instead of `HintTarget`. Assert paint uses given cells without a viewport and filtering returns the original index.

Run: `cargo test -p wwt-ui hint::`
Expected: FAIL at old signatures.

- [x] **Step 2: Narrow `HintSession`**

Store cells and parallel labels. Return the selected index. No link, target kind or activation enters `wwt-ui`.

- [x] **Step 3: Preserve page activation in `Session`**

`enter_hints` converts `HintTarget::label_cell(&Viewport)` before creating the UI session and retains page targets on the tab. On activation, look up the selected target and call the existing centre-click path. A stale index leaves hint mode and does nothing rather than panic.

- [x] **Step 4: Verify no behaviour moved**

```bash
cargo test -p wwt-ui
cargo test -p wwt --lib
cargo clippy --workspace --all-targets -- -D warnings
```

Commit: `refactor(ui): a hint begins at a cell`

---

### Task 5: Chromium produces a semantic document

Build the only browser-dependent half. The on-demand query returns semantic data and `Status` in one round trip.

**Files:**
- Modify: `crates/wwt-page/Cargo.toml`, `crates/wwt-page/src/lib.rs`, `crates/wwt-page/src/extract.rs`
- Create: `crates/wwt-page/assets/reader.js`, `crates/wwt-page/src/reader.rs`
- Create: `crates/wwt-page/tests/reader.rs`
- Create: `crates/wwt-page/tests/fixtures/reader.html`, `reader-competing.html`, `reader-body.html`

**Interfaces:**
- Consumes: task 1's semantic types, existing `Page::js` and `Status`.
- Produces `ReaderExtraction { document, status }` and `Page::reader()`.

- [x] **Step 1: Add the dependency and module boundary**

Add `wwt-reader` to `wwt-page`. Keep raw serde structs private to `reader.rs`. Make `Page::js` `pub(crate)` only if the sibling module needs it; the public general evaluator remains test-support only.

- [x] **Step 2: Write browser tests against public data**

Using the shared harness, assert:

- an article beats a main that owns only surrounding text;
- a main can win on substantial text it owns;
- no landmark falls back to body;
- hidden/site furniture disappears while article headers remain;
- headings, lists, quotes, pre, tables and alt text retain order;
- URLs are absolute and `_blank` is recorded;
- empty and `javascript:` links are text, not targets;
- reader and ordinary extraction report equal status.

Run: `cargo test -p wwt-page --test reader`
Expected: FAIL, no method or fixtures.

- [x] **Step 3: Implement candidate selection in `reader.js`**

Use exactly spec section 3: `article`, `main`, `[role=main]`; score text owned outside nested candidates; non-link characters plus one quarter link characters; document-order tie; body fallback. Skip the named semantic furniture and hidden subtrees. Cache computed style per visited element during the query.

`reader.js` is an expression evaluated only by `Page::reader`. It is not installed on every page and adds no ordinary-path work.

- [x] **Step 4: Serialize once in document order**

Flush at block boundaries so no ancestor duplicates descendant text. Implement every mapping from spec section 4. Collapse ordinary inline whitespace across nodes, preserve pre and `br`, strip control characters, join table cells with ` | `, resolve `anchor.href`, and retain link identity through nested inline nodes.

Return status fields directly rather than call `window.__wwt.status()`, so reader does not depend on the normal extraction function being healthy.

- [x] **Step 5: Deserialize and validate**

Raw types derive serde in `wwt-page` and convert through the document builder. Invalid block names, heading levels or link indices return contextual errors. Refactor the existing raw-status conversion once; do not duplicate it.

No readable blocks returns an error whose visible message is `no readable content`.

- [x] **Step 6: Verify and commit**

```bash
cargo test -p wwt-page --test reader
cargo test -p wwt-page
cargo clippy --workspace --all-targets -- -D warnings
```

Commit: `feat(page): read meaning without asking for layout`

---

### Task 6: Reader state crosses the seam and is cached on a tab

Add one read and one cache before adding a key. Session tests drive it directly.

**Files:**
- Modify: `crates/wwt/Cargo.toml`
- Modify: `crates/wwt/src/effect.rs`, `event.rs`, `tab.rs`, `core.rs`, `session.rs`
- Test: `crates/wwt/src/tab.rs`, `crates/wwt/src/session.rs`

**Interfaces:**
- Produces `Effect::ReadReader(TabId)`, `Job::Reader(TabId, Result<Box<ReaderExtraction>, Failure>)`, `ReaderState`, and `Session::start_reader`.

- [x] **Step 1: Write tab-state tests**

A new tab has no document/layout, row zero, inactive/unwanted, and dirty reader data. Detach keeps cache, row and view flags; clears shared `reading`; marks both representations dirty.

- [x] **Step 2: Implement one `ReaderState` field**

Use one struct on `Tab` with optional document/layout, `top_row`, `active`, `wanted`, `dirty`. `Default` starts empty and dirty. `mark_dirty` marks page and reader data. `detach` preserves reader content and invalidates it for reattach.

- [x] **Step 3: Widen effect and job vocabularies**

Reader has no `Source`: it is a chosen semantic view, not normal extraction's fallback. Add it to job-id resolution and every exhaustive match.

- [x] **Step 4: Let `Core` answer without deciding**

`ReadReader` spawns `page.reader()`, boxes success, maps failure through `Failure::from_error`, and always returns `Job::Reader`. The existing five-second deadline applies.

- [x] **Step 5: Write session read tests**

Assert one request uses the shared read slot; success caches and lays out at current width; timeout stalls without dropping an old layout; first failure keeps page view; failure never sets `degraded`; a dirty signal during the read produces one follow-up.

- [x] **Step 6: Implement start and answer rules**

Emit only for an attached tab that is wanted/active, reader-dirty and not reading. Set `reading = true` and reader dirty false before emitting.

Every answer clears reading. Success builds current-width layout, replaces cache, clamps row, applies status, and activates only if `wanted` remains true. Failure labels the tab, keeps old active layout, clears first-entry `wanted`, and leaves `degraded` untouched. Re-run once if dirty became true in flight.

- [x] **Step 7: Verify and commit**

```bash
cargo test -p wwt --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Commit: `feat(wwt): cache a reader document on its tab`

---

### Task 7: `r` enters a reader frame and returns to the page

Make cached content visible. Scrolling and links remain for task 8.

**Files:**
- Modify: `crates/wwt/src/keymap.rs`, `crates/wwt/src/session.rs`, `crates/wwt-ui/src/chrome.rs`
- Test: the same files

**Interfaces:**
- Produces `Action::ToggleReader`, `Session::set_reader`, `Chrome::reader`.

- [x] **Step 1: Add the key test**

Bare `r` in normal mode is `ToggleReader`; `Ctrl-r` remains `Reload`; other modes do not gain the key.

- [x] **Step 2: Write entry/exit and late-answer tests**

Assert first `r` leaves real runs composed while the job is away; clean cache enters with no effect; second `r` cancels pending entry; late success caches but does not activate; exit immediately repaints cached runs and preserves `scroll_y`.

- [x] **Step 3: Implement entry and exit**

Entry sets wanted. Clean cache activates immediately; otherwise it leaves page active, shows `reading`, and calls `start_reader` when the shared read slot is free. Exit clears wanted/active and starts normal extraction only if page data is dirty. Never scroll or save.

- [x] **Step 4: Give compose explicit precedence**

Reader layout, else pixel picture, else page runs. Paint hints next and chrome last. A reader frame carries no image, deleting an old graphics placement.

- [x] **Step 5: Add chrome tag and selected progress**

`Chrome` takes named `reader: bool`. Reader suppresses `[pixel]`; `[degraded]` may remain. Session passes reader `top/max_top` while active, page progress otherwise. Add narrow-line and tag-order tests.

- [x] **Step 6: Stop/start screencast only on actual view change**

Successful entry from pixel stops focused screencast. Exit to pixel starts it. Merely asking for a first document keeps the picture running because it is the frame standing while the query is away.

- [x] **Step 7: Verify and commit**

```bash
cargo test -p wwt-ui
cargo test -p wwt --lib
cargo clippy --workspace --all-targets -- -D warnings
```

Commit: `feat(wwt): r opens a distinct reader view`

---

### Task 8: Reader scrolling, hints, links and mouse are local

Interpret the existing controls in reader geometry and send nothing to the hidden page.

**Files:**
- Modify: `crates/wwt/src/keymap.rs`, `crates/wwt/src/session.rs`
- Test: the same files

**Interfaces:**
- Produces a view-neutral `ScrollAmount` and reader activation by `LinkId`.

- [x] **Step 1: Refactor scroll actions without changing effects**

`Action::Scroll(f64)` currently bakes CSS pixels into the keymap. Replace the scroll variants with semantic amounts: lines, half-page, page, top, end. Session converts them to the exact existing CSS distances for page view and to rows for reader view.

Before changing implementation, pin every current normal key to its existing `Effect::Scroll` value. Run those tests after the refactor to prove normal behaviour is identical.

- [x] **Step 2: Write local-scroll tests**

Assert line, half-page, page, top/end and wheel clamp `top_row`; emit no effect and no save; update reader progress; and do not trigger M7 relaunch while browser is lost. Page view still emits the old effects.

- [x] **Step 3: Implement local scrolling**

One line is one layout row. Half-page and page use page rows with the existing two context rows. Wheel is three rows. Clamp through `max_top` and use saturating arithmetic.

- [x] **Step 4: Write reader-hint tests**

`f` takes only visible distinct links from `Layout`, emits no `Effect::Hints`, paints their cells through `HintSession`, and reports `no hints` when none are visible. Selecting same-tab leaves reader and emits existing navigation; `_blank` uses `open_tab` and leaves the source tab's reader state intact in the background.

- [x] **Step 5: Generalize hint activation ownership**

Session must remember whether the open `HintSession` indexes page targets or reader link ids. Keep this session-only, for example:

```rust
enum HintSource {
    Page,
    Reader(Vec<LinkId>),
}
```

Clear it whenever hint mode ends or the view/tab changes. `wwt-ui::Mode` still knows only the hint session.

- [x] **Step 6: Write and implement mouse tests**

Wheel scrolls locally. Left press on a visible `LinkRange` follows the destination. Press elsewhere does nothing. Releases are consumed. No reader mouse event converts through `Viewport` or emits `Effect::Send`.

- [x] **Step 7: Make browser-loss classification view-aware**

Replace pure `action_touches_the_page` with a session-aware question. Reader scroll and local hints work with no browser; following a destination, entering insert, or leaving for navigation still asks M7 to relaunch.

- [x] **Step 8: Verify and commit**

```bash
cargo test -p wwt --lib keymap::
cargo test -p wwt --lib session::
cargo clippy --workspace --all-targets -- -D warnings
```

Commit: `feat(wwt): read and follow links without moving the page`

---

### Task 9: Dirtiness and navigation choose the current representation

Make reader mode event-driven and define every way the document becomes stale or is left.

**Files:**
- Modify: `crates/wwt/src/tab.rs`, `crates/wwt/src/session.rs`
- Test: `crates/wwt/src/session.rs`

- [ ] **Step 1: Write dirty-refresh tests**

Assert a dirty signal in active reader mode:

- leaves old layout composed;
- marks ordinary runs and reader data dirty;
- requests reader, not normal extraction or status;
- coalesces repeated signals while reading;
- keeps numeric top row and clamps it after replacement.

Also assert dirty signals on an inactive cached reader ask only for the ordinary representation and leave reader dirty until the next `r`.

- [ ] **Step 2: Introduce one current-read dispatcher**

Keep `start_extract` and `start_reader` as narrow helpers, but route dirty signals and completed reads through `start_current_read(id, effects)`. It chooses reader only when wanted/active; otherwise it preserves pixel/status/degraded logic exactly.

Every `Job::Extracted`, `Status`, `Reader`, `Settled`, `Resized` and successful `Opened` path must end by asking the current representation, not by hard-coding normal extraction.

- [ ] **Step 3: Write navigation and insert tests**

Assert:

- `H`, `L`, `Ctrl-r`, `:open`, `:back`, `:forward`, `:reload` leave reader before emitting navigation;
- same-tab reader links follow the same path;
- navigation clears document, layout and top row;
- `i` leaves reader and enters insert on the real page without clearing the reusable reader cache;
- `p` leaves reader and selects pixel mode without clearing the cache;
- a `_blank` reader link leaves the source tab's reader cache/view untouched while the new normal tab is focused.

- [ ] **Step 4: Centralize invalidation**

Add a tab method for document replacement/navigation that clears reader cache, flags and top row in one place. Do not reuse `detach`: detach deliberately keeps the visible reader document.

When leaving reader for insert or pixel without navigation, clear only active/wanted and keep the clean cache for instant re-entry.

- [ ] **Step 5: Handle no-content and refresh failures**

First no-content/failure clears wanted and keeps real page. Refresh failure keeps active layout. Timeout sets `Stalled`; refusal sets an error message; neither sets degraded. A later dirty signal may try again, and no timer does.

- [ ] **Step 6: Verify and commit**

```bash
cargo test -p wwt --lib
cargo clippy --workspace --all-targets -- -D warnings
```

Commit: `feat(wwt): refresh whichever document is in front`

---

### Task 10: Tabs, resize, pixel mode and detachment preserve the view

Close the lifecycle windows where one tab's picture, layout or late answer could appear under another tab's chrome.

**Files:**
- Modify: `crates/wwt/src/tab.rs`, `crates/wwt/src/session.rs`
- Test: the same files

- [ ] **Step 1: Write tab-switch tests**

Build one reader tab and one normal tab. Assert switching each direction is a repaint; each reader row is retained; page extraction remains background-idle; reader hints are closed on switch; and no tab paints another's reader layout.

- [ ] **Step 2: Make screencast following view-aware**

The current `follow_focus` assumes global pixel implies every focused tab wants a screencast. Replace that assumption with `shows_pixel(tab)`:

- leaving a pixel page stops it;
- entering a reader tab starts none;
- leaving reader for a normal tab while pixel is on starts the new tab;
- switching onto an opening/detached reader tab does not emit a start that `Core` will drop;
- `Job::Opened` starts screencast only if the now-focused tab shows pixels.

Keep the previous-picture rule for pixel-to-pixel switches. Reader compose must never retain the previous picture behind its cells.

- [ ] **Step 3: Specify `p` completely**

In reader view, `p` leaves reader and sets global pixel on, regardless of its old value. In real-page view it keeps M5's toggle. Test entering reader from pixel, exiting with `r` back to pixel, and selecting pixel with `p` from reader entered out of text.

- [ ] **Step 4: Reflow every cache on resize**

For each tab with document/layout:

1. take `source_at(top_row)` before replacement;
2. build a layout at new columns;
3. set top through `top_for(source)` and clamp.

Do this for background tabs so their next switch is a repaint. Preserve all existing `SetViewport` effects for attached real pages and all image resizing rules. Reader adds no page effect of its own.

- [ ] **Step 5: Test detachment and reattachment**

Eviction and browser loss keep active layout, row and document; clear shared reading; mark both representations dirty. A reader tab remains locally scrollable with `browser_lost`. `BrowserBack`/`Job::Opened` refreshes reader rather than normal runs when reader is wanted/active. The frame stands throughout.

Eviction still skips any tab using the shared read slot. Otherwise a background reader tab is eligible like any other tab because `max_tabs` counts targets, not cached Rust data.

- [ ] **Step 6: Test late jobs across tabs and views**

Reader success for a background tab caches only there and never changes the visible view or mode. A job for a closed tab drops. A page extraction that began before `r` may fill cached runs but must hand the slot to the still-wanted reader query afterward.

- [ ] **Step 7: Run the complete unit suite and commit**

```bash
cargo test -p wwt --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Commit: `feat(wwt): let reader state follow the tab`

---

### Task 11: Measurements, end to end, notes and manual pass

Prove the feature against Chromium and a PTY, record its costs, apply the parent amendments, and make M8 discoverable.

**Files:**
- Modify: `crates/wwt-reader/src/layout.rs` (measurement)
- Modify: `crates/wwt-page/tests/reader.rs` (measurement)
- Modify: `crates/wwt/tests/smoke.rs` or create `crates/wwt/tests/reader.rs`
- Add any reader PTY fixture under `crates/wwt/tests/fixtures/`
- Modify: `docs/superpowers/specs/2026-08-19-wwt-design.md`
- Modify: `docs/superpowers/specs/2026-08-24-wwt-m8-design.md` only for facts learned during implementation
- Modify: `CONTEXT.md`, `CLAUDE.md`, `README.md`
- Modify: this plan, ticking completed boxes

- [ ] **Step 1: Measure extraction**

Add `measure_reader_extract` beside the reader browser tests. Warm the page, read `heavy.html` repeatedly, print steady-state duration and assert only that a non-empty valid document arrived. Record the observed release number in the final commit body and in the M8 spec's testing/cost section if it materially changes a design assumption.

Run:

```bash
cargo test -p wwt-page --test reader measure_reader_extract --release -- --nocapture
```

- [ ] **Step 2: Measure layout**

Build a semantic document large enough for several thousand rows and add `measure_reader_layout`. Reflow at representative narrow and wide widths, print both, and assert rows and source anchors remain valid rather than assert wall-clock budgets.

Run:

```bash
cargo test -p wwt-reader measure_reader_layout --release -- --nocapture
```

- [ ] **Step 3: Add the PTY flow**

Drive the real binary against a noisy fixture:

1. wait for ordinary content;
2. press `r` and assert `[reader]` plus article text without furniture;
3. scroll and assert the page changes locally;
4. press `f`, select a known link and assert the destination;
5. on a second run, scroll the real page first, enter and leave reader, and assert the original real-page rows and percentage return.

Use the existing PTY harness. Do not create a second terminal driver.

- [ ] **Step 4: Apply the five parent amendments**

Make M8 spec section 10's changes in `2026-08-19-wwt-design.md`:

- add `wwt-reader` to components;
- define reader hints as direct cell positions;
- say reader scroll is local and links are destinations;
- clarify reader is a per-tab view, not an input `Mode`;
- record in-memory lifecycle and no snapshot persistence;
- discharge the M8 milestone boundary.

- [ ] **Step 5: Update the glossary and working notes**

In `CONTEXT.md`, add:

- **Reader document:** semantic blocks/spans/links with no CSS geometry.
- **Reader layout:** width-specific rows, source anchors and link ranges.
- **Reader view:** per-tab, locally scrolled, cached in memory, leaving the page untouched.

In `CLAUDE.md`:

- change current milestone to M8;
- add `wwt-reader` to the crate table;
- add both measurement commands;
- add a Reader Mode section recording the page-stands-still rule, semantic-vs-layout boundary, source-anchor resize rule, local hint geometry, no snapshot persistence, and pixel precedence.

- [ ] **Step 6: Update README**

Add `r` to the key table and explain in user language: what content is chosen, that reader scroll is separate, `f` follows links, `i` returns to the live page, and a second `r` returns to the original position. Add the M8 spec and plan to Documentation and change status from M7 hardening to M8 reader mode.

- [ ] **Step 7: Manual pass in a real terminal**

Record surprises and fix what they expose:

1. A long news article with site header/nav/footer: title, byline and body remain; furniture does not.
2. Documentation with `main` but no article: all main content remains, headings/lists/code are readable.
3. A marketing page falling back to body: the result is imperfect but usable and `r` exits immediately.
4. Enter halfway down a page, scroll reader independently, exit: exact real position returns.
5. Resize narrow and wide in reader: the sentence at the top stays there.
6. Follow same-tab and `_blank` links by hint and mouse.
7. Enter from pixel, leave to pixel, switch between reader and normal tabs while pixel is on: no old image behind reader.
8. Evict a reader tab with low `max_tabs`, switch back: cached reader paints immediately and refreshes behind it.
9. Kill Chromium in reader: content scrolls locally, the browser restarts, and the reader refreshes without blanking.
10. A page with no readable text and a page whose main thread is wedged: real/cached frame stands with the correct label.

- [ ] **Step 8: Run every check and measurement**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p wwt-page --test extraction measure_extraction --release -- --nocapture
cargo test -p wwt-page --test interaction measure_hints --release -- --nocapture
cargo test -p wwt --lib measure_switch --release -- --nocapture
cargo test -p wwt-term --lib measure_pixel_frame --release -- --nocapture
cargo test -p wwt --lib measure_pixel_compose --release -- --nocapture
cargo test -p wwt --lib measure_halfblock_frame --release -- --nocapture
cargo test -p wwt-page --test snapshot measure_snapshot --release -- --nocapture
cargo test -p wwt-page --test extraction measure_status --release -- --nocapture
cargo test -p wwt-page --test reader measure_reader_extract --release -- --nocapture
cargo test -p wwt-reader measure_reader_layout --release -- --nocapture
```

Expected: all pass; M2 through M7 figures stay in their established order of magnitude. Investigate a moved ordinary-path number before calling M8 complete.

- [ ] **Step 9: Tick the plan and commit**

Commit implementation test changes with their owning crate if the PTY or measurement exposes code work. The final documentation commit is:

```text
docs: write down the view that leaves the page alone
```

Its body records the measured extraction/layout numbers, the manual pages used, and any design correction learned rather than silently changing this plan after the fact.

---

## Done when

- `r` shows the dominant readable content reflowed to terminal width, with `[reader]` visible.
- Reader scroll, page keys and wheel are local and emit no page scroll or save.
- A second `r` restores the real page at the exact scroll offset held on entry.
- `f` and the mouse follow reflowed links without fabricated CSS geometry.
- Resize preserves the source near the top and costs no reader-specific browser round trip.
- Reader state belongs to its tab, survives switches, eviction and Chromium replacement in memory, and is absent from `session.json`.
- Pixel mode and reader never paint simultaneously or leave an old image behind reader cells.
- First read failure keeps the real page; refresh failure keeps the old reader document; late answers respect later keys.
- Ordinary extraction, scrolling, hints, tabs, pixel mode and degradation retain their tests and measured costs.
- `cargo test --workspace` and clippy pass, the PTY flow passes, the manual pass is complete, and the parent spec, glossary, working notes and README describe what shipped.
