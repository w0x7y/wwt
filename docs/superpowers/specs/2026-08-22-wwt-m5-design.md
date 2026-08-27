# wwt M5 — Pixel mode

**Date:** 2026-08-22
**Status:** Approved, pre-implementation
**Parent spec:** `2026-08-19-wwt-design.md` (sections 3, 7 and 8 govern here).

This is a delta against the system design, not a replacement for it. Where the two
disagree the parent spec wins and this document is wrong, except for the amendments in
section 9, which change the parent spec itself.

## 1. What M5 delivers

M4 made a browser you keep open. M5 makes it one you can trust about what a page
actually looks like. Text mode reconstructs a page from its runs, which is right almost
always and cannot be right for a chart, a map, a photograph or a CAPTCHA. Pixel mode is
the answer to "but what does it really look like", and it is the reason the coordinate
model was built the way it was: the same viewport, the same scroll offset, the same
focus, painted a second way.

At the end of M5, `p` swaps the page between text and true pixels without moving it,
and `f` still labels every link with the pixels underneath.

### In scope

`Page.startScreencast` on the focused target, the Kitty graphics protocol with unicode
placeholders, one-shot capability detection, the `p` toggle and `:set pixel on|off`,
overlays that survive over an image, and the resize and tab-switch paths.

### Out of scope

The half-block degradation path, deferred to M6 by amendment 1 in section 9. Reader
mode, the reflow renderer and the `DOMSnapshot` fallback extractor, which are M6.
Eviction, which is M7. Also out: per-tab pixel mode, a saved pixel preference, image
scaling or zoom independent of the cell size, cropping to a region, and any second
graphics protocol (sixel, iTerm2).

## 2. Architecture

The seam is unchanged and so is the loop. `Session` still owns every piece of state,
still reaches nothing, and still answers `on(Event) -> Vec<Effect>` and `compose() ->
Frame`. `Core` is still the adapter that decides nothing. What M5 adds is a second kind
of thing a frame can carry, and a renderer that knows one protocol.

### The bytes never leave base64

This is the decision the milestone is cheap because of. `Page.screencastFrame` carries
the image as base64-encoded PNG. The Kitty graphics protocol carries an image as
base64-encoded PNG. So a frame is forwarded from the websocket to stdout as the string
it arrived as: chunked, never decoded, never re-encoded, never held as pixels.

wwt therefore gains no image decoder and no dependency, and the fixed set in
`Cargo.toml` stands. It is also why half-block left this milestone: half a cell needs a
foreground and a background colour, which needs real samples, which needs an inflate
and an unfilter that nothing else here would ever use.

### The grid wins over the image

Unicode placeholders mean placement *is* cell content: a cell holding the placeholder
character shows the image behind it, and a cell holding a glyph shows the glyph. So the
rule is one sentence, and hint labels over a pixel page cost nothing to arrange.

`Frame` gains an image; `Cell` does not change. The renderer synthesizes placeholder
cells as it writes and owns every byte of the protocol, so `wwt-frame` stays what it is:
coordinate math, cells and painting, with no I/O and no dependencies. A `Cell` that had
to carry combining diacritics would put a terminal protocol inside the one crate whose
hard rule is that it knows about nothing.

### Rejected alternatives

**A pixel path that bypasses `Frame`.** Screencast events drive a second renderer
directly, and pixel mode never composes a frame at all. It saves moving a string, and it
costs the property `CLAUDE.md` states outright: `Frame` is the single output type every
rendering mode produces, so modes cannot diverge in how they reach the screen. Two paths
to stdout is two places to get the cursor, the chrome and the resize wrong.

**Placeholders in the cell grid.** `Cell` carries the placeholder character and its
diacritics, and the existing differ places the image for free. It is genuinely elegant
and it is wrong twice: a cell would have to hold up to three codepoints where it holds
one, and `wwt-frame` would have to know what a Kitty diacritic is.

**Per-tab pixel mode.** Rejected in favour of a global toggle. Only the focused tab
screencasts either way, so per-tab buys a preference rather than a cost, and it would
have to be remembered in the session file, which is a snapshot version bump and a
rejected file for everyone who upgrades. A single flag on `Session` is the whole state.

**JPEG instead of PNG.** Smaller frames and a quality knob, and it would mean a lossy
picture of text, which is the one thing a browser in a terminal must not blur. PNG also
happens to be the format both ends already speak, so the choice costs nothing.

### Crate deltas

