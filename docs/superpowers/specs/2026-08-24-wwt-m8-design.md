# wwt M8 — Reader mode

**Date:** 2026-08-24
**Status:** Draft, pre-implementation
**Parent spec:** `2026-08-19-wwt-design.md` (sections 4, 6, 7, 8 and 11 govern here).

This is a delta against the system design, not a replacement for it. Where the two
disagree the parent spec wins and this document is wrong, except for the amendments in
section 10, which change the parent spec itself.

## 1. What M8 delivers

M7 made a browser that stays alive. M8 gives it one deliberate way to stop looking like
the page.

The ordinary text renderer keeps Chromium's geometry because that is what lets text,
pixels, clicks and hints agree. It is the right default and the wrong answer for a page
whose layout itself is the problem: three narrow columns, a paragraph set below one
cell's legibility threshold, or an article surrounded by everything a site would rather
you click first. Zoom can make the type larger, but it cannot make the article the page.

At the end of M8, `r` takes the dominant readable subtree, turns it into a linear
document, and wraps it to the terminal width. The ordinary scroll keys move through that
document without moving Chromium underneath it. `f` labels its links where they were
reflowed, the mouse can follow one, and a second `r` returns to the real page at exactly
the scroll offset where reader mode was entered.

This is the final milestone in the parent design.

### In scope

A semantic reader extraction, a deterministic dominant-subtree rule, a new pure
`wwt-reader` crate, block and inline content with links, terminal-width reflow, local
reader scrolling and progress, per-tab reader state, `r`, `[reader]`, hints and mouse
activation in reader geometry, resize, dirty-page refresh, tabs, detachment, pixel mode,
and the return to the real page.

### Out of scope

An editable reader view, forms, buttons, JavaScript controls, images beyond their alt
text, tables that preserve columns, author CSS, a typography configuration surface,
annotations, find-in-page, offline article storage, persistence of reader state across a
cold start, and a general-purpose readability library.

Reader mode is for reading and following links. `i` leaves it for the real page rather
than trying to make a reflowed form control mean the same thing as the control Chromium
laid out.

## 2. Architecture

The seam is unchanged. `Session` owns the choice of view and every rule around it,
reaches nothing, and answers `on(Event) -> Vec<Effect>` and `compose() -> Frame`. `Core`
turns `Effect::ReadReader` into one page query and returns `Job::Reader`; it decides
nothing. The terminal still consumes one `Frame` and learns no new mode.

### A document, then a layout

The page answers with meaning rather than geometry:

```rust
pub struct Document {
    pub blocks: Vec<Block>,
    pub links: Vec<Link>,
}

pub struct Block {
    pub kind: BlockKind,
    pub spans: Vec<Span>,
}

pub struct Span {
    pub text: String,
    pub link: Option<LinkId>,
}

pub struct Link {
    pub url: String,
    pub new_tab: bool,
}
```

`LinkId` is an index into `Document::links`, wrapped in a type so a block index cannot
be passed in its place. The page-level answer is:

```rust
pub struct ReaderExtraction {
    pub document: wwt_reader::Document,
    pub status: Status,
}
```

and crosses the seam as `Effect::ReadReader(TabId)` and
`Job::Reader(TabId, Result<Box<ReaderExtraction>, Failure>)`. The result owns its failure
for the same reason `Job::Extracted` does: it is one of the answers that clears
`Tab::reading`, so success and failure must be impossible to report through a path that
forgets to clear it.

`BlockKind` distinguishes a heading and its level, a paragraph, an ordered or unordered
list item and its depth, a quote, and preformatted text. It is deliberately not HTML:
there is no `Div`, no class name, and no computed style. Those are the layout reader mode
was asked to discard.

`wwt-reader` turns a `Document` and a column count into a `Layout`: styled rows, their
source positions, and the cell ranges occupied by links. `Session` keeps that layout on
the tab and paints a window of it into the page rows. Extraction and reflow are separate
answers, so a terminal resize reflows data already in memory and costs no Chromium round
trip.

### A new pure crate

