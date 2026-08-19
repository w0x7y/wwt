# wwt M3 — Interaction

**Date:** 2026-08-19
**Status:** Approved, pre-implementation
**Parent spec:** `2026-08-19-wwt-design.md` (sections 4, 6 and 8 govern here).

This is a delta against the system design, not a replacement for it. Where the two
disagree the parent spec wins and this document is wrong, except for the three
amendments in section 8, which change the parent spec itself.

## 1. What M3 delivers

M2 made a browser you can read with. M3 makes one you can use: keys reach the page,
links and buttons are reachable without a mouse, forms accept text, and a click lands
where you pointed.

At the end of M3, `wwt duckduckgo.com` can search, follow a result, and fill in a
login form. It still holds one page; tabs and the persistent profile are M4.

### In scope

The static key table and `Input.dispatchKeyEvent`, insert mode, hint mode with an
on-demand target query, mouse press and wheel dispatch, the `wwt-ui` crate, and the
four-mode state machine.

### Out of scope

Tabs, `F`-to-new-tab, sessions, pixel mode, reader mode, `:set zoom`, the
accessibility-tree fallback, and the Chromium supervisor. Deliberate limitations M3
does not lift: no background colors, no images, no text selection inside the page. New
limitations M3 accepts: no IME or multi-byte composition, no drag, no hover, no
context menu.

## 2. Architecture

The loop is unchanged. `Core` still owns all state, still mutates it alone, and still
spawns page operations rather than awaiting them inline. M3 adds two things to that
shape: a wider `Mode` enum, and an ordered input pump.

### The input pump

Every page operation so far has been idempotent or self-cancelling, so spawning each
one independently was safe. Keystrokes are neither. Three keys pressed quickly would
become three tasks racing to reach Chromium, and `abc` would sometimes arrive as
`acb`.

So input does not spawn per event. `Core` holds the sending half of an unbounded
channel whose receiver belongs to one long-lived task per page. That task awaits each
dispatch before taking the next message, which makes ordering a property of the
channel rather than of task scheduling. The loop still never blocks: sending to an
unbounded channel does not await.

Mouse events ride the same channel, so a click and the keys typed after it cannot
swap places.

### Crate deltas

| Crate | Change |
|---|---|
| `wwt-frame` | `HintTarget` and `TargetKind`, alongside `TextRun`. Still no I/O, still no dependencies. |
| `wwt-cdp` | None. |
| `wwt-page` | `dispatch_key`, `dispatch_mouse`, `hints`, `blur`. `hints()` in the injected script. |
| `wwt-term` | None. crossterm's event stream already yields mouse events once capture is on. |
| `wwt-ui` | **New.** `mode.rs`, `chrome.rs` and `command.rs` moved from `wwt`, and `hint.rs`. |
| `wwt` | `keys.rs` (the key table), the input pump, mode routing in `core.rs`. |

### `wwt-ui`

M2 deferred this crate on the grounds that a boundary drawn before its second consumer
exists is a guess. M3 is where the guess resolves: the mode machine, the hint overlay
and the chrome are one concern, they are all pure functions over a `Frame`, and none
of them should be able to touch a page. The crate depends on `wwt-frame` only, which
is what makes every mode transition, every label assignment and every painted overlay
testable with no browser in the loop.

`HintTarget` lives in `wwt-frame` rather than `wwt-page` for the reason `TextRun`
does: it is a geometric value that the page produces and the painter consumes, and
putting it anywhere else would force `wwt-ui` to depend on `wwt-page`. `wwt-page`
deserializes its own raw shape and converts, exactly as it already does for runs.

### Alternatives rejected

**The key table in `wwt-term`.** The parent spec's component table assigns "key/mouse
decoding" there, and `wwt-term` already owns crossterm. But the table's output is a
CDP payload owned by `wwt-page`, so putting the table in `wwt-term` creates a
`wwt-term` to `wwt-page` edge pointing backwards through the layering. The table goes
in the binary, next to the keymap it sits beside conceptually. It is pure, so it is
unit-tested in `src/` without a browser.

**Activating hints with `element.click()`.** One round trip, no geometry, immune to
occlusion. Rejected because it produces an untrusted event that some sites ignore, it
skips the focus and hover side effects a real click has, and it would mean hint
activation and mouse clicking take different paths to the same outcome. Hints
dispatch a real click at the target's centre, which is the same call the mouse makes.

**A configurable keymap file.** Config is not specified anywhere yet, and inventing a
format here would fix an interface before there is a second consumer for it. The
keymap stays a static table.

## 3. Modes

```rust
pub enum Mode {
    Normal,
    Command(String),
    Insert,
    Hint(HintSession),
}
```

`Core` routes each key by mode and nothing else:

| Mode | Keys |
|---|---|
| `Normal` | The keymap table. `i` enters insert, `f` enters hint, `:` and `o` enter command. |
| `Command` | Unchanged from M2. |
| `Insert` | Everything is forwarded to the page. `Esc` blurs and returns to normal. `Ctrl-]` forwards a literal Escape. |
| `Hint` | Label characters filter, `Backspace` un-types one, `Esc` cancels, a unique match activates. |

Insert mode forwards keys; it does not require that a text field is focused. This
matters for applications with single-key shortcuts of their own: `i` is how you hand
the keyboard to the page, and `Esc` is how you take it back.

`Esc` is never forwarded. A page cannot trap the keyboard, which is the property that
makes handing it over safe in the first place. `Ctrl-]` exists for the pages that
genuinely want an Escape, a dropdown to dismiss or a dialog to close.

The escape hatch is `Ctrl-]` and not the obvious `Ctrl-[` because a terminal transmits
`Ctrl-[` as the byte `0x1B`, which is Escape: the two are the same keystroke on the
wire and no amount of decoding separates them. Disambiguating would mean asking for
the kitty keyboard protocol, which not every terminal implements. `Ctrl-]` is
`0x1D`, unambiguous everywhere, and unbound.

Mode changes only in response to a key. Hinting an editable target enters insert mode
because activating that hint was your action, not the page's. Clicking with the mouse
never changes mode, and the page can never change it at all.

## 4. Key dispatch

`Input.dispatchKeyEvent` wants `windowsVirtualKeyCode`, `code`, `key` and `text` to be
mutually consistent. Anything reading `e.code`, and every application-level keyboard
shortcut, misbehaves when they are not. `keys.rs` is a static table from a crossterm
`KeyEvent` to that quad plus a modifier mask (Alt 1, Ctrl 2, Meta 4, Shift 8).

Coverage: printable ASCII, `Enter`, `Tab`, `Backspace`, `Delete`, `Esc`, `Space`, the
four arrows, `Home`, `End`, `PageUp`, `PageDown`, `Insert`, and `F1` to `F12`.
Anything outside that set is dropped rather than guessed at.

| Key | vk | `code` | `key` | `text` |
|---|---|---|---|---|
| `a` | 65 | `KeyA` | `a` | `a` |
| `A` | 65 | `KeyA` | `A` | `A` |
| `1` | 49 | `Digit1` | `1` | `1` |
| `!` | 49 | `Digit1` | `!` | `!` |
| space | 32 | `Space` | ` ` | ` ` |
| `Enter` | 13 | `Enter` | `Enter` | `\r` |
| `Tab` | 9 | `Tab` | `Tab` | `\t` |
| `Backspace` | 8 | `Backspace` | `Backspace` | none |
| `Esc` | 27 | `Escape` | `Escape` | none |
| `Left` | 37 | `ArrowLeft` | `ArrowLeft` | none |
| `Ctrl-s` | 83 | `KeyS` | `s` | none |

Two rules carry most of the correctness:

**Ctrl or Meta suppresses `text`.** `Ctrl-s` must reach a page's save handler without
depositing an `s` in whatever box has focus. Shift does not suppress text; crossterm
has already applied it to the character.

**`code` and the virtual key code come from the unshifted key.** `!` is produced by
`Digit1`, so that is the code it reports, with `Shift` in the modifier mask. This is a
US-layout assumption and section 11 records its cost.

A key with text dispatches `keyDown` carrying `text` and `unmodifiedText`, then
`keyUp`. A key without text dispatches `rawKeyDown`, then `keyUp`. Chromium turns the
first into a character insertion and leaves the second as a bare key event, which is
the distinction between typing and pressing.

## 5. Hints

### Finding targets

`__wwt.hints()` is a second entry point on the injected script, run on demand rather
than during extraction. It collects `a[href]`, `button`, `input` other than hidden,
`select`, `textarea`, `contenteditable`, the interactive ARIA roles, and elements with
a non-negative `tabindex`. A candidate survives three filters:

1. It has a non-empty rect and is not hidden by `display`, `visibility` or `opacity`.
2. That rect intersects the viewport.
3. `document.elementFromPoint` at the rect's centre returns the element itself, a
   descendant of it, or an ancestor of it.

The third filter is what keeps a sticky header from handing you labels for the links
it covers. It costs one hit test per surviving candidate, which is why this query is
not part of extraction.

Each target returns its rect and a `kind` of `editable` (text inputs, textareas,
`contenteditable`) or `clickable` (everything else). The kind is the only thing that
decides which mode activation leaves you in.

### Why not during extraction

