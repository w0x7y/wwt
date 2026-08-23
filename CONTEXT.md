# The vocabulary

The words this codebase uses for its own parts. They are consistent, they
are load-bearing, and most of them name a type you can go and read. If a
name here and a name in the code disagree, the code is right and this file
is a bug.

`CLAUDE.md` says how to work in the repo; the specs in `docs/superpowers/`
say what is being built and why. This says what the nouns mean.

## The page, as we see it

**Run** — one piece of text the page laid out, with its box, its baseline,
its style, and a `z`. The unit of everything painted: Chromium decides where
text goes, and a run is one answer. `wwt_frame::TextRun`.

**Extraction** — one pass of the injected script: every visible run, plus
the title, the URL, the scroll geometry, and the caret. Deliberately one
round trip, because it happens on every scroll frame. Nothing reads the page
any other way. `wwt_page::Extraction`.

**Dirty signal** — the page saying it has changed. Comes from a debounced
`MutationObserver`, a scroll listener, `load`, and the four field-state
events, through the `__wwt_dirty` binding. It is the *only* thing that
causes a re-extraction: there is no tick loop, and an idle page costs
nothing.

**Hint target** — one interactive box the page reported, with its rect and
whether activating it is a click or the beginning of typing. Queried on `f`
and cached until the next dirty signal, deliberately *not* part of
extraction. The query is the one effect whose answer changes the mode, so the
session knows while it is away: a second `f` asks nothing, and a late answer
opens hint mode only if the mode is still normal. `wwt_frame::HintTarget`.

**Caret** — where typing would land: a line's left edge, that line's
baseline, and a count of characters into it. Never a pixel position, because
the frame gives every character one cell whatever the font did, so a caret
placed by CSS x drifts left of the character it belongs beside.
`wwt_frame::Caret`.

## The screen

**Cell** — one character position in the terminal, with a foreground colour
and its flags. **Grid** — how many of them there are.

**Viewport** — the conversion between CSS pixels and cells, and the only
thing allowed to divide by a cell dimension. Zoom is a bigger declared
**cell size**, not a CSS zoom, so a page genuinely reflows into different
breakpoints. `wwt_frame::Viewport`.

**Page viewport** — the terminal grid less the two rows the chrome occupies,
and sitting below the tab bar. Chromium is told this is the whole window, so
the page knows about neither chrome row. The shift down lives in `Viewport`
as an **origin row** rather than as a `+1` wherever a cell is painted, so
`to_cell(to_css(c)) == c` holds at every origin. `session::page_viewport`.

**Frame** — a full grid of cells, plus where the terminal's cursor belongs
and, in pixel mode, the **image** behind them. The single output type every
rendering mode produces, so text mode and pixel mode, and later reader mode,
cannot diverge in how they reach the screen. `wwt_frame::Frame`.

**Image** — a picture of the page on its way to the terminal: base64 PNG,
which is how CDP sends it and how the graphics protocol wants it, so nothing
in wwt ever decodes one. Carries a **generation**, which is what the
renderer diffs on, because two frames can encode identically and comparing
the payloads would mean comparing a megabyte to answer what a counter
answers. `wwt_frame::Image`.

**Placeholder** — a cell carrying U+10EEEE, which shows part of an image
rather than a glyph. The image id rides in its foreground colour and its
position in two combining diacritics. A cell holding a real glyph shows the
glyph instead, which is how a hint label lands on top of a picture, and it
is why every cell spends its own diacritics: a cell without them continues
from the one before it, so an overlay in the middle of a row would orphan
everything after it.

**Chrome** — the two rows that are ours: the **tab bar** on top, and at the
bottom the statusline, or the `:` line when one is open. Both unconditional,
so opening a tab never reflows a page. **State** is what the statusline says
about the page — loading, ready, an error, a notice — and it is never a
reason to blank the frame.

## What the browser is doing

**Mode** — what keys mean right now: normal, insert, hint, or the `:` line.

**Pixel mode** — the page shown as a picture of itself rather than
reconstructed from its runs, entered with `p`. Global rather than per-tab,
and only the focused tab screencasts. The viewport, the scroll offset and
the focus are the same in both modes, which is what makes the toggle
instant.

**Screencast frame** — one picture of a page as CDP sends it. Not to be
confused with a `Frame`, which is the grid of cells; this one is a
`ScreencastFrame` and ends up as the image on one. **Ack id** is the integer
it must be answered with: CDP calls it `sessionId` and it is not a CDP
session id, it counts screencasts on one target. Chromium sends the next
picture only once the last is acked, which is what makes the ack the place
to set the frame rate.
Changes only in response to a keystroke; a page cannot move you between
modes, which is what makes handing it the keyboard safe. `wwt_ui::Mode`.