Reflow is neither page machinery nor terminal I/O. Putting it in `wwt-page` would make a
browser operation responsible for terminal cells; putting it in `wwt-ui` would break the
rule that UI knows only chrome and modes; putting it directly in `Session` would bury the
milestone's hardest pure logic in the state machine that merely chooses when to use it.

M8 therefore adds `wwt-reader`. It depends on `wwt-frame` and nothing else, performs no
I/O, and owns the semantic types and the layout. `wwt-page` depends on it to construct a
`Document`; `wwt` depends on it to store and paint one. This is the same reason M6 put
PNG arithmetic in `wwt-png`: the small hard thing gets a boundary that lets data test it
directly.

This adds no crates.io dependency. Wrapping is short, deterministic arithmetic over
characters, and a dependency large enough to justify itself here would be a browser
engine beside the one already doing the extraction.

### The page stands still

Reader scroll is an index into `Layout::rows`, never an `Effect::Scroll`. Entering reader
mode records no new page offset because the offset to restore is already `Tab::scroll_y`,
and nothing in reader mode changes it. Leaving simply paints the real page again and
starts a normal extraction if the page became dirty while hidden.

This is stronger than saving the offset on entry and scrolling back on exit. There is no
round trip, no visible jump, and no race in which the observer reports the temporary
reader scroll as the session position. Chromium never left the place being restored.

### One hint language, two geometries

`HintTarget` is page geometry: its rectangle is in CSS pixels and its activation is a
click at the centre. A reader link has no honest CSS rectangle. Giving it a fabricated
one would put reader geometry through `Viewport`, the one type whose purpose is to map
Chromium's geometry.

`HintSession` is therefore narrowed to what it actually needs: a list of `CellPos`
values, labels, and the index selected by filtering. For a real page, `Session` converts
each `HintTarget::label_cell(&Viewport)` before entering hint mode and keeps the page
targets on the tab. For reader mode, it takes the first visible cell range for each link
from `Layout`. `Filtered::Activate` returns the selected index; `Session`, which knows
which view is active, decides whether that means a page click or a reader destination.

The label assignment and painting stay in `wwt-ui`. Only the false assumption that every
label begins as CSS geometry leaves it.

### Rejected alternatives

**Restyle the live page.** Inject a reader stylesheet, hide everything outside the
chosen subtree, and let Chromium reflow it. Links and input would work without new
machinery. It also mutates the document, moves the real scroll offset, lets site CSS and
scripts fight the view, and makes leaving reader mode a best-effort undo of changes we
did not own. Reader mode is a distinct view precisely so the page underneath remains
true.

**Turn semantic blocks back into `TextRun`s.** Give every reflowed line synthetic CSS
rectangles and send it through `Frame::paint_runs`. That reuses one painter by lying to
the coordinate model: the numbers would not be CSS pixels, `Viewport` would still treat
them as if they were, and reader links would need the same lie. A second pure painter is
cheaper than making the central type mean two incompatible things.

**Use the accessibility tree.** It already knows headings and links, and M6 rejected it
only because it had no geometry. Geometry is not wanted here, but the tree's accessible
name is an aggregation rather than a text stream: a container repeats names owned by its
descendants, whitespace and preformatted text are lost, and reconstructing document
order without duplication becomes the extractor. The DOM already contains the exact
text and destinations this view needs.

**Vendor Mozilla Readability.** It solves more hostile documents than the rule in
section 3 and brings thousands of lines whose heuristics and output become ours to
explain. M8 starts with semantic landmarks and a measured fallback. If real pages prove
that rule too small, Readability remains a replaceable extractor behind the same
`Document`; it is not required to build the view around it.

**Make reader mode global.** Pixel is global because it is a rendering preference over
the same page. Reader is a different document with its own scroll position, chosen
because one particular page needs it. Switching tabs must not turn an application UI
into a stream of labels because the article beside it was being read.

### Crate deltas

