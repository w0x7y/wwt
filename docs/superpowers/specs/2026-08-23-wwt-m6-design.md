# wwt M6 — Degradation

**Date:** 2026-08-23
**Status:** Approved, pre-implementation
**Parent spec:** `2026-08-19-wwt-design.md` (sections 3, 8 and 11 govern here).

This is a delta against the system design, not a replacement for it. Where the two
disagree the parent spec wins and this document is wrong, except for the amendments in
section 9, which change the parent spec itself.

## 1. What M6 delivers

M5 made a browser that can show you what a page really looks like. M6 makes one that
keeps working when a piece of it does not. Two failures have been named since M1 and
answered with nothing: a page whose injected script throws, and a terminal that cannot
show a picture. Both currently end at a notice.

At the end of M6, a page that breaks `bootstrap.js` is still read, still scrolled and
still clickable, and `p` on a terminal with no graphics protocol shows the page in
half-block colour rather than refusing.

The two halves share no code. They are one milestone because they are one promise:
section 8's rule that every failure degrades to something worse rather than to nothing.

### In scope

`DOMSnapshot.captureSnapshot` as a second extraction source, the session rule that
reaches for it, hints from the same snapshot, the `[degraded]` tag, a `wwt-png` crate
that decodes what Chromium's screencast produces, half-block composition in
`wwt-frame`, a background colour on `Style`, and the reduced screencast size that
makes the decode cheap.

### Out of scope

Reader mode and the reflow renderer, which are M8 by amendment 3 in section 9. They
left this milestone with the open question in section 8 of the parent, which is now
closed: an extractor that returns geometry needs no reflow layer under it, so the
fallback stops being reader mode's second consumer and reader mode stops blocking it.

The Chromium supervisor, per-command deadlines, crash recovery and tab eviction, all
M7. Also out: a colour-capability probe (see amendment 2), any second graphics
protocol, JPEG, decoding anything Chromium's screencast cannot emit, a caret on a
degraded tab, and a user-facing switch to force either path.

## 2. Architecture

The seam is unchanged and so is the loop. `Session` owns every piece of state, reaches
nothing, and answers `on(Event) -> Vec<Effect>` and `compose() -> Frame`. `Core`
decides nothing. What M6 adds is a second way to answer two existing effects, and a
second way to compose a picture.

### Reading a page is now a question with two answers

`Effect::Extract` and `Effect::Hints` gain a `Source`, which is `Script` or
`Snapshot`. The effect says which way to read, so the page does not guess and the
session's rule about when to degrade is written where a test can reach it without a
browser.

`Job::Extracted` changes shape to carry a `Result`, for the reason M5 gave for
`Job::Hints`: it is the only thing that clears `extracting`, so there must be exactly
one place that cannot forget the extraction is over. Today an extraction failure
arrives as `Job::Failed`, which is also what a scroll and a viewport change report, so
the session cannot tell which of them failed and therefore cannot answer a failed
extraction differently from a failed scroll.

### A degraded picture is cells, not an image

Half-block needs no graphics protocol. A page area of `▀` cells with a foreground and
a background colour is ordinary cell content, so the diffing renderer, the overlay
rules and the cursor rules all apply unchanged, and a hint label over a half-block
page is a cell that differs. M5 spent a section of its spec on making the grid win
over the image; here the grid is all there is.

The decode happens once per frame, in `Session::on_frame`, and never in `compose`.
Composing is what a hint label, a mode change and a statusline update each cost, and a
picture decoded there would be decoded again for every one of them.

`on_frame` rather than a spawned task because there is no spawned task to put it in: a
picture arrives on the CDP arm of the loop's `select!` as `Event::Frame`, straight off
the websocket, and never as a `Job`. So the choice is the loop's thread or a new hop
through the result channel, and the loop wins on the numbers. A frame is paced at
`FRAME_INTERVAL`, 33ms, and the picture to decode is a few thousand pixels rather than
a megapixel, so the decode is a fraction of a millisecond against an interval it has
all of. Text mode produces no frames at all and pays none of it.

It also keeps the seam where M5 left it. The decode is pure arithmetic over bytes with
no I/O, which is the same thing `paint_runs` is, and `Session` already owns what a
picture is. A test feeds a `ScreencastFrame` carrying a fixture PNG and asserts on the
cells that compose out, with no browser and no terminal anywhere near it.

### Crate deltas

