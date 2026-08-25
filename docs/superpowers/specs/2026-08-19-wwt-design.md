# wwt — Design

**Date:** 2026-08-19
**Status:** Implemented through M8

## 1. What this is

`wwt` (world wide terminal) is a terminal web browser in Rust, intended as a genuine
daily driver rather than a text-mode curiosity. It drives a real headless Chromium
over the Chrome DevTools Protocol (CDP) and renders pages into the terminal grid two
ways: a crisp text-mode reconstruction by default, and true pixel rendering on
demand.

Both modes share one coordinate space, so switching between them preserves scroll
position, focus, and link hints exactly.

### Goals

- Full general web, including logged-in web apps
- Text rendering by default; pixel rendering on a keypress
- Persistent logged-in sessions across restarts, including OAuth flows
- Multiple tabs
- Forms and text input that work in real web apps
- Vim-style link hinting
- Idle pages cost approximately zero CPU
- Best preformence possible

### Non-goals

- Writing a layout or JavaScript engine. Chromium does layout; we render its output.
- Being a Chromium-free browser. A Chromium binary is a hard dependency.
- Faithful reproduction of decorative CSS. Text legibility beats visual fidelity
  in text mode; pixel mode exists for when fidelity matters.
- Supporting terminals without at least 256 colors.

### Success criterion

The author stops reaching for Firefox for reading documentation, GitHub, forums,
and email, and only opens a GUI browser for video and canvas-heavy applications.

## 2. Approach

Three approaches were considered:

**A. Semantic reflow.** Extract the DOM or accessibility tree, discard CSS layout,
re-lay-out content at terminal width. Excellent reading experience, trivial to
build, but text and pixel modes then have unrelated geometry — hints, scroll, and
focus do not correspond, and switching modes loses your place. Application UIs
reflow into nonsense.

**B. Geometric text rendering.** Set Chromium's viewport to exactly the terminal
grid measured in pixels, extract every text run's layout box, and paint each run
into the cells its box covers. One coordinate space for both modes.

**C. Pixel-first with text overlay** (the browsh model). Continuous screenshots
downscaled into cells with text overlaid. Highest fidelity, but constant
screencasting costs CPU and latency, and it requires building B's text-positioning
machinery anyway to make the overlay legible.

**Chosen: B.** It is the only option where the two modes are the same page viewed
two ways rather than two browsers that disagree with each other. It is also the
substrate the other two bolt onto cheaply: A becomes a per-page reader-mode toggle,
C becomes the on-demand pixel path. Starting with A would mean discarding it when
pixel mode arrives.

## 3. The coordinate model

This is the load-bearing decision. Everything else follows from it.

We tell Chromium the viewport is exactly the terminal grid, measured in pixels:

```
cell_css     = (9, 20)              # one terminal cell, in CSS pixels
grid         = (180 cols, 48 rows)  # from TIOCGWINSZ
viewport_css = (1620, 960)          # what Chromium believes the window is
```

Chromium lays out a normal desktop page at that size: real desktop CSS, real media
queries, no mobile fallback. Conversion in both directions is a division:

```
cell_x = floor(css_x / cell_css.w)      css_x = (cell_x + 0.5) * cell_css.w
cell_y = floor(css_y / cell_css.h)      css_y = (cell_y + 0.5) * cell_css.h
```

Consequences:

**Zoom is the cell mapping, not a CSS zoom.** Declaring one cell to be 12x26 CSS px
shrinks the viewport to 1215x720; the page genuinely reflows and hits different
breakpoints. Zoom is one number, `cell_css`, and re-layout is free because Chromium
performs it.

**Horizontal mapping is near-exact; vertical requires snapping.** Body text at 16px
averages roughly 8.2px per glyph against a 9px cell, so a run's character count is
within a few percent of the cell width its box occupies. We place the run at
`floor(x / cell_css.w)` and elide if it overruns. Vertically, a 24px line box against
a 20px cell does not divide evenly, so each line box snaps to the row containing its
**baseline**. Snapping by baseline rather than box top prevents drift in multi-column
and mixed-font-size content.