| Crate | M8 delta |
|---|---|
| `wwt-frame` | Nothing. Reader geometry never enters `Viewport`. |
| `wwt-reader` | **New.** `Document`, blocks, spans and links; terminal-width layout; source anchors; link hit ranges; painting a visible window into a `Frame`. Pure, no I/O, depends on `wwt-frame` only. |
| `wwt-png` | Nothing. |
| `wwt-cdp` | Nothing. The existing evaluated call and deadline are enough. |
| `wwt-page` | `reader()` and the DOM serializer that produces a `wwt_reader::Document` plus `Status`. |
| `wwt-term` | Nothing. It still diffs one `Frame`. |
| `wwt-ui` | `HintSession` takes cell positions and resolves an index; `Chrome` gains the reader tag and reader progress passed by its caller. |
| `wwt` | Per-tab reader state, `Action::ToggleReader`, `Effect::ReadReader`, `Job::Reader`, local scroll and link activation, and view-aware compose, focus, resize and navigation rules. |

## 3. Choosing the content

The query runs only when reader mode is first requested or its active document is dirty.
It is not part of ordinary extraction, and a page never pays for it merely by being
open.

The dominant subtree rule is deliberately small and deterministic:

1. Collect visible `article`, `main`, and `[role=main]` elements that contain non-space
   text after the exclusions below.
2. Score each on text it owns, excluding any nested candidate subtree: the number of
   normalized text characters outside links, plus one quarter of the characters inside
   links. Link text counts because it can be content; discounting it keeps a directory
   of navigation from beating an article of the same size. Excluding nested candidates
   keeps a `main` from winning merely because it contains the `article` being scored
   against it.
3. Choose the highest score. Equal scores choose the first in document order.
4. If there is no non-empty candidate, use `document.body`.

Nested candidates are allowed. An `article` inside a site-wide `main` ordinarily wins
because the outer candidate is scored only on what surrounds it. A `main` can still win
when its own introduction or documentation is the dominant material around smaller
nested candidates; choosing it then serializes the whole subtree, nested candidates
included. There is no fixed character threshold: a short article is still the article,
and thresholds turn a small correct document into the whole site.

The scoring walk and the serialization both exclude `script`, `style`, `template`,
`noscript`, `nav`, `aside`, `form` and `dialog`; a `header` or `footer` with no `article`
ancestor; elements with `hidden`, `aria-hidden=true`, `display:none` or
`visibility:hidden`; and descendants of any of them. A `header` or `footer` inside an
`article` is article content and stays, because that is where real pages put headlines,
bylines and footnotes.

`opacity:0` is excluded too, matching normal extraction. An author who puts the article
in `nav` has said it is navigation, and reader mode believes the semantic claim rather
than inventing a site-specific exception.

Computed style is read only for visibility. Font, colour, position, size and paint order
never cross the boundary.

## 4. The semantic document

The chosen subtree is walked once in document order. Every text node is emitted exactly
once. The serializer flushes at block boundaries rather than querying a list of block
elements independently, which is what prevents a paragraph from appearing again as the
text of each ancestor around it.

The mapping is:

- `h1` through `h6`: `Heading(level)`.
- `p` and generic runs of inline content between block boundaries: `Paragraph`.
- `li`: `ListItem`, carrying nesting depth, orderedness and the ordinal Chromium gives
  the list item. The marker is ours, not text copied from CSS.
- `blockquote`: `Quote`.
- `pre`: `Preformatted`; internal spaces and line breaks survive.
- `br`: a hard line break inside the current block.
- `dt` and `dd`: paragraphs, emitted in order.
- A table: one paragraph per row, with non-empty cells separated by ` | `. Reader mode
  linearizes a table; it does not claim the columns still line up after wrapping.
- `img[alt]`: its alt text in brackets. An image with no alt text contributes nothing.

All other block containers provide boundaries and all other inline elements contribute
their text. Ordinary whitespace collapses to one space across inline nodes; leading and
trailing whitespace at a block edge is removed. Preformatted text keeps spaces and
newlines but drops terminal control characters. No emitted string may contain an escape
or a control other than the represented hard break.

An `a[href]` contributes ordinary inline text plus a link reference. `HTMLAnchorElement`
resolves `href` before it is returned, so the Rust side sees an absolute destination.
Links with an empty destination or a `javascript:` scheme are text and not targets.
`target=_blank` is recorded as `new_tab`; every other target is same-tab. Event handlers,
buttons and controls do not become reader targets, because there is no meaningful
promise that activating them outside the page's geometry does what their label says.