| Crate | M6 delta |
|---|---|
| `wwt-png` | New. Base64, IHDR, IDAT, inflate, unfilter. **No I/O, no dependencies**, the same hard rule as `wwt-frame`, and no knowledge of terminals, cells or pages. |
| `wwt-frame` | `Style::bg`, and `Frame::paint_samples`, which turns a sample grid into half-block cells. |
| `wwt-page` | `snapshot()` beside `extract()`, and a screencast size that depends on what the terminal can show. |
| `wwt-term` | The renderer emits a background colour when a cell has one. |
| `wwt-ui` | The `[degraded]` tag. |
| `wwt` | `Source` on two effects, the degrade rule, and a `Picture` that is either bytes or samples. |

No new entry in the workspace dependency set. If one seems necessary the milestone has
taken a wrong turn, exactly as in M5.

## 3. The fallback extractor

### Why `DOMSnapshot` and not the accessibility tree

Section 8 of the parent left this open and asked that it be settled before M6 was
planned. It is settled: `DOMSnapshot.captureSnapshot`.

`Accessibility.getFullAXTree` was the original choice and cannot work here. An
`AXNode` carries no geometry, so an AX-sourced view can only be a reflowed linear
document, which means building the reflow renderer first and welding the fallback to
reader mode. `DOMSnapshot` meets the same independence requirement, since it shares no
code with our script and a bug in one cannot reach the other, and it returns layout
geometry in CSS pixels. So it feeds the renderer that already exists, and the
milestone shrinks to the half of it that is worth having soon.

### What the query asks for

    DOMSnapshot.captureSnapshot({
      computedStyles: ["color", "font-weight", "font-size", "visibility"],
      includePaintOrder: true,
      includeDOMRects: false,
    })

Four computed styles and no more. Two of them are the style: a run's `Style` is a
foreground colour and a bold flag and nothing else. Reverse belongs to the chrome, and
the background colour section 6 adds is half-block's, never a run's. There is no italic
in a `Style`, so asking for `font-style` would be asking for something nothing can
paint.

The other two are not style at all, and the probe is what added them. `font-size` is
arithmetic: the baseline rule below subtracts a fraction of it, so without it the
fallback cannot put a run in the row the script would. `visibility` is culling: a
snapshot reports a `visibility: hidden` text box with ordinary non-empty bounds, where
the script drops it, so a fallback that does not ask would paint text the browser does
not show.

`includePaintOrder` is what fills `TextRun::z`, which the painter's algorithm in
`paint_run` needs to resolve a contested cell.

### What it converts to

The same `Extraction` the script returns, so nothing downstream can tell the
difference except by the tag on the statusline:

- **Runs** come from `textBoxes`, one per inline text box, sliced out of the layout
  node's text by `start` and `length`. This is per-line geometry from the browser
  itself, so the fallback needs no line splitting, no `getClientRects` and none of the
  binary search the script does. The one thing our script does better is nothing, here.
- **Coordinates are document-relative and ours are viewport-relative**, so every rect
  has `scrollOffsetX`/`scrollOffsetY` subtracted. Getting this wrong is a page that
  looks right at the top of a document and drifts as you scroll, which is why it is
  written down.
- **The baseline** is the script's rule applied to the snapshot's box:
  `bounds.y + bounds.height - font_size * 0.21`, the `DESCENDER` constant
  `bootstrap.js` states once. The probe settled why it can be the same rule: a text box
  is the tight box and it is the *same* box, to the last fraction of a pixel, that the
  script gets from `getClientRects`. Anything else here is a fallback that reads the
  page correctly and paints it a row off.
- **Style** comes from `layout.styles`, positionally: one entry per property asked for,
  in the order asked for.
  Bold is `font-weight >= 600`, which is what the script already uses.
- **Culling is ours**, and there are two kinds. The snapshot is the whole document, so
  runs outside the viewport are dropped on our side; see section 11, where this is the
  fallback's real cost, accepted rather than solved. And a hidden run is culled by its
  `visibility`, since a snapshot reports one and the script never sees one.
  `display: none` needs no rule: it has no layout node to report.
- **Title, URL and scroll geometry** come from the document itself: `title`,
  `documentURL`, `scrollOffsetY` and `contentHeight`. The viewport height is ours
  already. So the statusline still costs no extra call, which was M1's rule.