**The page does not start at the top of the screen.** The chrome occupies rows the
page does not know about, so `Viewport` carries an origin row and the conversions above
map CSS pixels to *frame* rows rather than page rows. The load-bearing property is
unchanged in form and stronger in content: converting a cell to CSS and back is the
identity, at every cell size **and at every origin**. The page's CSS size is unaffected,
because where the page sits on our screen is not something the page is told.

**Pixel mode is the same viewport.** `Page.startScreencast` at that exact size,
blitted through the Kitty graphics protocol using unicode placeholders so images sit
within the cell grid and scroll with it. Switching text/pixel changes nothing about
geometry, scroll offset, or focus. `--disable-frame-rate-limit`, which M2 added because
headless otherwise paces a scroll at the display rate, owes a measurement here: an
uncapped compositor is free only while nobody is asking it for frames, and a screencast
asks.

**Cell size detection.** `ioctl(TIOCGWINSZ)` against the controlling tty provides
`ws_xpixel`/`ws_ypixel`; cell size is those divided by the grid dimensions. If the
terminal reports zeros, fall back to querying `CSI 14 t` (window size in pixels) and
`CSI 18 t` (grid size). If both fail, fall back to a configurable default of 9x20
and warn once.

### Accepted costs

- Text below roughly 11px cannot be represented honestly at one glyph per cell and
  renders as a dim block. Zoom in, or use reader mode.
- Overlapping absolutely-positioned content requires painter's-algorithm resolution;
  the later stacking context wins the cell.
- Proportional glyph widths are not preserved within a run. We place runs by their
  box origin and let them occupy their natural character count.

## 4. Components

A Cargo workspace, split so the difficult logic is pure and testable.

| Crate | Purpose | Depends on |
|---|---|---|
| `wwt-frame` | The `Frame` type and all coordinate math: cell grid, styled cells, interactive-box list, compositing, elision, hit-testing. **Zero I/O.** | none |
| `wwt-reader` | Semantic reader documents and pure terminal-width reflow: rows, source anchors, link ranges, and painting. **Zero I/O.** | `wwt-frame` |
| `wwt-cdp` | CDP transport: websocket, request/response correlation, typed commands, event subscription, target lifecycle | tokio, tungstenite |
| `wwt-page` | One tab: owns the injected scripts, extracts page geometry or a semantic reader document, dispatches input to the page | `wwt-cdp`, `wwt-frame`, `wwt-reader` |
| `wwt-term` | Terminal I/O: cell-size probe, grid diffing and flush, Kitty graphics protocol, key/mouse decoding | crossterm |
| `wwt-ui` | Chrome: tab bar, statusline, command palette, hint overlay, modal state machine | `wwt-frame`, `wwt-term` |
| `wwt` | Binary: session and tab management, config, keymap, wiring | all |

`wwt-frame` having no I/O is deliberate: snapping, occlusion, elision, and hit-testing
are testable with plain unit tests and no browser in the loop.

### The Frame

The central type. Every rendering mode produces one, and the renderer consumes it:

- A grid of styled cells (glyph, fg, bg, attributes)
- A list of interactive boxes in **cell** coordinates, each with its backing CSS-pixel
  rect and a stable element handle for dispatch
- An optional image for the page viewport, base64 on its way through rather than
  pixels, because pixel mode is the whole viewport and never a region
- Scroll offset and page metadata

### CDP client: hand-rolled

We write `wwt-cdp` rather than adopting `chromiumoxide`. The slice of CDP we need is
narrow and unusual — `Runtime.addBinding`, `Page.startScreencast`, and raw
`Input.dispatchKeyEvent` with exact `windowsVirtualKeyCode` values — and a
page-level abstraction fights us on precisely those, while its generated surface is
large. The layer is roughly 800 lines, it is the layer we will debug most, and
owning it outright is worth more than the wrapping it would save.

### The injected script

One JavaScript file, injected via `Page.addScriptToEvaluateOnNewDocument` so it
survives navigation. Responsibilities:

1. Walk text nodes, collecting `Range.getClientRects()` and computed style
   (color, weight, size, stacking depth) per run