The parent spec's section 6 assumes hint boxes arrive with the runs and are therefore
nearly free. They do not: extraction walks text nodes and knows nothing about
interactive elements, so hints would add a `querySelectorAll`, a rect per candidate
and a hit test per candidate to a path that runs on every scroll frame and every
mutation. That path is scroll latency. Hints are pressed occasionally. The query runs
on `f`.

Results are cached and reused if hint mode is re-entered before the next dirty signal;
any dirty signal discards them. On a live page that means most `f` presses pay a round
trip, which is the right trade: the labels are then guaranteed to describe the page as
it is now.

A dirty signal arriving *while* labels are on screen does not disturb them. Labels
must not move under the keys you are typing. They may go stale, and `Esc` followed by
`f` is the fix.

### Labels

The alphabet is the home row and its neighbours: `sadfjklewcmpgh`. Labels are of
**uniform length**: the shortest length whose alphabet covers the target count, so 14
targets or fewer get one character and up to 196 get two. Uniform length makes the set
prefix-free, which means activation needs no timeout and no tie-break rule: the
moment what you have typed identifies one target, that target is clicked.

Labels are assigned in document order, so re-entering hint mode on an unchanged page
gives the same target the same label.

### Painting and activation

Labels paint after the page runs, at the top-left cell of each target's rect, clamped
into the grid, in bold reverse video. They cover the page text underneath, which is
what makes them readable and is undone the moment hint mode ends.

Typing filters locally against the cached targets and repaints. No round trip, no
Chromium involvement, no perceptible latency. A unique match dispatches a real click
at the target's centre and then enters insert mode for an `editable` target or returns
to normal for a `clickable` one. `Esc` cancels and paints nothing.

If the query returns no targets, hint mode is not entered: the statusline says so and
the mode stays normal. Entering a mode with nothing in it would only need escaping.

### Seeing what you type

A form control's value is not a text node. `extract()` walks text nodes, so
before M3 shipped typing there was nothing to reveal the gap: you could fill in
a form, submit it, and never see a character of what you had entered.

So extraction gains a second pass over `input`, `textarea` and `select`, reading
each control's rendered text from element state and placing it in the control's
content box. It reports what the browser shows rather than what the control
holds: a placeholder when the value is empty, the chosen option for a `select`,
bullets for a password, and nothing at all for a checkbox, whose value is the
string `on`.

This one is not on demand. It is part of every extraction, because the text in a
field is page content in the way a hint target is not: leaving it out of a pass
would blank text that is on screen.

### Measuring inside a control

Knowing *that* a control has text is not enough. Where the browser wrapped a
line, which part of a scrolled value is on screen, and where the insertion point
sits are all facts about character positions, and there is no `Range` inside an
`input` to ask.

So the script mirrors the control: an absolutely positioned, `visibility:
hidden` div carrying the control's own font, line height, wrapping rules and
content width. The same engine on the same inputs breaks the lines in the same
places, and the mirror *does* have a `Range`, so `linesOf` reads its line boxes
with the binary search it already uses for the page's text. Positions come back
relative to the mirror's origin and are translated into the control's content
box, less its scroll offsets.

`visibility: hidden` and not `display: none`: the latter would throw away the
boxes this exists to measure.

**The mirror is a DOM mutation, and the observer that signals dirtiness watches
the whole document.** Left alone, extraction would signal itself dirty, the core
would re-extract, and an idle page would spin forever, which is the one thing
the dirty-signal design exists to prevent. The measuring pass therefore
disconnects the observer, measures, removes the mirror, discards the records it
caused with `takeRecords`, and observes again. Extraction is synchronous and
JavaScript is single-threaded, so no genuine mutation can occur inside that
window. There is a test asserting that extracting a focused field produces no
dirty signal.

A mirror costs a layout, and extraction runs on every scroll frame, so only the
controls that need one get one: a `textarea`, which may wrap; a control whose
value overflows or is scrolled, which shows a window into itself rather than its
head; and the focused one, whose insertion point must be found. A plain field
showing all of its value takes the cheap path and costs nothing beyond the
styles already read.

### The caret

`Extraction` carries `caret: Option<CssRect>`, a zero-width box on the line the
insertion point is on, measured from the character beside `selectionStart`
rather than from a collapsed range, which browsers treat inconsistently at the
end of a line.

`Core` inverts that cell rather than overwriting it, so the character under the
caret stays readable. It paints **only in insert mode**: a page can focus a
field without being asked, and a caret in normal mode would promise that your
typing lands there when it does not.

The caret does not blink. Blinking needs a timer, and an idle page must cost
nothing.

Still uncovered: the caret in a `contenteditable`, which needs the Selection API
rather than a mirror. Its *text* already renders, because that content is real
text nodes.

## 6. Mouse