Adjacent spans with the same link are joined. Empty blocks are dropped, and consecutive
blank rows are a layout decision rather than blocks in the document. The result is data
small enough to cache on a tab and stable enough to assert without painting it.

The query also returns the ordinary `Status`. Title and URL can change while a reader
document is being built, and the one round trip should not need a second read to keep the
chrome honest. Its page scroll offset is the unchanged real offset and is applied through
the same `apply_status` path as every other read.

## 5. Reflow

`Layout::new(&Document, cols)` fills exactly `cols` cells per logical row and never
consults a pixel. The page area is the full available width: terminal users already own
their margins, and taking two more columns from a narrow terminal to imitate paper buys
less than it costs.

Ordinary blocks wrap at whitespace. A word longer than the available width is split, not
elided, because reader mode's promise is that content is rearranged and never discarded.
Wrapping counts `char`s, the same unit `Frame::paint_text` and the existing run painter
use. Wide-character display width remains the codebase's existing limitation; M8 does
not add a Unicode-width dependency to one renderer while every other renderer still
counts one scalar as one cell.

Block presentation is fixed:

- Paragraphs have one empty row after them.
- Headings are bold, have one empty row before and after, and never gain more than one
  blank row where blocks meet. Levels one and two are additionally separated by a row
  of `=` or `-` characters capped to the heading's visible width.
- Unordered list items begin with `• `; ordered ones with `<ordinal>. `. Wrapped lines
  align under the text after the marker. Each nesting level adds two cells, capped so at
  least one content cell remains.
- Quotes begin with `> ` on every wrapped row. Nested quotes add another `> ` under the
  same cap.
- Preformatted lines preserve their spaces and hard breaks, but a line wider than the
  terminal is hard-wrapped. Reader mode has one vertical scroll axis and does not grow a
  hidden horizontal one for code.
- Links use the default foreground, bold, with no background. The hint is the strong
  visual affordance; a fixed blue would disappear into some terminal themes and an
  underline would widen `Style` for one view.

Every laid-out row carries a source position: block index and character offset of its
first content. Before a resize, `Session` records the source position at the top row;
after reflow it chooses the new row at or immediately before that position. Resizing a
window therefore changes wrapping around the sentence being read rather than sending the
reader to an unrelated numeric row.

Every cached reader layout is rebuilt on resize, including background tabs, so the next
switch remains a repaint. Attached pages still receive the existing
`Effect::SetViewport`: the real page must have the right geometry when reader mode is
left. Local reflow adds no browser operation of its own.

A link that wraps produces one `LinkRange` per occupied row. Each range carries the link
index and an inclusive start and exclusive end column. They serve both mouse hit-testing
and hint placement; the first visible range of a link is its label cell. A link appears
once in hint mode however many rows it spans.

Layout is rebuilt when the document or column count changes, never in `compose`.
Composing a reader frame copies only the visible rows into the page area and then paints
hints and chrome in the same order every other view does.

## 6. Scrolling and progress

Reader state keeps `top_row`, clamped to `0..=max_top`, where:

```text
max_top = layout.rows.len().saturating_sub(page_rows)
```

`j`, `k`, arrows, half-page keys, page keys, `g`, `G`, Page Up, Page Down, Home, End and
the mouse wheel all change that number locally. Their distances are in terminal rows,
not CSS pixels: one line is one row, half a page is half the page rows, a page is the page
rows less the same two rows of context used by the normal keymap, and a wheel notch is
three rows.

The keymap still returns one semantic scroll action. It does not learn which view is in
front. `Session` interprets the action as a local row movement for reader mode and an
`Effect::Scroll` for a real page. This is the same split by which `Session` already
decides whether a dirty signal costs an extraction or a status read in pixel mode.

Reader progress is `top_row / max_top`, or zero when `max_top` is zero. The statusline
uses it while `[reader]` is present and returns to the page's own progress on exit.
Reader scrolling emits no `Effect::Save`: it is not restored after a cold start and it
must not overwrite the real page offset that is.

## 7. Links, hints and the mouse