**Tab** — one page, and everything true of it rather than of the browser: its
URL, title, runs, caret, scroll offset, and what we have asked it for.
Identified by a **tab id**, a counter that never reuses a value, because a
page operation outlives the state that asked for it and an index would let a
closed tab's answer land on the tab that took its place. `wwt::tab::Tab`.

**Focus** — which tab you are looking at. The only tab that receives keys,
clicks and hint queries, and the only one painted. Switching **activates**
the target as well, because `Input.dispatchMouseEvent` is answered by
whichever target the browser has in front.

**Idle** — what a background tab is, precisely: it is read once when it
opens, so its title is real and the first switch to it is a repaint, and
after that it re-extracts only while focused. A dirty signal for a background
tab sets a flag that is spent when focus arrives.

**Snapshot** — the open tabs on their way to or from disk: a URL, a title and
a scroll offset each, plus which one was in front. Not called a session,
because `Session` already names the state machine and `wwt-cdp` already calls
an attached target a session id. `wwt::store::Snapshot`.

**Degraded** — a tab whose injected script threw, and which is therefore read
by `DOMSnapshot` instead. Sticky until the tab navigates, because a new
document reinstalls the script. It keeps runs, hints, scrolling and input; it
loses the caret, wrapping inside a control, and the occlusion test that keeps
a label off a covered link.

**Source** — which way a page is read: `Script` or `Snapshot`. Named by the
effect rather than chosen by the page, so the rule about when to reach for the
second one is a decision `Session` makes and a test can exercise with no
browser. `Source::Snapshot` is `DOMSnapshot.captureSnapshot` and has nothing
to do with the `Snapshot` above, which is the session file.

**Samples** — a picture as colours, one per half cell, so `rows` is twice the
cell rows it covers. What pixel mode composes to on a terminal with no
graphics protocol. `wwt_frame::Samples`.

**Half-block** — a cell showing `▀` with the top sample as its foreground and
the bottom as its background. Two colours in one cell, which is the whole
reason `Style` has a background at all.

**Action** — what a key means, given a mode. The whole keyboard is one
table, `keymap::action_for(mode, key, vp)`, pure and total.

**Session** — every piece of state there is, and the only thing that mutates
it. Takes an **event**, returns **effects**, and composes a **frame**. It
reaches nothing: no page, no socket, no terminal, which is why every rule it
holds is testable without a browser. `wwt::session::Session`.

**Event** — something that happened: a key, a mouse event, a resize, a dirty
signal, a finished job.

**Effect** — something the session wants done: extract, query hints, scroll,
navigate, send input, blur, resize the page, capture the mouse, quit. One
vocabulary, so the loop has one place that spawns.

**Job** — the result of something that ran off the loop's thread, on its way
back in as an event.

**Core** — the adapter. Turns tokio into events and effects into spawns, and
decides nothing. `wwt::core::Core`.

**Input** — one key or one click, as a thing to send. The vocabulary, so it
lives beside the shapes it wraps in `wwt_page`, not beside the pump.
`wwt_page::Input`.

**Input pump** — the one task that delivers them in order. Everything else
about a page is idempotent or self-cancelling and is allowed to race; three
keys as three tasks would sometimes type `acb`.

## The browser we drive

**Client** — the hand-rolled CDP connection: one websocket, request and
response correlated by id, events broadcast to subscribers. **Session id**
identifies one attached target on it; every command a page issues is
`call_on` its own.

**Profile** — Chromium's `--user-data-dir`, persistent at
`$XDG_DATA_HOME/wwt/profile`. The cookie jar that makes logins durable, and
the lock: Chromium refuses one another Chromium holds, so a second wwt gets a
temporary profile and writes no session file. The instance holding the
profile owns that file.

**Adoption** — taking over a target a page opened for itself, reported by
**auto-attach** and told apart from one we created by its `openerId`. Its
document has usually already run by the time we hear about it, so `adopt`
registers the bootstrap for the tab's next document and evaluates it into the
one already there.

**Bootstrap** — `crates/wwt-page/assets/bootstrap.js`, installed once per
document so it survives navigation. **`__wwt.__pure`** is its arithmetic
half, exposed so it can be asserted on with data rather than through a
rendered page.

**Mirror** — a hidden div a form control is copied into, with its font and
content width, because there is no `Range` inside an `input` and a control's
value is not in the DOM at all. Costs a layout, so only controls that need
one get one.

**Harness** — in tests, the shared browser and the right to be the only test
using it. `crates/wwt-page/tests/common`.