Terminal mouse capture is enabled in `main.rs` beside the alternate screen, and
released with it on exit, so the two cannot get out of step on an error path. `Core`
toggles it at runtime through the same writer it renders to. It costs the terminal's own text selection, which most terminals hand
back when shift is held, and `:set mouse off` turns it off for the ones that do not.

Handled: left button press and release, dispatched as `mousePressed` and
`mouseReleased` at the cell centre through `Viewport::to_css`, which already returns
centres for exactly this reason. Wheel up and down are dispatched as `mouseWheel` **at
the pointer's cell** rather than at the origin, so a nested scroller under the cursor
scrolls instead of the document.

Ignored: motion (hover would cost a round trip per reported frame), right button (no
context menu), and middle button (nothing to open a tab into until M4).

Clicks on the chrome row are swallowed. The page does not know that row exists and
must not receive coordinates from it.

## 7. Chrome

The statusline gains a mode indicator on the left, before the state tag: `-- INSERT --`
in insert mode, and in hint mode the typed prefix with the number of targets still
matching. Normal mode shows nothing, as now. The command line is unchanged.

## 8. Amendments to the parent spec

These change `2026-08-19-wwt-design.md` and land in the same commit as this document.

1. **Section 6, hints.** The sentence claiming hint boxes come from the same
   extraction and are therefore nearly free is removed. Hints are queried on demand
   and cached until the next dirty signal, for the reason in section 5 above.
2. **Section 6, modes.** The paragraph making mode track the page's reality through a
   `focusin` listener is removed. Mode changes only in response to a key. A page that
   autofocuses its search box does not take the keyboard; `i` gives it away and `Esc`
   takes it back.
3. **Section 11, M3.** "Page-driven focus tracking" is struck from the milestone. With
   mode manual, nothing needs to observe focus, and no listener is added to the
   injected script.

## 9. Failure modes

Section 8 of the parent spec holds throughout: never blank the frame you are looking
at. Every path below degrades to stale-but-labelled.

| Failure | Behaviour |
|---|---|
| `hints()` throws | `State::Error`, mode stays normal, frame untouched. |
| No targets found | Statusline says so, mode stays normal. |
| A key dispatch fails | `State::Error`. Mode is **not** changed: dropping you out of insert because one keystroke failed would lose the next one too. |
| A click dispatch fails | `State::Error`, mode unchanged. |
| `blur()` fails on `Esc` | Insert mode is left anyway. Taking the keyboard back is local and must not depend on the page. |
| The terminal refuses mouse capture | Reported in the statusline once; everything else continues. |
| The input pump's page operation hangs | Keys queue behind it. The loop stays responsive and the statusline still updates, which is the visible difference from blocking the loop. |

## 10. Testing

Per parent spec section 9, the subtle logic lives where it can be tested without a
browser.

| Crate | Tests |
|---|---|
| `wwt-frame` | `HintTarget` construction and rect arithmetic. |
| `wwt-ui` | Mode transitions from each mode, in particular that `Esc` reaches normal from all three. Label generation: uniform length, prefix-free, correct count at the alphabet boundary and one past it. Filtering: a prefix narrows the set, a unique prefix identifies one target, an impossible prefix matches none. Overlay painting: labels land on the target's top-left cell, clamp at the grid edge, and win the cell against page text. Statusline mode tags. `:set mouse on\|off` parsing. |
| `wwt` | The key table: quad consistency across the printable range, Ctrl and Meta suppressing text, shifted punctuation reporting the unshifted `code`, unbound keys dropped. Mode routing: `q` in insert mode types rather than quits. |
| `wwt-page` | Browser tests: type into an input and read the value back; press Enter in a form and observe the navigation; click a link and observe the navigation; `hints()` against a fixture with hidden, off-screen and occluded elements, asserting counts and kinds; a wheel event over a nested scroller moves the scroller and not the document. |

The hint query carries a measurement, not a test: the heavy fixture from M2 timed for
`hints()`, recorded in the plan, the way extraction was.

## 11. Open questions

**IME and multi-byte input.** crossterm reports composed characters as `Char`, so a
composed glyph types correctly, but there is no composition state, no candidate
window, and no way to correct mid-composition. Languages needing an IME are not usable
for text entry. This is out of scope for M3 and needs a decision before wwt claims to
be usable outside Latin scripts.

**Layout assumptions in the key table.** `code` and the virtual key code are derived
from a US layout. On other layouts the character typed is still correct, because
crossterm reports the produced character, but `e.code` will name the wrong physical
key and layout-sensitive shortcuts will land wrong. Fixing it needs the layout, which
the terminal does not report.

**Occlusion by the centre point alone.** One hit test at the rect centre misses the
case of a target whose centre is covered but whose edges are not, for instance a wide
link crossed by a floating element. Adding fallback points multiplies the query's
cost. Left as is until a real page demonstrates the problem.
