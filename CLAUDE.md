# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`wwt` (world wide terminal) is a terminal web browser: headless Chromium driven over
a hand-rolled CDP client, with its layout painted into the terminal cell grid.
Chromium does layout and JavaScript; this codebase never reimplements either.

The goal is to be a first alternative to qutebrowser rather than a text-mode
curiosity, so **latency is a feature, not a finishing touch**. Read the performance
section below before touching the extraction path, which is what a scroll costs.

Currently at **M4** (tabs and sessions). Milestones M1–M7 are defined in
`docs/superpowers/specs/2026-08-19-wwt-design.md` §11.

## Commands

    cargo run -p wwt -- example.com              # run it (needs a real terminal)
    cargo test --workspace                       # 307 tests; the integration ones launch Chromium
    cargo test -p wwt-frame                      # pure logic, no browser needed
    cargo test -p wwt-page --test extraction extracts_the_visible_text   # one test by name
    cargo clippy --workspace --all-targets -- -D warnings   # must be clean, per task, not per plan

    UPDATE_SNAPSHOTS=1 cargo test -p wwt-page --test extraction   # regenerate the ASCII snapshot
    cargo test -p wwt-page --test extraction measure_extraction -- --nocapture   # extraction latency
    cargo test -p wwt-page --test interaction measure_hints -- --nocapture       # hint query latency
    cargo test -p wwt --lib measure_switch -- --nocapture                        # tab switch latency

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

The vocabulary the two share is its own: `event.rs` holds `Event` and `Job`, what
arrives; `effect.rs` holds `Effect`, `Scroll` and `Navigation`, what is asked for.
Both sides name them and neither owns them, so `Core` does not import the state
machine to describe a spawn.

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

A frame has one cursor and two modes want it, so **`Session::compose` is the
only caller of `set_cursor`**: the page's caret in insert, `chrome::command_caret`
in command, nothing in normal or hint. The chrome says where its caret would go
and never places it, or the two would be exclusive only by paint order.

Hint targets come from `__wwt.hints()`, queried on `f` and cached until the
next dirty signal. They are deliberately not part of extraction: that path
runs on every scroll frame. Labels are of uniform length, which makes the set
prefix-free, so activation needs no timeout.

It is the one effect whose *answer* changes the mode, so the session knows
while it is in flight: a second `f` asks nothing, and an answer opens hint mode
only if the mode is still normal. A round trip is long enough to have typed
half a `:` command, and labels must not land on top of it. `Job::Hints` carries
a `Result` rather than splitting into two variants, so there is one place that
can forget to note the query is over.

**An in-flight flag must not outlive the effect it was set beside.** `Core` drops
any effect naming a page it does not hold, which is every effect between asking for
a tab and being told it opened, and `Job::Hints` is the only thing that clears
`hinting`. So `f` is not asked at all on a tab that has not opened: setting the flag
for a query nobody can answer left `f` dead on that tab for the rest of the run.
`Tab::opened` names that window, and it is the question to ask before setting any of
the three flags beside an effect. `navigating` and `extracting` are safe today by
accident rather than by rule: `open_tab` already sets `navigating`, and an extraction
is only ever asked for by a dirty signal, which a page has to exist to send.

## Tabs and sessions

`Session` holds `Vec<Tab>` and a focus index; `Tab` (`wwt/src/tab.rs`) holds
everything true of one page rather than of the browser. `Core` holds
`HashMap<TabId, Arc<Page>>`. Four rules carry M4:

- **A `TabId` is a counter and never a position.** Effects name a tab and jobs
  name it back, and a job whose tab is gone is dropped. Close a tab while its
  extraction is in flight and every later tab shifts down one; an index would
  let the answer land on a page that never asked.
- **A background tab keeps its runs.** A switch paints from the cache and only
  then re-extracts, so it is a repaint rather than a round trip.
  `measure_switch` holds that down: tens of microseconds for three tabs of 300
  runs, against a ~4ms extraction, which is the whole point. It
  also keeps its dirty flag rather than spending it: a tab is read once when it
  opens and thereafter only while focused, so an idle background tab costs what
  an idle foreground tab costs.
- **Switching activates.** `Input.dispatchMouseEvent` is answered by whichever
  target the browser has in front. With one target that was a test-harness
  quirk; with several it is a correctness rule, and M5's screencast will want
  the same guarantee.