- **Field values** come from `nodes.inputValue` and `nodes.textValue`, painted into
  the control's own box and elided at its width. A password shows bullets and an empty
  field shows its placeholder from `nodes.attributes`, which is what the script does
  and what a browser shows.

### What it cannot do

**No caret.** Character positions inside a control come from the mirror, and the
mirror is script machinery. Insert mode still works, because keys are dispatched over
CDP and never through our script, but on a degraded tab you type without seeing where
the insertion point is. The statusline says the tab is degraded, which is the honest
version of this.

**No wrapping or scrolling inside a control**, for the same reason. A long value is
elided rather than wrapped.

**No occlusion test for hints.** The script hit-tests a candidate before labelling it,
so a covered link gets no label. A snapshot has no hit test, so a degraded page can
label something behind a modal. Accepted: a spurious label is a wasted keystroke, and
the alternative is a hit test per candidate, which is a round trip per candidate.

## 4. The rule

In `Session`, and therefore testable with no browser and no terminal:

- A `Source::Script` extraction that returns `Err` sets `tab.degraded` and emits
  `Effect::Extract(id, Source::Snapshot)`. **One retry, not a loop.**
- A `Source::Snapshot` extraction that returns `Err` is the end of the line:
  `State::Error`, and the frame you are looking at stands. That is section 8 of the
  parent, unchanged.
- A tab that is already degraded asks for `Source::Snapshot` directly. A page that
  breaks the script permanently therefore costs one round trip per scroll rather than
  a failed one and then a good one.
- **Navigation clears the flag.** A new document reinstalls `bootstrap.js` through
  `Page.addScriptToEvaluateOnNewDocument`, so the next page has done nothing to deserve
  the slow path. This is also the way back: reload a tab that degraded on a transient
  failure.
- **Hints follow the flag** rather than deciding anything. Degradation is decided by
  extraction, which runs on every dirty signal, so by the time `f` is pressed the flag
  already says which way to ask. A hint query that fails is a `Job::Hints` carrying an
  `Err`, exactly as today.

`tab.degraded` is a field on `Tab`, beside `dirty`, `extracting` and `navigating`. It
is not one of the three in-flight flags, so M4's rule about not setting a flag beside
an effect for a tab that has not opened does not apply to it.

## 5. The statusline

A `[degraded]` tag, beside `[pixel]` and produced the same way: a bool parameter to
`statusline`, not a `State` variant. `State::Notice` is cleared by the next successful
extraction, and on a degraded tab the next extraction succeeds every time, so a notice
would say this once and never again. The condition has to outlive the extraction that
reports it.

## 6. Half-block

### A sample grid, not an image

Without the Kitty graphics protocol, pixel mode composes the page area as `▀` cells:
the upper half-block glyph, the top sample as the foreground colour and the bottom
sample as the background. One cell is two samples, so the grid the picture needs is
`cols` by `2 · rows` of the page area.

Nothing about the graphics protocol is involved, so a hint label, an elided run and
the chrome rows all behave over half-block the way they behave over text.

### Chromium scales; we resample

`start_screencast` already passes `maxWidth` and `maxHeight`. On a terminal with no
graphics it asks for **twice the sample grid**, `2 · cols` by `4 · rows`, and the
frame that arrives is box-averaged down to exactly `cols` by `2 · rows`.

Twice, rather than exactly, because Chromium scales to fit inside both bounds while
preserving the source aspect ratio, and the sample grid's aspect is deliberately not
the source's: a half cell is roughly 9 by 10 CSS pixels, not square. Asking for exactly
the sample grid therefore returns a frame that is short on one axis, which is a
letterboxed picture rather than a page. Asking for twice guarantees the frame is at
least the sample grid on both axes for any cell aspect between 1:2 and 2:1, and the
resample is where the non-square cell is accounted for. The picture fills the page area
exactly.

The PNG that reaches us is a few kilobytes rather than the few hundred a full-page
frame costs, which is what makes decoding it in process reasonable at all. Half-block
also needs no cell pixel size, so it works on the terminals where `ws_xpixel` reports
zero and the cell size is a configured guess.

### `wwt-png`

A new crate with `wwt-frame`'s hard rule: no I/O, no dependencies. It parses IHDR,
concatenates IDAT, inflates, unfilters, and returns samples. That is all of it.