2. Read the text of form controls, which point 1 cannot reach. A control's value
   is not in the DOM: `input.childNodes` is empty however much you type into it,
   because the browser paints the value from element state rather than from a
   text node. Without this pass you cannot see what you are typing. The pass
   also covers placeholders, a `select`'s chosen option, and passwords, which
   are reported as bullets: the frame shows what the browser shows.
3. Collect interactive elements (`a`, `button`, `input`, `select`, `textarea`,
   `[role=button]`, `[tabindex]`, `[onclick]`) with their client rects, on demand
   rather than per extraction (section 6)
4. Collect replaced-content boxes (`img`, `canvas`, `video`, `svg`) as block
   placeholders
5. Signal dirtiness through `Runtime.addBinding` from a debounced `MutationObserver`
   plus scroll and resize listeners, and from `input`, `selectionchange`, `focusin`
   and `focusout`, which are what point 2 changing looks like

Point 5 is what makes the system event-driven rather than polling: we re-extract only
when the page changes, so an idle page costs no CPU. This is the difference between a
browser you leave open and one you close.

The second half of point 5 exists because a form control's value and selection are
element state rather than DOM: nothing mutates when you type into an `input` or walk
the insertion point through it, so the observer sees none of it and the pass in point
2 reads state nothing signals. Without those four listeners what you typed, and where
the caret sits, stay on screen as they were until something unrelated changes the page.

The `focusin` listener signals dirtiness and nothing else. An earlier draft had one
that mode followed, so that the page could put us in insert mode; section 6 says why
*that* is gone. Repainting a page that already looks different is not the same thing
as letting it take the keyboard.

Extraction returns one flat, sorted array per pass through a single `Runtime.evaluate`
round trip, never thousands of individual `DOM.getBoxModel` calls. On a heavy page
that is roughly 15ms versus several seconds.

## 5. Data flow

Two loops, decoupled by channels, neither able to block the other:

```
                    +---------------- tokio ----------------+
  terminal --keys-->| input task --> core --> page.dispatch |--CDP--> Chromium
                    |                 ^            |        |
   screen  <--diff--| renderer <------+-- extract <+--------|<-event--+
                    +---------------------------------------+
```

The core owns all state and is the only thing that mutates it. Input events and CDP
events arrive as messages on one `select!`. There are no locks around the frame, and
a hung page cannot freeze the UI — the statusline marks that tab stalled while other
tabs stay usable.

**Amended in M3.** "The core" is two things with a seam between them, because one
module holding both left every rule in the browser untestable. `Session` owns the
state and decides: `on(Event) -> Vec<Effect>`, `compose() -> Frame`, reaching no page,
no socket and no terminal. `Core` is the adapter around it — tokio in, spawns out —
and decides nothing. Events are what arrives on the `select!`; effects are what the
loop does. The properties above are unchanged; what changed is that they can now be
asserted without a browser.

**Rendering is diffed.** `wwt-page` produces a new `Frame`; the renderer diffs it
against the last presented frame and emits escape sequences only for changed cells.
A page where one counter ticks costs a handful of bytes per update.

## 6. Input

**Key dispatch.** Terminal key events carry no keycodes, but `Input.dispatchKeyEvent`
requires `windowsVirtualKeyCode`, `code`, `key`, and `text` to be mutually consistent
or web applications misbehave — anything reading `e.code`, and every application
keyboard shortcut. A static table maps crossterm `KeyEvent` values to that quad.
Tedious but bounded; correctness here is the difference between typing into boxes and
working in web apps.

**Mouse and scroll.** Clicks convert the target cell's center back to CSS pixels and
dispatch `Input.dispatchMouseEvent`. Scrolling dispatches `mouseWheel`, so Chromium
scrolls natively — sticky headers, infinite scroll, and virtualized lists work with no
special handling.

### Modes

- **Normal** — keys are browser commands: `j`/`k` scroll, `f` hints, `o` open,
  `:` command palette, `gt`/`gT` tab switching, `p` toggles pixel mode, `r` toggles
  reader mode.