| Crate | Delta |
|---|---|
| `wwt-frame` | `Frame` gains `image: Option<Image>` and `Image { generation, payload, area }`. No protocol, no I/O. |
| `wwt-cdp` | Nothing. `call_on` and `subscribe` already carry a screencast. |
| `wwt-page` | `start_screencast`, `stop_screencast`, `ack_frame`, and the frame event reader. |
| `wwt-term` | Capability detection, the graphics protocol, placeholder emission, image diffing. |
| `wwt-ui` | `p` in the key table, `:set pixel on\|off`, and what the statusline says about the mode. |
| `wwt` | `Session::pixel`, `Effect::StartScreencast`/`StopScreencast`/`AckFrame`, `Job::Frame`. |

## 3. The screencast

`Page.startScreencast` on the focused target's session, with `format: "png"` and
`maxWidth`/`maxHeight` set to the CSS viewport the page already has, so nothing is
scaled on the way out. It starts when pixel mode is entered and on every switch into a
tab while pixel mode is on, and stops on the way out of both.

Only the focused tab screencasts. This is the same rule as extraction and for the same
reason: a background tab is idle, and an idle tab costs nothing.

**Every frame is acked or the frames stop.** `Page.screencastFrame` carries an integer
the ack must quote back. It is called `sessionId` in the protocol and it is not a CDP
session id; this document and the code call it the **ack id**, because `wwt-cdp` already
means something else by session and a third meaning would make the glossary useless.

**At most one frame is ever in flight, so nothing coalesces.** Chromium sends the next
frame only once the last is acked, which means the ack is the backpressure and there is
no queue to drain. A buffer here would be machinery guarding a case the protocol already
prevents. If a page ever does outrun the loop, the knob is `everyNthFrame` on the
screencast rather than a buffer: a frame we have already been sent has cost us the frame
either way.

**A frame is acked whatever becomes of it.** One that arrives for a tab you have
switched away from, or after pixel mode was left, is answered and discarded. Chromium
counts acks and not paints, so a frame dropped without one stops the screencast, and it
shows up later as a picture that never moves rather than as an error.

**An idle page still costs nothing.** Chromium emits a screencast frame when the page
paints and not on a clock, so the rule that an idle page costs ~zero CPU survives pixel
mode, and no tick loop appears anywhere.

**The screencast frame rate is set by holding the ack back.** Chromium sends the next
frame only once the last is answered, so `FRAME_INTERVAL` caps pixel output at thirty
frames per second with the protocol's own flow control. Nothing polls, nothing is
buffered, and a still page produces no frames and pays nothing.

The browser itself retains normal begin-frame pacing. The later YouTube SPA regression
showed that M2's `--disable-frame-rate-limit` could starve application work even while
video and screencast frames continued. `--disable-gpu-vsync` now removes only the
presentation wait needed for low-latency scrolling; the ack remains necessary because
native sixty-frame pacing is still faster than a terminal needs to decode full-page PNGs.

## 4. The image on the way out

Three escape sequences and one rule.

**Two image ids, alternating.** A frame is transmitted with `a=t` into whichever id is
*not* on screen, then placed with `a=p,U=1,p=1`, then the cells are pointed at it, and
only then is the other id deleted. Chunks are 4096 bytes of payload, `f=100` is PNG, `t=d`
puts the payload in the escape, and `q=2` goes on every sequence, suppressing both the
success and the error replies: a terminal answering into our stdin is a keystroke we
would have to know to throw away.

The second id is the part that had to be measured rather than reasoned about, and it took
three attempts. Transmitting to an id tears down its placement for as long as the
transmission lasts. With a small picture that arrives in one sequence the window is
nothing and everything looks correct, which is exactly what a probe with a test image
shows. With a real page, a few hundred kilobytes chunked into dozens of sequences, the
window is the whole transmission, and the cells on screen spend it addressing a placement
that is not there: the picture drops to the terminal's background and the placeholders
show as missing glyphs. That is the flicker. Sent to the other id instead, what is on
screen is untouched until the new picture is complete.

**Paint.** The page area is filled with U+10EEEE, the foreground colour carrying the id of
the image the cell belongs to, and **every cell carrying its own row and column** as
combining diacritics from the protocol's table.

Addressing only the first cell of each row and letting the rest continue from it is
smaller and is wrong. A cell with no diacritics continues from the cell before it, so a
hint label painted into the middle of a row orphans every placeholder after it and the
picture tears from the label to the right edge. Overlays are the whole reason this design
uses placeholders rather than placing the image directly, so surviving one is the
requirement, not an optimisation to trade against.