**It decodes what Chromium's screencast emits and refuses everything else**, and the
probe is what fixed that scope: 8-bit channels, colour type 2, compression method 0,
filter method 0, interlace method 0. Colour type 2 is RGB, with no alpha, which is the
one that matters downstream: a screencast frame is opaque, so a sample is three bytes
and there is no compositing question to answer. Colour type 6 is accepted too, since
the container work is already done and dropping a fourth byte is one line, but the
fixture is type 2 because that is what arrives. No
interlacing, no palettes, no 16-bit channels, no ancillary chunks it does not need. A
decoder that accepts what it will never be given is code no test covers, and a wrong
guess about a format is worse than an error: it puts a plausible wrong picture on
screen. Anything unexpected is an `Err`, and section 10 says what happens to it.

Inflate is the only sharp part: fixed and dynamic Huffman, stored blocks, and the
length and distance tables. It is pure arithmetic over bytes, so it is tested the way
`window.__wwt.__pure` is tested, against data and without a page. That is the whole
reason it is its own crate.

### `Style` gains a background

`Style::bg: Option<Rgb>`. The comment on `Style` already says there is no background
colour "yet" and why; half-block is the why. The renderer writes `\x1b[48;2;r;g;bm`
when a cell has one and resets when it does not, which is one branch in the single
style-writing path that `perf(term): share the payload and write a row at a time`
consolidated. Text mode never sets it, so text mode is unchanged byte for byte.

### The frame's two shapes

`Event::Frame` is unchanged: it carries the `ScreencastFrame` exactly as M5 defined it,
base64 and all. What changes is what `Session::picture` becomes, which is a `Picture`
that is either the `Image` M5 forwards or the `Samples` this milestone decodes.
`compose` sets an image on the frame for the first and paints cells for the second.

Everything else about pixel mode is M5's and is untouched: the ack, the pacing, the
rule that only the focused tab screencasts, the rule that every frame is acked
including the dropped ones, and what a switch costs.

**The base64 stops being opaque on this path.** M5's whole economy was that a payload
arrives encoded and leaves encoded; half-block has to look inside it, so `wwt-png`
decodes base64 as well as PNG. Both are pure byte arithmetic and both live in the same
crate for the same reason. The graphics path is unchanged and still never decodes
anything.

## 7. Detection and the key

`graphics::query` already answers once, before raw mode, in the one moment stdin
belongs to nobody. Its answer stops meaning "may pixel mode be entered" and starts
meaning "which way is it composed". Both answers are pixel mode.

`p` therefore never refuses. `:set pixel on|off` is unchanged, the statusline still
says `[pixel]`, and there is no second key and no second mode name: whether the picture
is true pixels or coloured blocks is a property of the terminal, not a decision the
user makes or a state they have to track.

## 8. Compose

In pixel mode, `compose` paints no runs, for the reason M5 gives: the picture is the
page, and runs underneath would show text through everything the picture does not
cover. Half-block covers the whole page area, so this is unchanged.

Order within a frame stays: page, then hint labels, then chrome. A label is a cell that
wins over a half-block cell the same way it wins over a placeholder.

## 9. Amendments to the parent spec

1. **Section 8, "Injected script throws".** The open question is closed:
   `DOMSnapshot.captureSnapshot`, for the reasons in section 3 here. The sentence
   coupling the fallback to reader mode goes with it.
2. **Section 8, "No Kitty graphics".** Two tiers, not three. The third, "labeled
   placeholder blocks", is deleted rather than deferred: the renderer writes
   `\x1b[38;2;r;g;bm` for every styled cell with no capability check at all, so a
   terminal that cannot do half-block cannot do text mode either and wwt was never
   usable on it. A tier below half-block is a fallback for a case that cannot arise.
   A colour-capability probe, if one is ever wanted, is a change to text mode first.
3. **Section 11, milestones.** M6 is degradation: the fallback extractor and the
   half-block path. M7 stays hardening. Reader mode and the reflow renderer become M8.
   Robustness comes before a new view because section 11 already says daily use begins
   at M4, and every milestone since has added something a crash now loses. Reader mode
   is the one remaining piece that is a nicety rather than a foundation, and nothing
   blocks it.
4. **M5's spec, section 5.** Its notice on a terminal with no graphics was explicitly
   the answer "until M6". It is now half-block.

## 10. Failure modes

Governing rule unchanged: never blank the frame you are looking at.

- **The script throws.** Section 4. The tab degrades, the snapshot answers, and the
  statusline says so.
- **The snapshot also fails.** `State::Error`, stale frame, and the tab stays
  switchable-away-from. There is no third source.