- **Insert** — every keystroke forwards to the page. Entered with `i` or by hinting a
  text field, exited with `Esc`, which is never forwarded. `Ctrl-]` sends the page a
  literal Escape, since a terminal cannot distinguish `Ctrl-[` from `Esc`.
- **Hint** — `f` overlays labels on every interactive box; typing a label clicks it.
  The boxes come from their own query, run when `f` is pressed and cached until the
  page next says it changed. Extraction knows only about text nodes, and adding an
  element sweep with a hit test per candidate to a path that runs on every scroll
  frame would buy nothing: hints are pressed occasionally, and a query made at the
  moment you press `f` describes the page as it is now. Filtering is then local:
  assign labels, paint over the frame, filter on keypress.
- **Command** — a `:` line for `:open`, `:tabclose`, `:set zoom`, and similar.

### Reader mode

Approach A, retained as a per-page escape hatch rather than a rendering strategy.
Pressing `r` picks the dominant content subtree, discards its CSS layout, and reflows
its text to terminal width as a linear document. It is for pages whose real layout is
hostile to a cell grid — dense multi-column marketing pages, or body text set below
the legibility threshold in section 3.

Reader mode deliberately breaks the shared coordinate space: its geometry is our own.
Its hints are direct terminal cell positions handed to the existing hint UI, and its
links are URL destinations rather than synthetic clicks into page geometry. Scroll
keys and the wheel move a local reader row without moving Chromium underneath. A
second `r` therefore returns to the real page at the exact scroll offset it held
throughout.

Reader is a per-tab view, not a fifth input `Mode`. Normal, insert, hint and command
still answer what the keyboard means. Reader answers which document normal mode is
looking at. If the global pixel preference is on, leaving reader shows that unchanged
real page as pixels. The statusline says `[reader]` only while the semantic document
owns the page area.

Mode changes only in response to a keystroke. A page that autofocuses its search box
does not take the keyboard, and a mouse click does not either: `i` hands it over and
`Esc` takes it back. Tracking the page's own focus was considered and rejected. It
reads as convenient until a page uses it to swallow `j`, and a browser whose keyboard
belongs to whatever site you happened to open is not one you can trust with a
single-key quit.

## 7. Sessions and tabs

One Chromium process with a persistent `--user-data-dir` at
`~/.local/share/wwt/profile`. This is what provides durable logins, and OAuth
redirects work because it is a real browser with a real cookie jar.

The chrome is two rows: the tab bar at the top and the statusline at the bottom, so the
page viewport is the terminal grid less two. Both are unconditional, which is what
keeps opening a tab from reflowing every page.

Each tab is a CDP target. Only the foreground tab holds an active extraction
subscription and screencast; background tabs keep their target alive but idle. A switch
therefore costs a round trip for the first pixel frame while pixel mode is on, and the
previous image stays on screen until it lands: the repaint-and-no-round-trip guarantee
of section 7, and the measurement that holds it down, are text mode's. Idle
has a precise meaning: a tab extracts once when it opens, so its title is real and the
first switch to it is instant, and after that it re-extracts only while focused. A
dirty signal for an unfocused tab sets a flag that is spent when focus arrives.

A page that opens a tab for itself, through `target=_blank` or `window.open`, creates a
target we did not ask for. It is adopted: the binding, the bootstrap and the viewport
are installed before it is allowed to run, and it becomes a real tab.

Restore is lazy: the focused tab opens and every other restored tab starts with no
target at all, paying for one only when it is reached. The tab bar is complete on the
first frame regardless, because titles and URLs come out of the session file. The
sentence above about a tab extracting once when it opens still holds and now says
something slightly narrower, since a lazily restored tab has not opened yet.

Session state, meaning open URLs, titles and scroll positions, is serialized to disk on
change so a crash restores. The instance holding the profile owns that file; a second
instance, which cannot have the profile, runs on a temporary one and writes nothing.

Reader documents, layouts, local positions and the selected per-tab view live only in
memory. They survive tab switches, target eviction and a Chromium relaunch within one
run. They are deliberately absent from `session.json`: a cold start restores the URL
and real page offset, then begins in the real-page view.