- **A `Page` never reaches `Session`.** The loop's result channel carries a
  `Finished`, which is either a `Job` on its way through or a target that
  finished opening; the page is filed in `Core` and the session hears only that
  the tab opened.

The chrome is two rows, the tab bar on top and the statusline at the bottom,
both unconditional so opening a tab never reflows a page. The page therefore
does not start at frame row 0, and that shift lives in `Viewport` as an origin
row rather than as a `+1` in `paint_run`, `Caret::cell` and `page_cell`.
`to_cell(to_css(c)) == c` now holds at every origin too.

**The profile is the lock.** Chromium refuses a `--user-data-dir` another
Chromium holds, so a second `wwt` falls back to a temporary profile, says
`private session`, and writes no session file. The instance holding the profile
owns that file: one rule for both resources and no lock file of ours to go
stale after a crash.

**Deciding to save is a rule, writing is machinery.** `Session` emits
`Effect::Save` when the tab set, the focus, or a page's URL, title or scroll
offset changes; `Core` coalesces on a timer and writes temp-then-rename. An
extraction of a page that did not move is not a write, and whether it moved is
decided against what the tab stores rather than against what the extraction
carried: an error page's URL is deliberately not kept, so comparing with the
extraction would write on every dirty signal.

**Adoption catches up; it does not hold.** A target a page opened is reported
by auto-attach and told apart by `openerId`, which `Target.createTarget` does
not set. `waitForDebuggerOnStart` looks like the way to get the bootstrap into
the document such a tab loads and is not: a held target answers
`Target.getTargetInfo` and `Runtime.runIfWaitingForDebugger` and nothing else,
so no setup call can be awaited, and a registration queued ahead of the release
still misses the document. `Page::adopt` registers the bootstrap for the next
document and evaluates it into the one already there; the script returns early
when it finds itself installed.

**A tab is reached by its position, not by cycling to it.** Shift and a digit focuses
the first tab through the ninth.

**The digit is what carries that across layouts, and the glyph is muscle memory on top
of it.** What shift and a digit prints belongs to the keyboard layout, so `keymap.rs`
takes the digit with `SHIFT` or without and asks nothing of the terminal. Nearly every
layout has digits on the unshifted number row, so the plain digit is that key; the ones
that do not, French among them, are exactly the ones where shift and that key is how a
digit is typed at all. The glyph table is US muscle memory plus the foreign glyphs that
collide with none of it, and a collision is resolved by leaving the glyph out rather
than guessing: `&` is a US shift-7 and a German shift-6, `"` is a German shift-2 and a
US shift-apostrophe. Nothing is lost by leaving one out, because every layout that
prints it has the digit.

**Do not enable Kitty's keyboard protocol to read the number row.** It looks like the
principled fix and is a regression twice over. `REPORT_ALTERNATE_KEYS` reports the
PC-101 key beside the layout's own, which would be layout independence outright, but
crossterm 0.29 (`parse.rs`) discards it: given `SHIFT` it takes the *shifted* codepoint,
overwrites the keycode and clears the modifier, which is the layout-dependent glyph
again. And `DISAMBIGUATE_ESCAPE_CODES` alone reports the unshifted key, so shift and `h`
arrives as `Char('h')` with `SHIFT` rather than as `Char('H')`: `H`, `L` and `G` stop
working, and insert mode types a lowercase letter and the wrong punctuation, since the
glyph a shifted key prints is what the flag stops telling us. Typing is worth more than
a keystroke to a tab. `supports_keyboard_enhancement` also takes the terminal's stdin
for up to two seconds to ask.

Binding a bare digit spends the count prefix a vim-like would put there. Reaching a tab
on every layout is worth more than a count no command takes yet; `/` is kept unbound for
find-in-page for the same reason, though it is European shift-7.

**A page is not told nobody is looking.** `Page::prepare` overrides the user agent
with the browser's own, headless marker removed. Search engines read `HeadlessChrome`
as a crawler: with it, duckduckgo.com returns a shell with no results and its html and
lite endpoints return a CAPTCHA. This is why `wwt-cdp` has a `user_agent` at all.
Google is unaffected by it and blocks on the requesting address instead.