Pressing `f` in reader mode is local. `Layout` supplies the visible links and their label
cells, so there is no `Effect::Hints`, no in-flight flag and no possibility of a late
page answer opening hint mode over another view. The same alphabet, prefix-free labels,
filtering, overlay style and status tag are used.

Selecting a reader link does one of two existing things:

- A same-tab link leaves reader mode and calls `navigate(Navigation::Open(url))`.
- A `target=_blank` link calls `open_tab(url)` and becomes a normal new tab.

Following a destination rather than synthesizing a click is deliberate. Reader mode
keeps links and drops controls; its promise is the destination in `href`, not whatever a
site's click handler might do. It also makes a cached reader document useful after its
target was evicted, because a URL does not become invalid when page geometry does.

A left-button press on a visible `LinkRange` follows the same path. A press elsewhere
does nothing, and releases are consumed locally. The wheel scrolls locally. No reader
mouse event is converted through `Viewport` or sent to Chromium.

If there are no visible links, `f` says `no hints`, matching page mode. Links below the
window become hintable after scrolling to them; labels are for what can be seen, exactly
as the page query only reports interactive boxes on screen.

## 8. The view lifecycle

### Per-tab state

Each `Tab` gains reader state: an optional `Document`, its width-specific `Layout`, the
top row, whether reader is active, and whether the cached document is dirty. The existing
`reading` flag guards reader reads too. There cannot be a normal extraction, status read
and reader query in flight together, and one flag is the fact that enforces it.

Reader state is not added to `Snapshot`. A semantic document can be large, may contain
text the user did not expect written to disk, and belongs to the version of a page that
was live rather than to the URL that can be restored. A cold start comes back to the real
page. A tab switch, eviction or Chromium relaunch within one run keeps the cached reader
document, because it is already the frame being read.

### Enter and leave

The first `r` marks reader wanted and starts `Effect::ReadReader` if no clean cached
document exists, the tab is attached, and the existing read slot is free. If an ordinary
read is already in flight, its one answer starts the reader read afterward. Until the
reader answer lands the real page remains on screen with a `reading` notice. A first
query that fails never enters reader mode, so failure cannot turn a useful page into an
empty view.

With a clean cache, entry is a repaint. With a successful first answer, the document is
laid out, `top_row` starts at zero, and reader becomes active. Later `r` entries return to
the reader row last used, unless the document was invalidated by navigation.

A second `r` makes the reader inactive immediately. The browser has not moved, so the
cached runs paint at the same real scroll offset immediately. If normal runs became dirty
while hidden, extraction refreshes them behind that repaint.

`i` leaves reader mode and enters insert mode on the real page. `H`, `L`, `Ctrl-r`,
`:open`, `:back`, `:forward` and `:reload` leave it before navigating. A reader link does
the same for a same-tab destination. Navigation clears the cached `Document`, layout and
top row: they describe the document being left and must never appear under the new URL.

### Dirtiness

A dirty signal always marks the ordinary page runs dirty, as it does now. It also marks
the reader document dirty. If reader mode is active, the current layout stays on screen
and a reader read starts; if it is inactive, the flag waits until the next `r` and costs
nothing meanwhile.

A successful reader answer replaces the document and reflows it. On an automatic refresh
the numeric top row is kept and clamped; DOM edits do not provide a stable semantic
anchor across two different documents, and pretending block indices are stable would be
worse than the small movement this rule admits. A second dirty signal while the read is
in flight leaves the flag set and causes exactly one more read when the first lands, the
same coalescing rule ordinary extraction uses.

### Tabs and detachment

Reader-active is per-tab and follows the tab. Switching from a reader tab to a normal tab
paints that tab's cached runs; switching back paints the cached reader layout. A switch
is still a repaint and never waits for Chromium merely to show what the tab last looked
like.

`Tab::detach` keeps the document, layout, active flag and top row, clears `reading`, and
marks both reader and page data dirty. A reader-active tab remains locally scrollable
while Chromium is being replaced. Once the target reattaches, `Job::Opened` refreshes the
reader document instead of starting an ordinary extraction. Cached link URLs remain
usable data, but an action that needs a missing browser follows M7's rule and asks for one
back first.