## 8. Failure modes

Governing principle: **never blank the frame you are looking at.** Every failure
degrades to stale-but-labeled, never to empty.

- **Chromium dies.** Websocket close is the signal. A supervisor restarts it with
  backoff and rebuilds tabs from the live session state. Scroll positions survive; form
  contents and per-tab history do not. **Amended in M7:** the rebuild is from live state
  and not from the session file. The file is a debounced copy, up to `SAVE_DEBOUNCE`
  behind, so rebuilding from it would discard up to a second of navigation at the moment
  it is least worth discarding. The file remains what a cold start reads. Every tab
  detaches, keeping its url, title, offset and runs, so the frame you were reading stays
  up; the focused tab asks for a target when the replacement arrives and the rest wait to
  be reached.
- **Page hangs.** Every CDP command carries a deadline. On timeout the tab is marked
  stalled in the statusline, keeps its last frame, and remains switchable-away-from.
  **Settled in M7:** two deadline classes, 30s for a navigation and 5s for everything
  else, and a timeout is typed so that it can be told apart from a script that threw. A
  timed-out read does *not* fall back to `DOMSnapshot`: that fallback answers a different
  failure, and the snapshot needs the same main thread our script does. A stalled tab
  needs no retry policy, because a wedged page cannot run the observer that would ask
  again; a keystroke or a reload is how it is asked.
- **Injected script throws.** Caught at its top level and reported through the
  binding; that tab falls back to a CDP-native extractor that shares no code with our
  script, so a bug in the extractor cannot take a page from degraded to unusable.
  **Settled in M6: `DOMSnapshot.captureSnapshot`.** It returns layout geometry, so it
  feeds the normal renderer and needs no reflow layer, and it is therefore independent
  of reader mode rather than coupled to it. `Accessibility.getFullAXTree` was the
  original choice and is rejected: `AXNode` carries no geometry, so an AX-sourced
  fallback could only produce a reflowed linear document, which is what would have
  tied the fallback to a view. What a snapshot cannot do — the caret, wrapping inside
  a control, the hint occlusion test — is the fidelity that was traded for that
  independence.
- **Terminal resize.** Debounce 100ms, recompute the grid, push a new
  `Emulation.setDeviceMetricsOverride`, force re-extract. The page genuinely reflows.
- **No Kitty graphics.** In M5 pixel mode is refused with a notice and the frame you
  are looking at stands. From M6 it degrades to half-block unicode, and there is no
  tier below that. The third tier this said was coming, labeled placeholder blocks, is
  deleted rather than deferred: the renderer writes truecolor for every styled cell
  with no capability check at all, so a terminal that cannot show half-block cannot
  show text mode either, and a fallback below half-block would guard a case that
  cannot arise. A colour-capability probe, if one is ever wanted, is a change to text
  mode first. Text mode is unaffected either way, which is the point of text being the
  default.
- **No Chromium installed.** Detected at startup with a clear prompt to either point
  at a system binary via `chromium` in `config.toml`, or fetch a pinned
  Chrome-for-Testing build into
  `~/.local/share/wwt/`. Never a silent download.
- **Too many tabs.** Background targets beyond a configurable limit are closed while
  their URL and scroll offset remain in the session, and are transparently restored
  on switch. **Settled in M7.** "Configurable" is `max_tabs` in `config.toml`, defaulting
  to eight, and it counts live targets rather than tabs: the bar goes on showing all of
  them. The limit is a target and not a guarantee, because a tab with work in flight is
  never evicted and racing an answer already on its way would be a worse bargain than
  holding one target too many for a moment. Lazy restore at startup is the same machinery
  pointed elsewhere and lands with it.

## 9. Testing

- **`wwt-frame`: unit and property tests.** All subtle logic lives here and needs no
  browser. The property worth asserting: `cell -> css -> cell` is the identity for
  every cell in the grid at every zoom level. Most coordinate bugs die there.
- **Extraction: golden tests.** Fixture HTML served by a local server, driven through
  real headless Chromium, with the resulting cell grid asserted against a checked-in
  text snapshot. These snapshots are ASCII art of the rendered page; they diff well in
  review and are the tests that catch pages rendering wrong.