**A frame rewrites the cells, and that is the price of not flickering.** An earlier
version of this section claimed the opposite, on the reasoning that a fixed id lets the
placeholders on screen re-render against new data. They do, and doing it that way
flickers. Because a cell says which image it belongs to in its foreground colour,
alternating ids means repointing every cell the picture shows through: about ten bytes a
cell, so roughly fifty kilobytes for a full screen, against a few hundred kilobytes of
PNG for the same frame. It is the smaller half of what a frame costs, and it buys a
picture that does not blink.

The cells are still not touched by a scroll that produces no new frame, and a still page
produces no frames at all.

## 5. Detection and what happens without it

**Asked once, with our own timeout.** At startup, after the cell-size probe and before
the first paint, wwt sends the graphics query and waits ~100ms for a reply. A reply is
support; silence is not. This is the one shape of terminal question this codebase
accepts: `CLAUDE.md` rejects `supports_keyboard_enhancement` because it takes stdin for
up to two seconds on every run, and the objection is the two seconds and whose the
timeout is, not the asking. Environment sniffing was rejected as a list that is wrong
through tmux and ssh in both directions.

**Without graphics, `p` is a notice and nothing else.** The statusline says pixel mode
needs a terminal that can show images, the mode does not change, and the frame you were
looking at is still the frame you are looking at. Guessing and emitting the escapes
anyway would spray a terminal that cannot read them with garbage, which is the one thing
section 8 of the parent spec forbids. Half-block would have been the third answer here
and is M6's.

**Amended by M6, 2026-08-23.** The notice was explicitly the answer "until M6", and
M6 arrived: `p` on a terminal with no graphics protocol now shows the page in
half-block colour, at half the vertical resolution and in the same place at the same
scroll offset, and never refuses. Nothing else in this section changes — the detection
is the same probe answered at the same moment, and what it answers now decides which
picture rather than whether there is one. See section 6 of
`2026-08-23-wwt-m6-design.md`.

## 6. Compose

In pixel mode `Session::compose` paints the tab bar and the statusline exactly as it
does now, leaves the page rows blank, and attaches the image with the page area as its
rect. Runs are not painted. Everything else about the frame is unchanged, which is what
makes the toggle instant: the same state composes both ways.

Overlays are painted after and win, by the rule in section 2. That is hint labels today
and whatever M6 and M7 put on top of a page tomorrow.

The caret is not one of them. `Frame::cursor` is still set only in insert mode and still
by `compose` alone, and in pixel mode the page draws its own caret into the picture, so
placing ours on top of it would be two carets disagreeing. Insert mode in pixel mode
therefore shows the page's caret and not the terminal's.

## 7. Keys, commands and the statusline

`p` toggles pixel mode, in normal mode only, and is the only new key. `:set pixel on`
and `:set pixel off` do the same thing from the command line, beside `:set mouse`.

The statusline says `pixel` while the mode is on. It says nothing while it is off,
because text is the default and a statusline that names the normal case wastes the row.

The mode is not written to the session file. Text is what wwt is, pixel mode is a thing
you ask for, and a new field in `Snapshot` is a version bump that costs every existing
session file its tabs on upgrade.

## 8. Tabs, resize, and what a switch costs

**A switch stops one screencast and starts another.** M4's rule that switching activates
the target already holds, and the screencast needs it for the same reason the mouse did.

**A switch in pixel mode is not a repaint.** M4's guarantee is that a switch paints from
the cached runs and costs no round trip, and `measure_switch` holds it down. That
guarantee is text mode's. In pixel mode the new tab's first frame is a round trip, and
until it lands the previous image stays on screen with the new tab's chrome around it,
because the alternative is blanking the frame you are looking at. The spec says so
rather than letting the measurement quietly become a lie.

**A resize is a stop, a restart and a repaint.** The debounced resize path already
recomputes the grid and pushes new device metrics; pixel mode adds stopping the
screencast, deleting the image, restarting at the new size, and writing placeholders
again. `Renderer::invalidate` already covers the cell side.

**Leaving pixel mode deletes the image.** `a=d,d=i,i=<id>`, so nothing is left behind in
the terminal's memory for a mode nobody is in.

## 9. Amendments to the parent spec

These change `2026-08-19-wwt-design.md` and land in the same commit as this document.

1. **Section 11, M5 and M6.** The half-block degradation path moves from M5 to M6. M5 is
   the screencast, the graphics protocol and the toggle; the fallback picture belongs
   beside the reflow renderer and the fallback extractor, which are the milestone about
   what wwt does when it cannot have what it wants. Without Kitty graphics, M5's pixel
   mode is a notice rather than a worse picture.