Eviction excludes a reader read in flight through the existing `reading` check. A
background reader tab otherwise remains as eligible as any other tab; its cached document
is cheap and its Chromium target is what `max_tabs` exists to bound.

### Pixel mode

Reader is a view over a tab; pixel is the global rendering choice for real pages. An
active reader view suppresses the screencast for that tab but does not change the global
pixel preference.

Entering reader from pixel mode stops the focused screencast and keeps the last real
picture out of the composed reader frame. Leaving returns to the real page in pixel mode
and starts the screencast again. Switching from a reader tab to a normal tab while pixel
is on starts that tab's screencast; switching onto a reader tab stops the one being left
and starts none. The previous picture is never allowed to sit behind reader cells.

Pressing `p` in reader mode leaves reader mode and selects pixel mode. Pressing it in a
real-page view keeps M5's toggle exactly. The statusline never says `[pixel]` and
`[reader]` together because the visible frame cannot be both.

## 9. The key and the statusline

Bare `r`, currently unbound, is `Action::ToggleReader`. Reload remains `Ctrl-r`. The
distinction is written into the key table and README together, because a reader toggle
that steals reload would be an invisible regression in the browser's most common
recovery key.

`:set reader on|off` is deliberately not added. Reader is per-tab and carries a local
position; fitting it into the global `Setting` enum would imply the same semantics as
mouse and pixel. The key is the whole surface for M8. A command can be added when there
is a use for scripting this rather than for symmetry alone.

`Chrome` gains `reader: bool` and receives the progress already chosen by `Session`.
`[reader]` sits where `[pixel]` sits and is printed only while active. `[degraded]` may
remain beside it: degraded describes how the real page is read, while reader describes
what is in front, and both can be true without contradiction.

Reader mode is not a `wwt_ui::Mode`. Normal, insert, hint and command answer what the
keyboard means; reader answers which document normal mode is looking at. Making it a
fifth input mode would duplicate command and hint transitions inside it and would make
`Esc` ambiguously mean both "leave hints" and "leave reader". Only an explicit user
action changes the view: `r` enters or leaves it, and `p`, `i` or a navigation leaves it
for the real page. No page event can enter it.

## 10. Amendments to the parent spec

Five, to be made in the same commit as the implementation they describe.

1. **Section 4, components.** Add `wwt-reader`: semantic reader data and pure reflow,
   depending on `wwt-frame` only and performing no I/O. `wwt-page` constructs its
   `Document`; the binary owns when one is shown.
2. **Section 6, reader mode.** "Hints within it address reflowed positions" is now cell
   positions handed directly to the existing hint UI. Reader scroll is local and never
   moves the page underneath. Links are destinations, not synthetic page clicks.
3. **Section 6, mode wording.** Reader is a per-tab view rather than a fifth input
   `Mode`. Entering and leaving are still caused only by keys. Leaving returns to the
   real page at the scroll offset held throughout; if the global pixel preference is on,
   that real page is painted as pixels rather than text.
4. **Section 7, sessions.** Reader state survives tab switches, target eviction and a
   Chromium relaunch in memory, and is not serialized into `session.json`. A cold start
   restores the URL and page offset and begins in the real-page view.
5. **Section 11, M8.** The boundary is discharged by sections 1 through 9 here: semantic
   extraction, the reflow renderer, reader links, and the per-tab view on top of them.

## 11. Failure modes

The governing rule is unchanged: never blank the frame you are looking at.

- **No dominant semantic element exists.** `document.body` is the candidate and the
  excluded landmarks are removed. A poor linearization is still the page's readable
  text, and the second `r` is always one key away.
- **The chosen subtree has no readable blocks.** Reader mode does not open. The real page
  stands and the statusline says `no readable content`.
- **The reader query times out.** `State::Stalled`, the current real or cached reader
  frame stands, and no fallback query is attempted. It needs the same main thread that
  did not answer, exactly as M7's ordinary extraction rule.
- **The reader query fails.** A first entry stays on the real page with an error. A
  refresh keeps the previous reader layout with an error. It does not set `degraded`:
  reader serialization and normal extraction are separate functions, and one failing
  says nothing about the other.