**Anything that is not a URL is a search.** `normalize_url` sends a word with a dot in
it, or a host and a port, to `https://`, and everything else to DuckDuckGo. It is the
one place that decides, so `:open`, `:tabopen` and the command line argument all agree.

**Restoring a tab is opening it, not opening then scrolling.**
`Effect::OpenTab` carries the offset. As two effects they are two spawned
tasks, and an extraction that wins that race reads offset zero and writes it
down, losing the position being restored.

Eviction of background targets past a limit, and the lazy restore that shares
its machinery, are deferred to M7. They introduce the one state this design
does not have, a tab that exists without a target.

## Performance

The goal above is a latency goal, and the numbers that back it are in the tests
rather than in anybody's head:

    cargo test -p wwt-page --test extraction measure_extraction -- --nocapture
    cargo test -p wwt-page --test interaction measure_hints -- --nocapture

    cargo test -p wwt-page --test interaction measure_scroll_latency -- --nocapture

`heavy.html` is fifteen hundred paragraphs of which a dozen are on screen. Extracting
it costs ~4ms, and cost 18ms until the walk learned to stop measuring what nobody can
see. A scroll keystroke reaches new text in ~5ms, and took 36ms. The rules that keep
them there:

- **Ask the cheap question first.** Every pass over the document orders its tests by
  what they cost: a string test, then a tag name, then one layout read, and only then
  `getComputedStyle` and the character measuring. The text walk, `fieldRuns` and
  `hints` all do this, and all three would be equally correct in any order, only
  slower.
- **Cull a node before splitting it.** `reachesViewport` answers from the line boxes
  alone. Splitting a node costs a binary search per line it has, so splitting one
  nobody can see is the whole difference between 4ms and 18ms. The question is asked
  per *node* and never per line: a text node taller than the viewport has its visible
  lines in the middle of it, and
  `a_text_node_taller_than_the_viewport_keeps_the_lines_on_screen` is what holds that
  down.
- **The mirror measures everything.** `measureField` passes its own boxes and no
  viewport, because its lines are laid out off screen on purpose.
- **Nothing repaints without an event.** `Core::run` composes only when a `select!`
  arm produced an `Event`. An arm that produced none left the session untouched, so
  the frame would be identical; without this a page chattering on the console costs a
  full repaint per line it logs.
- **Output is buffered.** `stdout()` is a `LineWriter`, so the `\r\n` a full repaint
  puts between rows is a write syscall per row. `main` wraps it, which is forty
  syscalls a frame down to three.
- **Scroll leads, mutations trail.** `throttle` signals on the first scroll event and
  rate-limits what follows; `debounce` waits out the burst. A keypress produces
  exactly one scroll event, so trailing it coalesced nothing and cost 16ms. Mutations
  are genuinely bursty and still trail. Do not unify these two.
- **The frame rate is uncapped.** `--disable-frame-rate-limit`, because headless
  otherwise paces frames at the display's rate and a scroll is not visible to the
  page until the frame it lands on. It was two thirds of the scroll latency. An idle
  page produces no frames, so this costs nothing at idle, and that was measured. It
  is worth re-measuring when M5 puts `Page.startScreencast` on the same viewport,
  since an uncapped compositor is free only while nobody is asking it for frames.
- **Nothing deep-copies a payload.** An extraction is every run on screen;
  `Client::send` and `Page::js` take their `Value` rather than clone it, or the whole
  of it is copied twice on the way to the caller.

The frame pipeline is not where the time goes and is not worth tuning: composing 300
runs into a 120x40 grid and diffing it against the last one is ~40µs against a ~4ms
extraction. Spend the effort on the page side.

Neither the frame cap nor the scroll window shows up if you change one and measure:
each hides the other, and changing only one moves the median not at all. Measure the
grid, not the diagonal.

Two things that look like easy wins and are not. Disabling images
(`--blink-settings=imagesEnabled=false`) would save every decode, but pixel mode is
`Page.startScreencast` over this same viewport (spec section 3), so it would cost M5
its reason to exist. And headless does not throttle our timers: the page reports
`visibilityState: "visible"` and a 16ms `setTimeout` fires at 16.1ms, so the usual
`--disable-background-timer-throttling` family of flags buys nothing here.

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
