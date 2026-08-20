# wwt M2 — Navigation and Reading

**Date:** 2026-08-19
**Status:** Approved, pre-implementation
**Parent spec:** `2026-08-19-wwt-design.md` — sections 5, 6, and 8 govern here.

This is a delta against the system design, not a replacement for it. Where the two
disagree, the parent spec wins and this document is wrong.

## 1. What M2 delivers

M1 renders one page and exits. M2 makes it a browser you can read with: the page
stays live, scrolls natively, re-renders when it changes, and you can navigate
somewhere else without restarting the process.

At the end of M2, `wwt https://example.com` is a usable read-only browser. It
cannot click, type, or hold more than one page — those are M3 and M4.

### In scope

Scroll, `:open`, history, the diffing renderer, the `MutationObserver` dirty-signal
loop, terminal resize, `Page.loadEventFired`, and loading/error states in a
statusline.

### Out of scope

Hints, insert mode, mouse dispatch, tabs, the persistent profile, pixel mode, reader
mode, the accessibility-tree fallback, and the Chromium supervisor. M1's deliberate
limitations that M2 does not lift: no background colors, no images, no text
selection.

## 2. Architecture

The parent spec's section 5 diagram is the architecture. One core task owns all
state and is the only thing that mutates it, driven by a single `select!` over three
sources: terminal events, CDP events, and a debounce timer.

Two alternatives were considered and rejected:

**Actor per page.** Each page a task owning its own state, the core a router. This
is the right shape at M4 when there are several pages to own. Building it now means
designing a message protocol against one participant and guessing at the second.

**Fixed-tick poll loop.** Wake at 60Hz, re-extract if a dirty flag is set. Simple,
and exactly what section 4 of the parent spec rejects: an idle page must cost
approximately zero CPU. A browser you leave open cannot spin.

### Crate deltas

| Crate | Change |
|---|---|
| `wwt-frame` | `Frame::paint_text`, `Style.reverse`. Still no I/O, still no dependencies. |
| `wwt-cdp` | Event subscription. `Page` ownership moves from borrow to `Arc`. |
| `wwt-page` | Installed script, navigation, scroll, history, event-driven load. |
| `wwt-term` | `Renderer`: a diffing renderer holding the last presented frame. |
| `wwt` | `core.rs` (the loop and all state), `chrome.rs` (statusline and command line). |

`wwt-ui` is **not** created in M2. The parent spec's component table assigns chrome to
it, but M2's chrome is one statusline and one command line with a single consumer. A
crate boundary drawn before its second consumer exists is a guess. M3 creates
`wwt-ui` when hint overlays and the modal state machine arrive, and moves M2's chrome
into it.

## 3. The event pump

`wwt-cdp`'s `read_loop` currently drops every message without an `id`. Those are
protocol events, and M2 needs them.

```rust
pub struct Event {
    pub session_id: Option<String>,
    pub method: String,
    pub params: Value,
}

impl Client {
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<Event>;
}
```

Subscribers are registered in a `Vec` behind the same mutex as `pending`. A message
with an `id` resolves a pending call as it does today; everything else is broadcast.
A closed receiver is dropped from the list on next send, so a subscriber that goes
away does not leak.

Events M2 consumes: `Page.loadEventFired`, `Page.frameNavigated`,
`Runtime.bindingCalled`.

### Page ownership

`Page<'a>` borrows its `Client`, which is why `wwt-page`'s test harness holds the two
separately and each test opens its own page. A core loop that owns both cannot use a
borrow without becoming self-referential, so `Page` takes an `Arc<Client>` instead.
This is required for M4's several pages over one connection regardless, and it
simplifies the existing harness rather than complicating it.

## 4. The injected script

M1 evaluates `extract.js` as an IIFE on every extraction. M2 splits it in two.

**Bootstrap**, registered with `Page.addScriptToEvaluateOnNewDocument` so it survives
navigation, per parent spec section 4. It defines `window.__wwt.extract()` and
installs the dirty-signal listeners.

**Extraction** becomes `Runtime.evaluate("__wwt.extract()")` — one round trip
returning one flat array, unchanged in shape from M1.

### The dirty signal

`Runtime.addBinding` registers `__wwt_dirty` on the session. The bootstrap calls
it from:

- a `MutationObserver` on `document` (`subtree`, `childList`, `characterData`,
  `attributes`), 50 ms trailing debounce
- a passive `scroll` listener on `window`, 16 ms trailing debounce
- the `load` event

Each arrives as `Runtime.bindingCalled`. The core sets a dirty flag and keeps at most
one extraction in flight; when one completes and the flag is still set, it extracts
again. This self-coalesces with no timer and no polling — a page that mutates in a
tight loop costs one extraction per extraction, not one per mutation.