- **The page changes while reader mode is open.** The old layout stands until one
  coalesced refresh replaces it. Nothing flashes blank and nothing polls.
- **A link has no usable destination.** It is text, not a target. Reader mode does not
  invent an activation it cannot honour.
- **A same-tab reader destination fails to load.** Reader has already left. Chromium's
  error page and the existing `chrome-error://` rule handle it like any other navigation.
- **The terminal becomes one column wide.** Prefixes are capped away until one content
  cell remains. Every character still appears in order and no layout arithmetic can
  underflow.
- **Chromium dies in reader mode.** The cached document remains readable and locally
  scrollable. The tab is labeled with M7's browser notice, and the next action that needs
  a page asks for the browser again.
- **A late reader answer names a closed tab.** Dropped like every other job naming a tab
  that no longer exists.
- **A late reader answer lands after `r` was pressed again.** Cached on that tab but not
  made active. A completed round trip cannot reverse a later keystroke.

## 12. Testing and cost

Most of M8 is pure, and its tests split at the same seam as the code.

- **`wwt-reader` document data:** adjacent spans join without losing link boundaries;
  every block kind is representable; a link index always names an existing destination.
- **`wwt-reader` layout:** golden rows for paragraphs, long words, nested lists, quotes,
  headings, preformatted text, tables and one-column terminals. No row exceeds the
  width, no non-space input character disappears, and wrapping the same document twice
  is identical.
- **Source anchors:** resizing narrower and wider leaves the same source position at or
  immediately below the top. End positions clamp when content shrinks.
- **Link geometry:** a link across three rows has three hit ranges and one visible hint;
  scrolling changes which range owns the label; a click outside every range is nothing.
- **`wwt-page`:** fixture pages with one article inside noisy landmarks, two competing
  articles, `main`, body fallback, hidden content, nested inline links, lists, a table,
  preformatted text and image alt text. Assert the `Document`, not JavaScript's wire
  shape. A real Chromium is required because visibility and resolved URLs are browser
  answers.
- **`wwt-ui`:** hint assignment and filtering are unchanged, now asserted from cell
  positions, and activation returns the selected index.
- **`wwt`:** `r` requests once, failure keeps the old frame, a clean cache re-enters
  locally, reader scroll emits no effect, exit preserves `scroll_y`, dirty signals
  coalesce, navigation clears the document, tab switches preserve each reader position,
  detach keeps the layout and clears the read, pixel screencasts follow only real-page
  views, and late answers obey the later key. No browser: these are decisions.
- **End to end:** through a PTY, enter reader mode on a noisy fixture, scroll, hint a
  link, and assert the destination. A second flow enters halfway down a real page, reads,
  exits, and asserts the original real-page rows return.

Two measurements are added and print rather than assert wall-clock budgets:
`measure_reader_extract` on the existing `heavy.html`, beside script and snapshot reads,
and `measure_reader_layout` over a document large enough to fill several thousand rows.
The important structural costs are asserted instead: ordinary extraction is unchanged,
a terminal resize adds no page effect beyond the existing viewport update, reader
scrolling emits no page effect, and an inactive cached reader document costs nothing on
a dirty signal beyond setting a flag.

No M2 through M7 measurement is allowed to move. Reader code is off every ordinary path
until `r` is pressed.

## 13. Open questions

None blocking. Three noted:

1. **Whether semantic landmarks plus body fallback are enough.** This is intentionally
   less ambitious than Readability. The fixture and manual pass must include real news,
   documentation, blog and marketing pages. If the fallback repeatedly includes site
   furniture, the replacement belongs wholly inside section 3 and does not change the
   document or layout contracts.
2. **Whether a same-tab fragment should stay inside reader mode.** The first version
   treats it as a normal destination, leaves reader and lets Chromium resolve the
   fragment. Mapping DOM anchors into semantic source positions would be nicer and would
   add a second identity system solely for in-document links. Real use should decide
   whether that machinery earns itself.
3. **Snapshot version strictness**, carried from M5 and M6. M8 deliberately adds no
   reader field to `Snapshot`, so it is still not the milestone that has to answer it.
