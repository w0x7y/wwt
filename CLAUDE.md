# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`wwt` (world wide terminal) is a terminal web browser: headless Chromium driven over
a hand-rolled CDP client, with its layout painted into the terminal cell grid.
Chromium does layout and JavaScript; this codebase never reimplements either.

The goal is to be a first alternative to qutebrowser rather than a text-mode
curiosity, so **latency is a feature, not a finishing touch**. Read the performance
section below before touching the extraction path, which is what a scroll costs.

Currently at **M7** (hardening). Milestones M1–M8 are defined in
`docs/superpowers/specs/2026-08-19-wwt-design.md` §11.

## Commands

    cargo run -p wwt -- example.com              # run it (needs a real terminal)
    cargo test --workspace                       # 509 tests; the integration ones launch Chromium
    cargo test -p wwt-frame                      # pure logic, no browser needed
    cargo test -p wwt-page --test extraction extracts_the_visible_text   # one test by name
    cargo clippy --workspace --all-targets -- -D warnings   # must be clean, per task, not per plan

    UPDATE_SNAPSHOTS=1 cargo test -p wwt-page --test extraction   # regenerate the ASCII snapshot
    cargo test -p wwt-page --test extraction measure_extraction -- --nocapture   # extraction latency
    cargo test -p wwt-page --test interaction measure_hints -- --nocapture       # hint query latency
    cargo test -p wwt --lib measure_switch -- --nocapture                        # tab switch latency, attached and detached
    cargo test -p wwt-term --lib measure_pixel_frame -- --nocapture              # what a picture costs
    cargo test -p wwt --lib measure_pixel_compose -- --nocapture                 # and what composing one costs
    cargo test -p wwt --lib measure_halfblock_frame -- --nocapture               # a degraded picture
    cargo test -p wwt-page --test snapshot measure_snapshot -- --nocapture       # a degraded read
    cargo test -p wwt-page --test extraction measure_status -- --nocapture       # what the chrome alone costs
    cargo test -p wwt --test supervisor -- --nocapture                           # a browser killed and replaced

`WWT_CHROMIUM` overrides browser discovery (otherwise: `chromium`,
`chromium-browser`, `google-chrome-stable` on `PATH`). Nothing is ever downloaded.

`$XDG_CONFIG_HOME/wwt/config.toml` is the user's, and has three keys:
`max_tabs` (live targets, the focused one included, default 8), `search` (a URL
with `{}` where the query goes) and `chromium` (a path, which `WWT_CHROMIUM`
still beats, because a variable is set for one run and a file for all of them).
A missing file is the normal case and says nothing; anything wrong with one is a
statusline notice and the default, because a browser that will not start because
of a typo is worse than one that starts and tells you.

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
| `wwt-png` | Base64, inflate, the PNG container, unfilter | **Decodes what Chromium sends and refuses the rest.** No dependencies, and it exists so that none is added. |
| `wwt-cdp` | Chromium launch, websocket, call/response correlation, event broadcast | Hand-rolled on purpose; see spec §4. |
| `wwt-page` | One page: bootstrap script, navigate/scroll/history, `extract()` | `eval` is behind `test-support`. |
| `wwt-term` | `TIOCGWINSZ` probe, diffing renderer | |
| `wwt-ui` | Modes, chrome, `:` commands, hint labels | Depends on `wwt-frame` only. No pages, no CDP, no terminal. |
| `wwt` | Binary: the `Session` state machine, the core loop, keymap, key table, input pump, `config.rs` | |

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
the three flags beside an effect. `navigating` and `reading` are safe today by
accident rather than by rule: `open_tab` already sets `navigating`, and a read of
either kind is only ever asked for by a dirty signal, which a page has to exist to
send.

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

**A tab is reached by its position, not by cycling to it.** Alt and a digit focuses the
first tab through the ninth. **The bare number row does nothing**, on purpose: the digits
are kept for the count prefix a vim-like puts on them, and `!` through `(` for whatever
wants them next.

**Alt is the modifier because it is the one a terminal reports.** It is sent as an
Escape and then the key, so the digit arrives intact with `ALT` beside it whatever the
keyboard is. Shift is not: shift and `1` arrives as the byte `!` and nothing else, since
crossterm sets `SHIFT` only for uppercase letters, so a shift binding could only ever be
a table of glyphs. That table existed and is gone.

