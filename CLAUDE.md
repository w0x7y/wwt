# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`wwt` (world wide terminal) is a terminal web browser: headless Chromium driven over
a hand-rolled CDP client, with its layout painted into the terminal cell grid.
Chromium does layout and JavaScript; this codebase never reimplements either.

Currently at **M3** (interaction). Milestones M1–M7 are defined in
`docs/superpowers/specs/2026-08-19-wwt-design.md` §11.

## Commands

    cargo run -p wwt -- example.com              # run it (needs a real terminal)
    cargo test --workspace                       # 222 tests; the integration ones launch Chromium
    cargo test -p wwt-frame                      # pure logic, no browser needed
    cargo test -p wwt-page --test extraction extracts_the_visible_text   # one test by name
    cargo clippy --workspace --all-targets -- -D warnings   # must be clean, per task, not per plan

    UPDATE_SNAPSHOTS=1 cargo test -p wwt-page --test extraction   # regenerate the ASCII snapshot
    cargo test -p wwt-page --test extraction measure_extraction -- --nocapture   # extraction latency
    cargo test -p wwt-page --test interaction measure_hints -- --nocapture       # hint query latency

`WWT_CHROMIUM` overrides browser discovery (otherwise: `chromium`,
`chromium-browser`, `google-chrome-stable` on `PATH`). Nothing is ever downloaded.

Unit tests in `src/` must run without Chromium; anything needing a browser goes in
`tests/`. Each test *binary* launches one Chromium and hands it out a test at a time
(`wwt-page/tests/common`), because `Input.dispatchMouseEvent` is answered by whichever
target the browser has in front. `wwt-cdp/tests/browser.rs` launches its own, since
launching is what it tests.

`CONTEXT.md` is the glossary: what a run, an extraction, a dirty signal, an effect and
a mirror are, and which type each one names.

## The coordinate model

This is the load-bearing decision (`wwt-frame/src/geom.rs`, spec §3). Chromium is told
the window is exactly `grid × cell_size` CSS pixels, so a normal desktop layout maps
one-to-one onto cells. Everything follows:

- **Every unit conversion goes through `Viewport`.** Nothing else divides by a cell
  dimension. `to_cell(to_css(c)) == c` for every cell at every zoom is a property test
  and must stay true.
- **Zoom is `CellSize`, not a CSS zoom.** A bigger declared cell shrinks the CSS
  viewport, so the page genuinely reflows and hits different breakpoints.