- **The PNG is not what `wwt-png` expects.** The frame is dropped, the previous picture
  stands, and a notice says the picture could not be read. **The ack is still sent**,
  because Chromium counts acks and not paints: a frame dropped without one stops the
  screencast, which shows up later as a picture that never moves. This is M5's rule and
  it now has a second way to be broken.
- **A degraded tab is closed or switched away from.** Nothing special: the flag is on
  the tab and goes with it.
- **The decode is slower than the frames arrive.** It cannot pile up: the ack is held
  for `FRAME_INTERVAL` and Chromium sends the next frame only once the last is
  answered, so decoding is inside that window rather than in a queue behind it.

## 11. What this costs, and the honest part

A snapshot is the whole document. `DOMSnapshot` has no viewport, so a degraded
extraction is O(document) where the script's is O(what is on screen), and the whole
reason extraction costs 4ms rather than 18ms is that the script learned to stop
measuring what nobody can see. On `heavy.html`, fifteen hundred paragraphs of which a
dozen are visible, the fallback pays for all fifteen hundred, in JSON, on every dirty
signal.

This is accepted, not solved. It is a fallback rather than a mode anyone chooses, and
a page that reaches it is a page that would otherwise show nothing. `measure_snapshot`
records the number beside `measure_extraction` so that the gap is a fact rather than a
guess, and section 12 keeps open whether it needs a cap.

Half-block costs the opposite of what pixel mode costs. The payload is a few kilobytes
rather than a few hundred, the decode is a few thousand pixels, and the cells are the
same cells any frame writes. `measure_halfblock_frame` records it beside
`measure_pixel_frame`.

## 12. Testing

- **`wwt-png`**: pure, against byte vectors, including PNGs Chromium itself produced,
  checked in as fixtures. Every filter type, a stored block, a dynamic Huffman block,
  and an error for each thing it refuses.
- **`wwt-frame`**: samples to half-block cells, including the odd row count where the
  bottom sample of the last cell falls outside the page area.
- **`wwt-page`**: `snapshot()` against real pages, asserting on `Extraction` and not on
  the wire shape. **The fidelity test is the interesting one**: on a page where the
  script works, extract both ways and assert the runs land on the same cells. That is
  what tells us the baseline rule and the scroll-offset subtraction are right, and it
  is what keeps the answer to open question 1 true rather than merely measured once.
- **`wwt`**: the degrade rule, the one retry, the sticky flag, the clear on navigation,
  and hints following the flag. No browser: these are decisions.
- **End to end**: a page painted a known solid colour, screencast at reduced size,
  decoded, and asserted to be that colour. It is the cheapest possible proof that the
  scaling, the decode and the resample agree with each other.
- **Arranging the failure**: a test overwrites `window.__wwt.extract` with a thrower
  through `Page::eval`, which is already behind `test-support`. Tests arrange with
  `eval` and assert on `extract`, so a change that leaves the DOM right and the
  extraction wrong still fails.
- **The measurements**: `measure_snapshot` and `measure_halfblock_frame`. M2's, M3's,
  M4's and M5's numbers must be unmoved, since nothing on the good path changes.

## 13. Open questions

1. ~~**The text box and the baseline.**~~ **Closed, 2026-08-23.** A `<p>` given
   `font: 16px/3 monospace`, so that its line box is 48px around 22px of text, was read
   both ways at once. The snapshot's text box is `[0, 56, 153.609375, 22]` and the
   script's rect is `x=0 y=56 w=153.609375 h=22`: not merely the tight box, the same
   box. So the baseline is the script's own rule, `bottom - font_size * 0.21`, which
   reproduces the script's answer exactly (74.64 for that paragraph, 36.28 for a 32px
   heading) rather than approximately. It forces `font-size` into the computed styles
   the query asks for, and the probe found `visibility` had to go in beside it, since
   a snapshot reports hidden text that the script never sees. Section 3 has both.
2. **Whether a full-document snapshot needs a cap.** Section 11 accepts the cost
   without bounding it. If `measure_snapshot` on `heavy.html` is bad enough to make a
   degraded tab unusable rather than merely slow, the answer is probably to stop
   converting once the visible rows are filled, since `textBoxes` arrive in document
   order. Decide with the number, not before it.
3. **Snapshot version strictness**, carried unchanged from M5's spec. M6 adds no field
   to `Snapshot`, so it still does not have to answer.
