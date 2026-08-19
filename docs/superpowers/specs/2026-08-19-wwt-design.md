# wwt — Design

**Date:** 2026-08-19
**Status:** Approved, pre-implementation

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

**Pixel mode is the same viewport.** `Page.startScreencast` at that exact size,
blitted through the Kitty graphics protocol using unicode placeholders so images sit
within the cell grid and scroll with it. Switching text/pixel changes nothing about
geometry, scroll offset, or focus.

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
| `wwt-cdp` | CDP transport: websocket, request/response correlation, typed commands, event subscription, target lifecycle | tokio, tungstenite |
| `wwt-page` | One tab: owns the injected script, extracts into a `Frame`, dispatches input to the page | `wwt-cdp`, `wwt-frame` |
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
- An optional pixel buffer for regions rendered as graphics
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
2. Collect interactive elements (`a`, `button`, `input`, `select`, `textarea`,
   `[role=button]`, `[tabindex]`, `[onclick]`) with their client rects
3. Collect replaced-content boxes (`img`, `canvas`, `video`, `svg`) as block
   placeholders
4. Signal dirtiness through `Runtime.addBinding` from a debounced `MutationObserver`
   plus scroll and resize listeners
5. Report focus changes via a `focusin` listener, so mode tracking follows the page

Point 4 is what makes the system event-driven rather than polling: we re-extract only
when the page changes, so an idle page costs no CPU. This is the difference between a
browser you leave open and one you close.

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
- **Insert** — every keystroke forwards to the page. Entered by hinting or clicking a
  text field, exited with `Esc`.
- **Hint** — `f` overlays labels on every interactive box; typing a label clicks it.
  Because boxes are already in cell coordinates from the same extraction, this is
  nearly free: assign labels, paint over the frame, filter on keypress.
- **Command** — a `:` line for `:open`, `:tabclose`, `:set zoom`, and similar.

### Reader mode

Approach A, retained as a per-page escape hatch rather than a rendering strategy.
Pressing `r` picks the dominant content subtree, discards its CSS layout, and reflows
its text to terminal width as a linear document. It is for pages whose real layout is
hostile to a cell grid — dense multi-column marketing pages, or body text set below
the legibility threshold in section 3.

Reader mode deliberately breaks the shared coordinate space: its geometry is our own,
so hints within it address reflowed positions, and switching back to text mode restores
the page's true layout at the scroll position we entered from. It is a distinct view,
not a third mode of the same view, and the statusline says so.

Mode tracks the page's reality, not only keystrokes: the injected script's `focusin`
listener means a site that autofocuses its search box on load puts us in insert mode
automatically, and clicking away drops out of it.

## 7. Sessions and tabs

One Chromium process with a persistent `--user-data-dir` at
`~/.local/share/wwt/profile`. This is what provides durable logins, and OAuth
redirects work because it is a real browser with a real cookie jar.

Each tab is a CDP target. Only the foreground tab holds an active extraction
subscription and screencast; background tabs keep their target alive but idle.
Session state — open URLs and scroll positions — is serialized to disk on change so a
crash restores.

## 8. Failure modes

Governing principle: **never blank the frame you are looking at.** Every failure
degrades to stale-but-labeled, never to empty.

- **Chromium dies.** Websocket close is the signal. A supervisor restarts it with
  backoff and rebuilds tabs from the session file. Scroll positions survive; form
  contents do not.
- **Page hangs.** Every CDP command carries a deadline. On timeout the tab is marked
  stalled in the statusline, keeps its last frame, and remains switchable-away-from.
- **Injected script throws.** Caught at its top level and reported through the
  binding; that tab falls back to a CDP-native extractor that shares no code with our
  script, so a bug in the extractor cannot take a page from degraded to unusable.
  **Open question — settle before M6 is planned.** The original choice was
  `Accessibility.getFullAXTree`, but `AXNode` carries no geometry, so an AX-sourced
  fallback cannot feed the geometric renderer and can only produce a reflowed linear
  document — which is what couples it to reader mode. `DOMSnapshot.captureSnapshot`
  meets the same independence requirement and does return layout geometry, so it
  would feed the normal renderer and need no reflow layer. The trade is fidelity of
  the degraded view against the size and placement of the milestone.
- **Terminal resize.** Debounce 100ms, recompute the grid, push a new
  `Emulation.setDeviceMetricsOverride`, force re-extract. The page genuinely reflows.
- **No Kitty graphics.** Pixel mode degrades to half-block unicode, then to labeled
  placeholder blocks. Text mode is unaffected, which is the point of text being
  the default.
- **No Chromium installed.** Detected at startup with a clear prompt to either point
  at a system binary via config or fetch a pinned Chrome-for-Testing build into
  `~/.local/share/wwt/`. Never a silent download.
- **Too many tabs.** Background targets beyond a configurable limit are closed while
  their URL and scroll offset remain in the session, and are transparently restored
  on switch.

## 9. Testing

- **`wwt-frame`: unit and property tests.** All subtle logic lives here and needs no
  browser. The property worth asserting: `cell -> css -> cell` is the identity for
  every cell in the grid at every zoom level. Most coordinate bugs die there.
- **Extraction: golden tests.** Fixture HTML served by a local server, driven through
  real headless Chromium, with the resulting cell grid asserted against a checked-in
  text snapshot. These snapshots are ASCII art of the rendered page; they diff well in
  review and are the tests that catch pages rendering wrong.
- **Input: fake transport.** `wwt-cdp` sits behind a trait; a recording fake asserts
  that a hint label produces the right `dispatchMouseEvent` coordinates and that the
  keymap emits a coherent quad across a table of keys.
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

**M3 — Interaction.** The keymap table, mouse dispatch, hint mode, insert mode, and
page-driven focus tracking. Forms work. This is the milestone that makes it a browser
rather than a viewer.

**M4 — Tabs and sessions.** Multiple targets, the persistent profile, session
serialization and restore, background-tab idling and eviction. Logins survive
restarts.

**M5 — Pixel mode.** `Page.startScreencast`, the Kitty graphics protocol with unicode
placeholders, mode toggling, and the half-block degradation path.

**M6 — Reader mode and the degradation path.** The reflow renderer, reader mode on
top of it, and the fallback extractor that feeds it when the injected script throws.
These are one milestone because they are one rendering path with two sources, not
because they are related in purpose. Splitting the fallback out would mean building
the reflow layer in one milestone and its second consumer in the next. See the open
question in section 8: if the fallback is sourced from `DOMSnapshot` rather than the
accessibility tree it needs no reflow, becomes independent of reader mode, and can
land considerably earlier.

**M7 — Hardening.** The Chromium supervisor and restart path, per-command deadlines,
and session recovery after a crash. Operational robustness, sharing nothing with M6
but the fact that both were once one milestone.

Daily use realistically begins at M4. M1 through M3 are the foundation and should not
be rushed to reach it.