2. **Section 7, tabs.** "Only the foreground tab holds an active extraction subscription
   and screencast" is unchanged in rule and gains its cost: a switch in pixel mode is a
   round trip for the first frame, and M4's repaint guarantee is text mode's.
3. **The Frame, section on the central type.** "An optional pixel buffer for regions
   rendered as graphics" becomes an optional image for the page viewport. Pixel mode is
   the whole viewport and never a region, and the buffer is base64 on its way through
   rather than pixels.
4. **Section 3 and the performance notes.** Chromium begin frames remain paced,
   presentation skips vblank, and screencast delivery is capped by acknowledgements,
   per section 3 above.

## 10. Failure modes

Section 8 of the parent spec holds throughout: never blank the frame you are looking at.

| Failure | Behaviour |
|---|---|
| The terminal cannot do graphics | `p` is a notice, the mode does not change, the frame stands. **Amended by M6: half-block colour instead.** |
| The detection query is answered late | Treated as unsupported. A reply arriving after the window is discarded rather than read as a key. |
| `Page.startScreencast` fails | A notice, and the mode falls back to text. The page is untouched. |
| A frame arrives for a tab that is not focused | Acked and dropped. Focus moved while it was in flight. |
| A frame arrives for a tab that is gone | Dropped, like every other job naming a closed tab. |
| Writing the image to stdout fails | The same failure any render has: reported, and the next frame tries again. |
| A frame arrives while pixel mode is off | Acked and dropped. Stopping is not instant, and one frame in flight is the most there can be. |
| The terminal is resized mid-transmission | The chunks in flight finish, the image is deleted and retransmitted at the new size. |

## 11. Testing

The split is the one the repo already has: what can be tested without a browser or a
terminal is, and what cannot is honest about needing one.

- **`wwt-frame`**: an image survives a compose, and a frame with an image still answers
  every existing property. Pure.
- **`wwt-term`**: the protocol, with data. Chunking at the 4096 boundary and at exact
  multiples of it, the diacritic table, placeholder rows that omit what they may omit,
  the id in the foreground bytes, and the delete sequence. No terminal: these are
  functions from a payload and a rect to bytes, and that is the point of writing them
  that way.
- **`wwt-page`**: a real Chromium, in `tests/`. A screencast starts, a frame arrives, an
  ack keeps them arriving, and a stop stops them.
- **`wwt`**: the session's rules. `p` toggles, `p` without graphics does not, a frame for
  the wrong tab is dropped, a switch stops and starts, and compose paints no runs in
  pixel mode. No browser, because these are decisions.
- **The measurement**: `measure_pixel_frame`, printing what a frame costs from event to
  bytes written, and the CPU comparison section 3 owes.

## 12. Open questions

1. ~~**Re-transmission to a live image id.**~~ **Closed, 2026-08-22.** Measured against
   a real Kitty: transmitting to an id that already has a virtual placement destroys the
   placement. The picture disappears and the cells addressing it show background, which
   is a more visible failure than a stale picture and so at least fails loudly. Re-issuing
   `a=p` after each transmission restores it with no cell rewritten. A frame is therefore
   two sequences and no repaint, and section 4 says so.
2. **Snapshot version strictness.** `store::load` now rejects any version that is not
   ours, which is right for a file from the future and heavy-handed for a field added to
   an old one. M5 does not touch the snapshot, so it does not have to answer this; the
   milestone that first adds a field does, and the answer is probably to accept anything
   at or below `VERSION` and let serde default what is missing.
3. ~~**Animated pages.**~~ **Closed, 2026-08-22.** It needed turning, and observation
   answered it before any measurement did: on a page that animates, an uncapped
   compositor produces frames faster than the terminal can decode a full-page PNG, and
   the picture visibly flickers. A still page is steady, which is what told us the rate
   was the cause.

   The knob is not `everyNthFrame`, which is a fraction of an unbounded rate and so is
   still unbounded. It is the ack: Chromium sends the next frame only once the last is
   answered, so holding the ack back for `FRAME_INTERVAL` sets the rate using the
   protocol's own flow control, with nothing polling and nothing buffered.
   This originally kept `--disable-frame-rate-limit`. A later live YouTube SPA
   reproduction superseded that decision: unbounded begin frames stopped YouTube's app
   shell while its video kept advancing. The launch policy now uses
   `--disable-gpu-vsync`, and the ack still caps screencast delivery independently.