- **Runs snap to the row containing their baseline**, not their box top — otherwise
  mixed font sizes drift. `Frame::paint_run` also elides with `…` when a run's text
  exceeds the columns its box covers, and resolves contested cells by `z`
  (painter's algorithm; equal `z` means later wins).
- **The page viewport is one row shorter than the terminal** (`core::page_viewport`).
  Chrome owns the last row and the page does not know it exists. The composited
  `Frame` is still full-grid, so no clipping machinery is needed.

## Data flow

```
terminal keys ──┐                        ┌──> Effect ──> spawned page op ──CDP──> Chromium
CDP events ─────┼──> Core::run select! ──┤                                            │
job results ────┘      (decides nothing) └──> compose Frame ──> Renderer ──> stdout   │
resize timer ───┘              ▲                                                      │
                               └──────────── Job ─────────────────────────────────────┘
```

There is a seam across the middle. `Session` (`crates/wwt/src/session.rs`) owns all
state, is the only thing that mutates it, and reaches nothing: `on(Event) ->
Vec<Effect>` and `compose() -> Frame`, both pure enough to test with no browser and no
tty. `Core` (`crates/wwt/src/core.rs`) is the adapter that turns tokio into events and
effects into spawns, and decides nothing at all.

**Put new rules in `Session` and new machinery in `Core`.** A decision that needs a
browser to exercise is a decision nobody will test.

Consequences to preserve when adding features:

- **Nothing blocks the loop.** Page operations spawn and report back as a `Job` on one
  channel. A thirty-second load still leaves keys responsive. `Core::spawn` is the
  only place anything is spawned; each effect says what its failure means by choosing
  the `Job` it reports, or reporting none.
- **Nothing in a `select!` arm touches `self`.** An arm produces an `Event` and
  nothing else. Borrowing `self` in one while the other futures are alive is what used
  to force a whole spawned task to merge two channels into one.
- **Re-extraction is event-driven, never polled.** `bootstrap.js` calls the
  `__wwt_dirty` binding from a debounced `MutationObserver`, scroll listener,
  `load`, and the four field-state events (`input`, `selectionchange`, `focusin`,
  `focusout`); the core keeps at most one extraction in flight and re-runs if the
  flag is still set. An idle page must cost ~zero CPU — do not add a tick loop.
- **Never blank the frame you are looking at** (spec §8). Every failure path degrades
  to stale-but-labeled: the old frame stays, only `State` in the statusline changes.
- **Rendering is diffed.** `Renderer` holds the last presented frame; call
  `invalidate()` after a resize or anything else that writes to the terminal.
- Chromium absorbs DNS/connection failures by navigating to its own `chrome-error://`
  page and firing a normal load event, so navigation "succeeds" into an error page.
  `Core::on_job` detects the scheme to set `State::Error` — keep this in mind when
  touching navigation.

## Crates

| Crate | Responsibility | Hard rule |
|---|---|---|
| `wwt-frame` | Coordinate math, cells, `Frame`, painting | **No I/O, no dependencies.** Non-negotiable. |
| `wwt-cdp` | Chromium launch, websocket, call/response correlation, event broadcast | Hand-rolled on purpose; see spec §4. |
| `wwt-page` | One page: bootstrap script, navigate/scroll/history, `extract()` | `eval` is behind `test-support`. |
| `wwt-term` | `TIOCGWINSZ` probe, diffing renderer | |
| `wwt-ui` | Modes, chrome, `:` commands, hint labels | Depends on `wwt-frame` only. No pages, no CDP, no terminal. |
| `wwt` | Binary: the `Session` state machine, the core loop, keymap, key table, input pump | |

`Frame` is the single output type every rendering mode produces, so text mode, and
later pixel and reader modes, cannot diverge in how they reach the screen.

## The injected script

`crates/wwt-page/assets/bootstrap.js` is installed once per document via
`Page.addScriptToEvaluateOnNewDocument`, so it survives navigation. `extract()`
returns runs *plus* title, URL, and scroll geometry in one `Runtime.evaluate` round
trip — the statusline costs no extra call. Line splitting uses `getClientRects` plus a
binary search over character offsets (`O(lines · log chars)` forced layouts); this is
scroll latency, so keep it cheap.

Both of the script's searches are `firstWhere(lo, hi, past)`, with the DOM half passed
in: `splitLines(rects, text, topOf)` and `offsetPast` are the callers that supply one.
`window.__wwt.__pure` exposes that arithmetic — `firstWhere`, `splitLines`, `caretIn` —
and `tests/geometry.rs` asserts on it with data. **Anything sharp enough to be worth
getting right belongs on that side of the line**, where its test costs no page.

`Page::eval` is `#[cfg(feature = "test-support")]`, turned on by a dev-dependency on
the crate itself. Tests use it to *arrange* a fixture and to reach `__pure`; what they
assert on is what `Extraction` returns, so a change that leaves the DOM right and the
extraction wrong still fails.

## Input

Three rules carry M3:

- **`Esc` is never forwarded.** A page cannot trap the keyboard. `Ctrl-]`
  sends the page a literal Escape, because a terminal transmits `Ctrl-[` as
  `0x1B`, which *is* Escape.
- **Mode changes only in response to a keystroke.** No `focusin` listener, no
  page-driven mode. `i` hands the keyboard over, `Esc` takes it back, and
  hinting a text field enters insert because that was your keystroke.
- **Input is ordered.** Keys and clicks go through one pump task
  (`wwt/src/input.rs`), not one spawned task each, or `abc` would sometimes
  arrive as `acb`. Everything else about the loop is unchanged: nothing
  blocks it.

`keymap.rs` answers `(mode, key) -> Action` for *every* mode, so what a key does is
one table rather than four files. `Session` interprets actions; it never re-reads a
key. `keys.rs` maps a crossterm event to the quad `Input.dispatchKeyEvent` needs.
It lives in the binary because its output type belongs to `wwt-page` and its
input type to crossterm, so either other home would point a dependency edge
backwards. Ctrl and Meta suppress the inserted text, or a page's `Ctrl-S`
handler would fire *and* type an `s`.

`extract()` has a second pass over `input`, `textarea` and `select`. A control's
value is not in the DOM (`input.childNodes` is empty however much you type), so
the text walk cannot see it and you would not be able to see what you type. It
reports what the browser shows: a placeholder for an empty field, the chosen
option for a `select`, bullets for a password.

Wrapping, scroll position and the caret need character positions, and there is no
`Range` inside an `input`. The script mirrors the control into a hidden div with
its font and content width and measures that instead. **The mirror is a DOM
mutation and the dirty observer watches the document**, so the pass disconnects
the observer, measures, and re-observes after `takeRecords`: without that, every
extraction signals dirty and an idle page spins forever. A mirror costs a layout,
so only controls that need one get one (multiline, overflowing, scrolled, or
focused).

A control's value and its selection are element state, not DOM, so no mutation
accompanies typing or moving the insertion point: `input`, `selectionchange` and
the focus events are the dirty source for this pass, and without them the caret
sits still until something unrelated changes the page. The focus listener signals
a repaint and never a mode; the rule above still holds.

`Extraction::caret` is a line (x, baseline) plus a character offset into it,
never a pixel position: `paint_run` gives every character one cell, so a caret
placed by CSS x drifts left of the character it belongs beside. It becomes
`Frame::cursor`, and the renderer puts the terminal's own cursor there as a bar
rather than painting a cell. Set in insert mode only, because a page can focus a
field without being asked.

Hint targets come from `__wwt.hints()`, queried on `f` and cached until the
next dirty signal. They are deliberately not part of extraction: that path
runs on every scroll frame. Labels are of uniform length, which makes the set
prefix-free, so activation needs no timeout.

## Working in this repo

- Specs and plans live in `docs/superpowers/`. Read the relevant spec before changing
  behavior it describes; the parent design doc wins where a milestone doc disagrees.
  When implementation forces a deviation, amend the spec in the same commit.
- **Do not add dependencies.** The set is fixed in `Cargo.toml` workspace deps; if a
  task seems to need a new crate, stop and ask.
- Comments explain *why*, in prose, where the reason is not obvious. Don't restate code.
- Test names are sentences describing the property (`cell_css_cell_roundtrip_is_identity`).
- Commits are conventional with a crate scope: `feat(page):`, `perf(page):`,
  `refactor(cdp):`. Behavior discovered during implementation goes in the body.
- No em-dashs