**Only the digit, never the glyph over it.** On a layout whose number row is punctuation,
French among them, shift and that key is how a digit is typed at all, so alt and that
digit is still one keystroke there. Taking the glyph as well would be worse than useless:
`&` is the French `1` and the US `7`, and one of the two would land on a tab nobody asked
for. Nothing else is bound under alt, which returns early the way control does, or a
mistyped shortcut would scroll the page.

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

`/` is kept unbound for find-in-page, though it is a European shift-7.

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

## Pixel mode

`p` swaps the page between its runs and a picture of itself. The viewport,
the scroll offset and the focus are untouched, which is what makes the toggle
instant: the same state composes both ways.

**The bytes never leave base64.** `Page.screencastFrame` carries base64 PNG
and the Kitty graphics protocol wants base64 PNG, so a frame is forwarded
from the websocket to stdout as the string it arrived as. This is why M5 adds
no dependency, and it is why half-block degradation is M6's: half a cell
needs a foreground and a background colour, which needs real samples, which
needs an inflate and an unfilter that nothing else here would ever use.

**The grid wins over the image.** Unicode placeholders make placement cell
content, so a cell holding a glyph shows the glyph and a cell holding a
placeholder shows the picture. Hint labels over a pixel page therefore cost
nothing to arrange. Whether a cell is one or the other is decided as it is
written, not in a pass of its own, which is what makes a label arriving and a
label going away the same event: a cell that changed, which the diff already
writes.

**Every placeholder cell spends its own two diacritics.** A cell without them
continues from the cell before it, so an overlay in the middle of a row
orphans every placeholder after it and the picture tears from there to the
right edge. Surviving an overlay is the requirement, not an optimisation to
trade against, since overlays are why this design uses placeholders at all.

**Two image ids, alternating.** Transmitting to an id tears down its
placement for as long as the transmission lasts, and a full-page PNG is
dozens of chunks, so aimed at the id on screen that window is a visible
flicker. A frame goes into the id that is *not* on screen, is placed, has the
cells pointed at it, and only then is the other deleted. A test image in one
sequence shows none of this; it took a real page to find.

**A frame therefore rewrites the cells, and that is the cheaper half.**
`measure_pixel_frame`: ~39KB of cells against ~410KB of payload, ~410µs to
write. `Frame` carries the image and `Cell` does not, because a cell holding
combining diacritics would put a terminal protocol inside the one crate whose
hard rule is that it knows about nothing.

**A payload is shared, never copied.** `Image::payload` is an `Arc<String>`
because a frame is cloned twice on the way to the terminal, once by `compose`
and once by the renderer keeping it to diff against, and a page's worth of
base64 is a few hundred kilobytes. This is the rule under Performance about
never deep-copying a payload; the image is the biggest one there is.

**A row of cells is assembled and written once.** A placeholder is three
codepoints and a row is a hundred cells, so writing them as they are produced
is thousands of writes a frame, each a capacity check to copy two bytes. The
per-cell paths build into a `String`, which is why they return no `io::Result`
at all. Together with the shared payload this took a pixel frame from ~1.5ms
to ~0.45ms.

**The ack is the frame rate.** Chromium sends the next picture only once the
last is answered, so holding the ack back for `FRAME_INTERVAL` paces the
stream with the protocol's own flow control: nothing polls and nothing is
buffered. It has to be paced at all because `--disable-frame-rate-limit`
means an animating page paints as fast as the compositor can, and every one
of those is a full-page PNG for the terminal to decode. A still page produces
no frames and pays nothing.

**Every frame is acked, including the ones dropped**: one for a background
tab, one that was in flight when pixel mode was left. Chromium counts acks
and not paints, so a frame dropped without one stops the screencast and shows
up later as a picture that never moves. A frame for a *closed* tab is the
exception, since `Core` drops effects naming a page it does not hold.

**A switch in pixel mode is a round trip.** M4's repaint guarantee, and
`measure_switch`, are text mode's. The previous picture stays up under the
new tab's chrome until the first frame arrives, because the alternative is
blanking the frame you are looking at.