- **Input: the effect vocabulary, not a fake transport.** The original plan put
  `wwt-cdp` behind a trait and recorded calls against a fake. M3 put the seam a layer
  higher instead: `Session::on` returns `Vec<Effect>`, so "this key produced this
  click at these coordinates" is a plain equality assertion and there is no second
  implementation of a browser to keep honest. There is only ever Chromium; abstracting
  it would buy a fake and cost a lie.
- **The injected script: its arithmetic, directly.** `window.__wwt.__pure` exposes the
  line splitting, the offset search and the caret attribution, which take data and
  return data. These are the functions whose mistakes a rendered frame hides — a caret
  two characters along still looks like a caret — so they are asserted on as data.
- **One browser per test binary.** Handed out a test at a time, because
  `Input.dispatchMouseEvent` is answered by the target the browser has in front.
- **End-to-end: a handful, over a PTY.** Spawn the real binary against fixtures, send
  keystrokes, assert screen contents. Only for modal flows — enough to catch wiring
  breakage, not a second test suite.

**CI constraint.** The Chrome-for-Testing version must be pinned, because Chromium
version bumps churn the golden snapshots. Updating it is a deliberate, reviewed
commit rather than something that silently breaks the build.

## 10. Environment

Developed against Kitty on Linux with the Kitty graphics protocol available. Rust
1.97+, tokio, crossterm. Other terminals degrade per section 8 but are not the
development target.

## 11. Milestones

The work decomposes into increments that each end at something runnable. Each is
expected to be its own implementation plan; this section defines the boundaries, not
the steps.

**M1 — Walking skeleton.** Launch Chromium, attach over hand-rolled CDP, set the
viewport from the measured terminal grid, extract text runs through the injected
script, paint a static page into the cell grid, quit cleanly. One tab, no input beyond
`q`. This proves the coordinate model end to end and is the milestone worth being
slowest and most careful about — everything later assumes it is right.

**M2 — Navigation and reading.** Scroll, `:open`, history, the diffing renderer, the
`MutationObserver` dirty-signal loop. At this point it is a usable read-only browser.

**M3 — Interaction.** The keymap table, mouse dispatch, hint mode, and insert mode,
with the four-mode state machine and the chrome moving into `wwt-ui`. Forms work. This
is the milestone that makes it a browser rather than a viewer.

**M4 — Tabs and sessions.** Multiple targets, adoption of targets a page opens for
itself, the persistent profile, session serialization and restore, and background-tab
idling. Logins survive restarts. Adoption is part of the milestone because a browser
with tabs that cannot follow a `target=_blank` link is not one.

**M5 — Pixel mode.** `Page.startScreencast`, the Kitty graphics protocol with unicode
placeholders, and mode toggling. Without graphics the toggle is a notice rather than a
worse picture: the half-block degradation path is M6's, because a fallback view belongs
beside the fallback extractor rather than beside the protocol it is a fallback for.

**M6 — Degradation.** The fallback extractor sourced from `DOMSnapshot.captureSnapshot`,
and the half-block path that shows a page on a terminal with no graphics protocol. Two
halves that share no code and one purpose: the browser keeps working when a piece of it
does not. Reader mode is not in it. Section 8's open question, answered, is what let it
move: a `DOMSnapshot` fallback returns geometry, so it feeds the normal renderer, needs
no reflow layer, and is independent of reader mode rather than the thing that had to be
built beside it.

**M7 — Hardening.** The Chromium supervisor and restart path, per-command deadlines,
session recovery after a crash, and the background-tab eviction and lazy restore
deferred from M4. Operational robustness, sharing nothing with M6 but the fact that
both are about a browser that does not fall over.

**M8 — Reader mode.** Delivered by the semantic extraction, pure reflow renderer,
reader link handling and per-tab view described in the M8 design. It remains last
because it is a nicety rather than a foundation: section 8's answer took its second
consumer away, and robustness had to come first.

Daily use realistically begins at M4. M1 through M3 are the foundation and should not
be rushed to reach it.