### Extraction cost

`extract.js` calls `getBoundingClientRect` once per character to group characters
into lines. Its own header comment schedules the replacement for M2, and M2 is where
it starts to matter: every scroll now triggers a re-extraction, so this cost *is* the
scroll latency.

The replacement is the one the comment names: `range.getClientRects()` over the whole
text node yields its line boxes directly, and a binary search over character offsets
finds where the text splits between them. `O(lines · log chars)` forced layouts
instead of `O(chars)`.

That bound is per node, and a long document is almost entirely nodes nobody can see,
so the walk asks each node's line boxes whether any of them reaches the viewport
before it splits anything. The document costs one layout read per text node; only the
handful on screen cost a search.

This is a distinct step with a before-and-after measurement on a heavy page, not a
change folded into another task. If it does not measurably help, it does not land.

## 5. The grid split

Chrome occupies the last terminal row. The page viewport becomes `rows - 1` tall, so
`css_height` shrinks by exactly one cell and Chromium reflows to the smaller window —
the page genuinely does not know the statusline exists.

```
grid          = (180, 48)      from the probe
page viewport = (180, 47)      what Chromium is told
frame         = (180, 48)      composited: page rows 0..46, chrome row 47
```

The composited `Frame` stays full-grid. Runs painted through the shorter viewport
cannot reach the last row, so chrome is `Frame::paint_text` onto it and no
compositing or clipping machinery is needed.

`Style` gains `reverse: bool` for the statusline. Not `bg: Option<Rgb>`, which the
parent spec's Frame description implies: extraction does not produce background
colors in M2, so the only consumer would be chrome, and reverse video is what chrome
actually wants.

## 6. Scroll

Scrolling dispatches `Input.dispatchMouseEvent` with type `mouseWheel` at the
viewport center, `deltaY` in CSS pixels. Chromium scrolls natively, so sticky
headers, infinite scroll, and virtualized lists work with no special handling — the
reason the parent spec chose this over `window.scrollBy`.

| Key | Movement |
|---|---|
| `j` / `k` | one cell |
| `d` / `u` | half a page |
| `space` / `b` | a full page, less two rows of overlap |
| `g` / `G` | top / bottom |

**A keypress waits for the truth.** Press `j`, and the wheel event dispatches, the
page scrolls, the scroll listener fires the binding, extraction runs, the renderer
diffs, and changed cells paint. Nothing appears on screen until it reflects the
page's real state.

Waiting for the truth is not the same as waiting longer than the truth takes, and
measuring the pipeline hop by hop found most of it was neither Chromium's work nor
ours. Two things were paying for nothing:

- **Headless paces frame production at the display's rate**, and a scroll is not
  visible to the page until the frame it lands on. `--disable-frame-rate-limit`
  removes that cap. An idle page produces no frames, so it costs nothing to idle
  CPU, which was measured rather than assumed.
- **The scroll signal trailed.** One keypress produces exactly one scroll event, so
  a trailing window coalesced nothing and delayed everything. It leads now, and the
  window rate-limits only what follows it, which is what a page that scrolls itself
  every frame needs.

Together: 36ms to 5ms from wheel dispatch to new runs in hand, on the heavy fixture.
Neither change alone gets there, because the frame cap hides the window and the
window hides the frame cap. `measure_scroll_latency` keeps the number honest.

The alternative — shifting the frame locally and correcting when the extraction
lands — was rejected. It creates two sources of truth for the same pixels, blanks the
newly exposed edge until the correction arrives, and makes sticky headers visibly
jump back into place. M2 instead measures the real latency and treats it as a number
to improve (see section 4), not to hide.

**One exception, stated rather than hidden:** `g` and `G` cannot be expressed as a
wheel delta, because the distance to the document's end is not known to us and the
document may grow while we scroll. They use `Runtime.evaluate` with `window.scrollTo`.
This is the only place in M2 where scrolling is not native, and it is why `G` on an
infinite-scroll page reaches the end of what has loaded rather than the end of the
document. That is the correct behavior; it is simply not wheel-driven.

## 7. Navigation and history

History is Chromium's, not ours. `Page.getNavigationHistory` returns the entries and
the current index; `Page.navigateToHistoryEntry` moves between them. A `Vec<Url>` of
our own would be smaller code and would silently diverge from reality the first time
a page called `pushState` — a single-page application's navigations would be
invisible to it. Asking the browser costs one round trip on a keypress.

`H` and `L` are back and forward. `Ctrl-r` reloads.

### Commands

`:` enters command mode; the last row becomes the command line, the terminal cursor
is shown at the insertion point, `Enter` runs, `Esc` cancels. `o` is a shortcut that
enters command mode pre-filled with `:open `.