**The picture follows the focus from four places, and one of them cannot start
it.** `focus_tab` and `close_tab` call `follow_focus`, which stops the old target
and starts the new one. `open_tab` and `adopt_tab` cannot: `Core` drops any effect
naming a page it does not hold, which is every effect between asking for a tab and
being told it opened, so a start emitted there is a start nobody hears. They call
`leave_for_a_new_tab`, which stops the tab being left, and `Job::Opened` does the
start. Miss either half and pixel mode looks frozen on a new tab: the tab you left
goes on sending frames that `on_frame` acks and discards for not naming the tab in
front, so the last picture it sent stays on screen until something else starts a
screencast. This is the same window `Tab::opened` names for the in-flight flags,
and the screencast is the fourth thing with that shape.

**Pixel mode is global and is not saved.** Only the focused tab screencasts
either way, so per-tab would buy a preference rather than a cost, and a new
field in `Snapshot` is a version bump that costs every existing session file
its tabs on upgrade.

**A dirty signal in pixel mode asks only what the chrome needs.** The picture
comes from the screencast, so the runs an extraction produces are thrown away by
`compose`; `Effect::ReadStatus` gets the title, the URL and the scroll geometry
without the walk. Three exceptions, each for a reason: a background tab is read
in full because only what is in front is a picture, a tab nobody has read yet is
read in full because a status carries neither runs nor a real title, and a
degraded tab asks the snapshot because `status()` is the same injected script
that already threw. A failed status read degrades the tab exactly as a failed
extraction does, and `Job::Status` joins `Job::Extracted` as the second and last
thing that clears `reading` — the flag is named for the question rather than for
one of its two answers.

**Leaving pixel mode marks every tab dirty, not just the one in front.** Nobody's
runs were being maintained while the picture was up. A switch spends a dirty flag
and never sets one, so marking only the focused tab left a tab you had visited in
pixel mode painting stale runs on the switch back. A background tab takes the flag
and pays nothing until you reach it, which is M4's idling rule doing its job.

**Detection is asked once**, before raw mode and before the first paint,
which is the one moment stdin belongs to nobody. `VMIN`/`VTIME` give the read
a real timeout: a plain read would block forever on exactly the terminals the
question exists to find. Without graphics, `p` is a notice and the frame you
are looking at stands.

## Degradation

**A page that breaks the script is read another way, not given up on.** A failed
`Source::Script` extraction degrades the tab and asks `DOMSnapshot` once; a failed
`Source::Snapshot` is the end of the line and leaves the frame you are looking at
alone. A degraded tab asks the snapshot first from then on, so a permanently broken
page costs one round trip per scroll rather than two, and navigation clears the flag
because a new document reinstalls `bootstrap.js`. That also makes reload the way back.

**The effect names the source.** Not the page, and not a field the page sets: the rule
is a decision, so it lives in `Session` where a test needs no browser. `Job::Extracted`
carries a `Result` for the reason `Job::Hints` does, and carries the source because a
failed scroll and a failed extraction used to arrive as the same `Job::Failed`.

**A snapshot is the whole document.** The script path costs what is on screen and this
one cannot, so a degraded read of `heavy.html` pays for all fifteen hundred paragraphs:
`measure_snapshot` puts that at ~26ms against the script path's ~4ms for the same
fourteen runs. Accepted rather than solved: it is a fallback and not a mode anyone
chooses, and it is slow rather than unusable. Culling to the viewport is on our side,
and it is the only reason it is bearable.

**What a degraded tab loses** is the caret, wrapping inside a control, and the hint
occlusion test. Everything else keeps working, because scrolling and input go over CDP
and never through our script.

**Without a graphics protocol the picture is half-block, not a notice.** `▀` with the
top sample as foreground and the bottom as background, which is cells, so the diffing
renderer and every overlay rule apply unchanged and a label over a picture costs
nothing. `p` never refuses.

**The picture is asked for at the size that will show it.** Twice the sample grid,
because Chromium preserves the source aspect while fitting inside both bounds and a
half cell is not square: asking for exactly the grid returns a letterboxed page. A few
kilobytes rather than a few hundred, which is what makes decoding it in process
reasonable.

**`wwt-png` decodes what Chromium sends and refuses the rest.** Base64, IHDR, inflate,
unfilter, always RGBA out. No interlacing, no palettes, no 16-bit: a decoder that
accepts what it will never be given is untested code, and a wrong guess puts a
plausible wrong picture on screen.

