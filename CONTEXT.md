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

**Page viewport** — the terminal grid less the row the chrome occupies.
Chromium is told this is the whole window, so the page does not know the
statusline exists. `session::page_viewport`.

**Frame** — a full grid of cells, plus where the terminal's cursor belongs.
The single output type every rendering mode produces, so text mode and later
pixel and reader modes cannot diverge in how they reach the screen.
`wwt_frame::Frame`.

**Chrome** — the bottom row: the statusline, or the `:` line when one is
open. **State** is what it says about the page — loading, ready, an error, a
notice — and it is never a reason to blank the frame.

## What the browser is doing

**Mode** — what keys mean right now: normal, insert, hint, or the `:` line.
Changes only in response to a keystroke; a page cannot move you between
modes, which is what makes handing it the keyboard safe. `wwt_ui::Mode`.

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
