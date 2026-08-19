# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`wwt` (world wide terminal) is a terminal web browser: headless Chromium driven over
a hand-rolled CDP client, with its layout painted into the terminal cell grid.
Chromium does layout and JavaScript; this codebase never reimplements either.

Currently at **M2** (navigation and reading). Milestones M1–M7 are defined in
`docs/superpowers/specs/2026-08-19-wwt-design.md` §11.

## Commands

    cargo run -p wwt -- example.com              # run it (needs a real terminal)
    cargo test --workspace                       # 102 tests; the integration ones launch Chromium
    cargo test -p wwt-frame                      # pure logic, no browser needed
    cargo test -p wwt-page --test extraction extracts_the_visible_text   # one test by name
    cargo clippy --workspace --all-targets -- -D warnings   # must be clean, per task, not per plan

    UPDATE_SNAPSHOTS=1 cargo test -p wwt-page --test extraction   # regenerate the ASCII snapshot
    cargo test -p wwt-page --test extraction measure_extraction -- --nocapture   # extraction latency

`WWT_CHROMIUM` overrides browser discovery (otherwise: `chromium`,
`chromium-browser`, `google-chrome-stable` on `PATH`). Nothing is ever downloaded.

Unit tests in `src/` must run without Chromium; anything needing a browser goes in
`tests/`. Each `tests/` test launches its own Chromium instance.

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
terminal keys ──┐
CDP events ─────┼──> Core::run select! ──> spawned page ops ──CDP──> Chromium
job results ────┘         │                       │
resize timer ───┘         └──> compose Frame ──> Renderer (diff) ──> stdout
```

`Core` (`crates/wwt/src/core.rs`) owns all state and is the only thing that
mutates it. Consequences to preserve when adding features:

- **Nothing blocks the loop.** Page operations spawn and report back as a `Job` on one
  channel. A thirty-second load still leaves keys responsive.
- **Re-extraction is event-driven, never polled.** `bootstrap.js` calls the
  `__wwt_dirty` binding from a debounced `MutationObserver`, scroll listener, and
  `load`; the core keeps at most one extraction in flight and re-runs if the flag is
  still set. An idle page must cost ~zero CPU — do not add a tick loop.
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
| `wwt-page` | One page: bootstrap script, navigate/scroll/history, `extract()` | |
| `wwt-term` | `TIOCGWINSZ` probe, diffing renderer | |
| `wwt` | Binary: core loop, keymap, `:` commands, chrome | `wwt-ui` is deferred to M3. |

`Frame` is the single output type every rendering mode produces, so text mode, and
later pixel and reader modes, cannot diverge in how they reach the screen.

## The injected script

`crates/wwt-page/assets/bootstrap.js` is installed once per document via
`Page.addScriptToEvaluateOnNewDocument`, so it survives navigation. `extract()`
returns runs *plus* title, URL, and scroll geometry in one `Runtime.evaluate` round
trip — the statusline costs no extra call. Line splitting uses `getClientRects` plus a
binary search over character offsets (`O(lines · log chars)` forced layouts); this is
scroll latency, so keep it cheap.

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