**The decode happens in `on_frame`, never in `compose`.** Composing is what a hint
label and a statusline update each cost. It is on the loop's thread because a frame
arrives on the CDP arm of the `select!` and never as a job, and the numbers make that
fine: `measure_halfblock_frame` is ~3.7ms of decode and compose against the 33ms
pacing interval. That is a release build; unoptimised it is ~47ms, almost all of it
inflate, which is why the measurement prints a number and asserts only that the frame
reached cells.

## Hardening

**A tab can exist without a target.** `Presence` is `Opening`, `Attached` or
`Detached`, and the three features M7 adds are that one state pointed three ways:
eviction detaches the tab you looked at longest ago, a dead browser detaches all
of them, and a restored tab starts that way. Building the state first is why the
three are one mechanism rather than three.

**`Tab::detach` is the one place that says what survives.** What it keeps is what
the tab looked like: url, title, offset and runs, which is what makes switching
back a repaint. What it drops is every in-flight flag, because `Core` holds no
page for that tab any more and a flag nothing can clear is `f` dead for the rest
of the run. Getting that list wrong is three bugs rather than one.

**A reattach is `Effect::OpenTab`.** It already carries the scroll offset, for
M4's reason, and its `Job::Opened` already activates the tab, restarts the
screencast and triggers the first read, so a reattach inherits every rule an open
has rather than needing its own copy of them. This is now the fifth place the
picture follows the focus.

**Eviction runs after any focus change, and `look_at` is the one place focus is
assigned** when the tab under it changes. Opening a tab is a focus change too:
stamping recency only on a switch made a tab you had just opened look like the
one you had looked at longest ago, so the newest was the first evicted. **The
limit is a target and not a guarantee**: a tab with work in flight is never
taken, because its url still names where it is leaving.

**The old browser is dropped before the new one launches.** `Chromium` is
kill_on_drop and the profile directory is the lock, so relaunching while our own
dying browser still holds it is the one failure this path would inflict on
itself, and it would present as an inexplicable fall back to a private session.

**The CDP arm is guarded off once it has answered `None`.** A closed receiver
answers `None` immediately and forever, so an unguarded arm spins the loop at one
hundred percent under a browser that has gone, which is worse than the failure
being handled.

**A timed-out read sets `Stalled` and does not degrade.** `DOMSnapshot` needs the
same main thread our script does, so asking it costs a second deadline for the
same answer. A degraded tab is one whose *script* is broken; a stalled one is a
page that is not running. **A stalled tab needs no retry policy**, because a
wedged page cannot run the observer that would ask again: a keystroke or a reload
is how it is asked.

**Relaunching is asked for by a keystroke and never by a timer.** An idle wwt
costs ~zero CPU, and that rule does not get an exception for the state where
there is nothing to be busy about. `relaunching` is the fourth in-flight flag,
and it is there so a held `j` after a failed relaunch asks once rather than once
a repeat.

**Restore is lazy: startup launches one page.** The bar is complete on the first
frame either way, because titles and urls have been in the session file since M4.
A restored tab that has never been read shows loading for one round trip when you
reach it, where an evicted tab paints its cached runs at once. Both are right,
and the difference is real: one of them has been read and one has not.

**`toml` is the one dependency added since the set was fixed**, and it was asked
for.

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
  page produces no frames, so this costs nothing at idle, and that was measured. M5
  asks the compositor for frames and found the other half of that trade: an
  animating page outruns what a terminal can decode. The flag stays and the ack is
  held back instead, because text is the mode wwt is in almost always.
- **Nothing deep-copies a payload.** An extraction is every run on screen;
  `Client::send` and `Page::js` take their `Value` rather than clone it, or the whole
  of it is copied twice on the way to the caller.
- **A read that paints no runs asks for less.** `Extraction` is runs plus a `Status`,
  and `Page::status()` reads the second half alone: no walk, no `getClientRects`, no
  field mirrors. `measure_status` puts it under a millisecond against
  `measure_extraction`'s ~4ms on the same page, most of what is left being the round
  trip itself. A dirty signal in pixel mode asks for that instead, because `compose`
  paints the picture and never the runs, so the walk was a forced layout on the same
  main thread that has to paint the next frame, for an answer that was thrown away.
  Scrolling is what makes it matter: one scroll keystroke is one dirty signal, and in
  pixel mode that used to be a full extraction per frame.

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