| Command | Effect |
|---|---|
| `:open <url>` / `:o` | navigate the current page |
| `:back` / `:forward` | history |
| `:reload` | reload |
| `:quit` / `:q` | exit |

A bare host gains `https://`. There is no search-engine fallback in M2: input that
does not parse as a URL is an error in the statusline, not a search. Choosing a
default engine is a configuration question, and M2 has no configuration.

## 8. States and failure

The core tracks one state for the page:

```
Loading | Ready | Stalled | Error(String)
```

The statusline shows it alongside the URL, the title, and the scroll percentage.

**The frame is never blanked**, per parent spec section 8. Navigating keeps the
previous page on screen, marked `loading`, until the new one has been extracted.

Failures split in two, because Chromium handles one kind itself:

- **Failures Chromium reports to us** — a malformed URL, a load that exceeds its
  deadline, a CDP error — set `Error` or `Stalled` and leave the old frame exactly
  where it was.
- **Failures Chromium absorbs** — DNS and connection failures — are not command
  failures at all. Chromium navigates to its own `chrome-error://` page and fires a
  normal load event, so we extract and render that page. We keep it: it names the
  host and the specific failure, which is more use than a stale frame, and it is what
  every other browser shows. What we add is the statusline, which detects the
  `chrome-error://` URL and reports `Error` against the URL that was asked for, so
  the address line never silently claims a page loaded that did not.

This is a behavioral change, not only an addition: M1 exits the process on a
navigation failure. After M2, nothing a page does terminates the browser.

Load completion moves from M1's `document.readyState` polling loop to
`Page.loadEventFired`, which the M1 source comment already schedules for M2. The
30-second deadline remains, and expiry means `Stalled` rather than an error return.

## 9. Resize

A crossterm `Resize` event arms a 100 ms deadline in the `select!`; further resizes
re-arm it. On expiry: re-probe the terminal, rebuild the viewport, push
`Emulation.setDeviceMetricsOverride`, force a re-extraction, and repaint in full.

The debounce matters because a dragged window edge produces a resize event per frame,
and each one would otherwise cost a Chromium relayout and a full extraction.

The renderer's cached frame is discarded whenever the grid dimensions change — a diff
against a frame of different dimensions is meaningless.

## 10. The diffing renderer

`wwt-term::render` repaints all 8,640 cells every time. M2 replaces it with a
`Renderer` that holds the last presented frame and emits only what changed: for each
row, the changed segments, each preceded by a cursor address, with style tracking
across the segment as today.

A page where one counter ticks costs a handful of bytes per update, which is the
parent spec's stated goal in section 5.

The free `render` function stays as the full-repaint path, used for the first paint
and after a resize. The signature of neither changes when M5 adds a pixel buffer to
`Frame`.

## 11. Testing

Per parent spec section 9, the subtle logic lives where it can be tested without a
browser.

| Crate | Tests |
|---|---|
| `wwt-frame` | `paint_text` placement and clipping at the grid edge; `reverse` styling. |
| `wwt-term` | An unchanged frame emits nothing. One changed cell emits one cursor address and one glyph. A dimension change forces a full repaint. |
| `wwt-cdp` | The event pump, by feeding a synthetic stream into `read_loop` — it is already generic over `S: Stream`, so this needs no browser and no trait refactor. Responses must still correlate while events flow. |
| `wwt-page` | A tall fixture: extract, scroll, extract, assert the first row differs. A mutation fixture: change the DOM, assert the dirty binding fires. |
| `wwt` | Command parsing (`:open example.com` → `https://example.com`; unparseable input → error). Scroll arithmetic at each key. One test driving the modal flow through the same types the loop uses rather than a PTY: it covers the `:`-to-command path deterministically, needs no new dependency, and cannot flake on process timing. |

The extraction rewrite in section 4 carries a measurement, not a test: a heavy
fixture timed before and after, recorded in the plan.

## 12. Open questions

**The fallback extractor's source.** Parent spec section 8 specifies
`Accessibility.getFullAXTree` as the degradation path when the injected script
throws. `AXNode` carries `role`, `name`, and `backendDOMNodeId` but **no geometry**,
so an AX-tree fallback cannot feed the geometric renderer at all — it can only
produce a reflowed linear document, which is why M6 pairs it with reader mode.

`DOMSnapshot.captureSnapshot` satisfies the same requirement — CDP-native, sharing no
code with `extract.js`, so a bug in the extractor cannot take a page from degraded to
unusable — and *does* return layout geometry. It would feed the normal renderer, need
no reflow layer, and become independent of reader mode entirely.

This does not affect M2, which builds neither. It affects whether the fallback stays
inside M6 or becomes a small, independent piece deliverable much earlier. It should
be settled before M6 is planned, and it is recorded in the parent spec's section 8.
