# wwt M4 — Tabs and sessions

**Date:** 2026-08-21
**Status:** Approved, pre-implementation
**Parent spec:** `2026-08-19-wwt-design.md` (sections 3, 5, 7 and 8 govern here).

This is a delta against the system design, not a replacement for it. Where the two
disagree the parent spec wins and this document is wrong, except for the amendments in
section 10, which change the parent spec itself.

## 1. What M4 delivers

M3 made a browser you can use on one page. M4 makes it a browser you keep open: many
pages at once, and the same pages tomorrow. It is where daily use realistically begins.

At the end of M4, `wwt` with no argument comes back to the tabs you left, still logged
in to the sites you were logged in to, and a link that wants a new tab gets one.

### In scope

Many CDP targets under one `Session`, a tab record per target, the tab bar, tab keys
and commands, adoption of targets a page opens for itself, background idling, the
persistent profile with its fallback, the session file, and restore at startup.

### Out of scope

Eviction of background targets past a limit, and the lazy restore that shares its
machinery: both are deferred to M7 for the reason in section 3. Also out: pixel mode,
reader mode, the Chromium supervisor, per-command deadlines, and any configuration
file. Tab titles come from the page and are not editable; there is no tab reordering,
no pinning, and no undo of a close.

## 2. Architecture

The loop is unchanged, and so is the seam. `Session` still owns every piece of state,
still reaches nothing, and still answers `on(Event) -> Vec<Effect>` and `compose() ->
Frame`. `Core` is still the adapter that decides nothing. What M4 changes is that both
sides now have to say *which page* they mean.

### Everything a tab needs, in the tab

A new `wwt/src/tab.rs` holds the per-page state that `Session` used to hold flat:

```rust
pub struct Tab {
    pub id: TabId,
    pub url: String,
    pub title: String,
    pub runs: Vec<TextRun>,
    pub caret: Option<Caret>,
    pub scroll_y: f64,
    pub progress: f64,
    pub state: State,
    pub dirty: bool,
    pub extracting: bool,
    pub navigating: bool,
    pub hints: Option<Vec<HintTarget>>,
    pub hinting: bool,
}
```

`Session` keeps only what is genuinely global: the grid, the cell size, the viewport,
the mode, the `:` line, a `Vec<Tab>`, and a focus index. This is also what splits
`session.rs`, which at 982 lines is the largest file in the repository and had become
two things in one.

A background tab keeps its `runs`. That is the whole point of the record: switching
paints the cached frame immediately and extracts afterwards, so a tab switch costs a
repaint rather than a round trip. Latency is a feature, and it should not stop being
one at the tab boundary.

### `TabId`, and why it is not an index

`TabId` is a monotone counter on `Session` that never reuses a value. Effects that
name a page carry one and jobs carry it back, and a job whose `TabId` is no longer in
the vector is dropped.

An index would be wrong for the reason the hint query was already wrong once: a page
operation outlives the state that asked for it. Close tab 2 while its extraction is in
flight and every later tab shifts down one; the answer would land on a page that never
asked. A value that is never reused makes the stale answer identifiable instead of
plausible, which is the difference between dropping it and painting it.

### Rejected alternatives

**Only the foreground tab holds live state.** Background tabs would be a URL, a title
and a page handle. Much the smaller change, and no `TabId` anywhere. Rejected because
a switch would then blank until the extraction lands, which is both a latency
regression and the one thing spec section 8 says never to do, and because a background
tab whose page retitles itself would go stale in the bar.

**A `Session` per tab, with a container above.** Zero disruption to the existing state
machine. Rejected because the mode, the `:` line and the viewport are global rather
than per tab, so the seam lands in the wrong place: the container would reach into the
focused sub-session for the mode anyway, and the viewport would be duplicated once per
tab and have to be kept in step.

**The tab bar above the statusline.** Costs nothing at all: page row 0 stays frame row
0 and the coordinate model is untouched. Rejected because a tab bar at the bottom
reads as a second statusline, and because the origin row it avoids is wanted by pixel
mode and reader mode anyway. See section 5.

**Our own lock file for the profile.** Rejected in favour of the profile itself; see
section 7.

**`gt` and `gT` for tab switching.** There is no prefix machinery in the keymap, `g`
is bound directly to scroll-top, and adding a pending-prefix state would make
`action_for` no longer total over `(mode, key)`. qutebrowser, which is the tool wwt
means to be an alternative to, binds `J` and `K`. See section 6.

### Crate deltas

| Crate | Change |
|---|---|
| `wwt-frame` | `Viewport` gains an origin row. Still no I/O, still no dependencies. |
| `wwt-cdp` | The persistent profile and its fallback in `launch.rs`. |
| `wwt-page` | `Page::adopt` for a target we did not create, `Page::scroll_to`, `Page::activate`, `Page::close`. |
| `wwt-term` | None. |
| `wwt-ui` | `chrome::tab_bar`. Still depends on `wwt-frame` only. |
| `wwt` | `tab.rs` and `store.rs` are new; `session.rs` shrinks; `core.rs` holds a map of pages and coalesces saves. `serde` and `serde_json` are added to its manifest from the existing workspace set. |

**No new dependencies.** `serde` and `serde_json` are already fixed in the workspace
`Cargo.toml` and used by `wwt-cdp` and `wwt-page`; M4 adds no crate to that set. The
XDG data directory is resolved by hand from `XDG_DATA_HOME` and `HOME` rather than by
adding `dirs`.

## 3. Tabs

**Focus.** Exactly one tab is focused. Only the focused tab receives keys, clicks,
scrolls and hint queries, and only the focused tab is painted.

**Idling.** A tab extracts once when it opens, so the bar has a real title and the
first switch to it is instant. After that it re-extracts only while focused: a dirty
signal for an unfocused tab sets its flag and does nothing else, and the flag is
spent when focus arrives. An idle background tab therefore costs exactly what an idle
foreground tab costs, which is nothing.

**Switching activates.** `CLAUDE.md` records that `Input.dispatchMouseEvent` is
answered by whichever target the browser has in front. With one target that was a
test-harness quirk; with several it is a correctness rule, so switching calls
`Target.activateTarget` and keeps the browser's foreground and ours the same thing.
M5's screencast will want the same guarantee.

**Closing.** Closing the focused tab focuses its neighbour, preferring the one to its
right. Closing the last tab quits, which is the same rule `q` follows and means there
is never a browser with no page in it.

**Modes across a switch.** Switching is reachable only from normal mode: the tab keys
are not bound in insert, where every key goes to the page, nor in hint, where every
key is a label character. So a switch always begins and ends in normal mode and no
rule is needed for what happens to insert mode when the page under it changes. Mode
still changes only in response to a keystroke.

The hint rule gains one clause. M3 established that a late `Job::Hints` opens hint
mode only if the mode is still normal, because a round trip is long enough to have
typed half a `:` command. It must now also be true that the answering tab is still
focused, or labels measured against one page would be painted over another.

**Adoption.** A page that opens a tab for itself, through `target=_blank` or
`window.open`, creates a target we did not ask for. `Target.setAutoAttach` reports it
as `Event::TargetOpened`, and `Page::adopt` installs the binding, the bootstrap and
the viewport on it. Auto-attach delivers a session for every new target, ours
included, so `Page::open` takes its session from the same event and the two paths are
one; they are told apart by `openerId`, which `Target.createTarget` does not set.

The document such a tab loads has usually already run by the time we hear about it,
and `Page.addScriptToEvaluateOnNewDocument` only reaches documents that have not
started. Registering it is still what covers the tab's *next* document, and `adopt`
evaluates the same source into the one already there, so an adopted tab is readable
on arrival rather than blank until it navigates. The bootstrap returns early when it
finds itself installed, so only one of the two ever takes effect.

*Amended.* This section previously specified `waitForDebuggerOnStart`, holding each
new target before its first script so the bootstrap could be installed into the
document it was about to load. Measured against Chromium, a held target answers
`Target.getTargetInfo` and `Runtime.runIfWaitingForDebugger` and nothing else:
`Page.enable`, `Runtime.addBinding` and `Page.addScriptToEvaluateOnNewDocument` all
queue unanswered until it is released, so no setup call can be awaited, and a
registration queued that way still misses the document. The hold costs every setup
call a round trip it cannot make and buys nothing, so it is not used.

The adopted tab opens in the foreground, which is what clicking such a link does in
any other browser.

*Known deviation.* A middle click makes Chromium open a background tab, and
`Target.targetCreated` does not distinguish that from a foreground one, so for now
both arrive in the foreground. `Page.windowOpen` carries the gesture and can settle it
later; middle click is not bound in M4, so it does not arise from our own input.

**Eviction, and why it is not here.** The parent spec closes background targets past a
configurable limit, keeping their URL and scroll offset, and reopens them
transparently on switch. It is deferred to M7 with the supervisor, because it
introduces the one state this design otherwise does not have: a tab that exists
without a target. Every rule in this document would need a second reading for it, and
it buys memory back in a browser nobody yet keeps open long enough to need it. Lazy
restore is the same machinery pointed at startup and is deferred with it.

## 4. The vocabulary

`event.rs` and `effect.rs` keep their meanings; what changes is that most of their
variants now name a tab.

```rust
pub enum Effect {
    Extract(TabId),
    Hints(TabId),
    Scroll(TabId, Scroll),
    Navigate(TabId, Navigation),
    Send(TabId, Input),
    Blur(TabId),
    SetViewport(TabId, Viewport),
    OpenTab { id: TabId, url: String },
    AdoptTab { id: TabId, target: TargetId },
    CloseTab(TabId),
    Activate(TabId),
    Save(Snapshot),
    MouseCapture(bool),
    Quit,
}
```

`SetViewport` is emitted once per tab on a resize, because a background tab has to be
the right size already when you reach it, not a round trip after.

```rust
pub enum Job {
    Opened(TabId, Result<(), String>),
    Extracted(TabId, Box<Extraction>),
    Settled(TabId),
    Failed(TabId, String),
    Hints(TabId, Result<Vec<HintTarget>, String>),
    Resized(TabId),
    Noted(String),
}
```

`Event::Dirty` gains a `TabId`, and `Event::TargetOpened(TargetId)` is new. `Core` holds `HashMap<TabId, Arc<Page>>` in place of
one `Arc<Page>` and asks the map which page a CDP event belongs to, which
`Page::is_dirty` already answers correctly by session id: its own comment anticipates
several pages on one browser.

**Ids are minted on one side.** A target a page opened for itself arrives at `Core`,
which does not mint `TabId`s, so adoption is two steps rather than one:
`Event::TargetOpened(target)` in, `Effect::AdoptTab { id, target }` back out once the
session has made a tab for it, and `Job::Opened` to report how preparing it went, the
same variant a tab we asked for reports through. `TargetId` is a CDP fact travelling
through the vocabulary the way `Input` and `Extraction` already do, which is what
keeps the session from having to know what a target is.

`Job::InputFailed` is renamed `Job::Noted`. It always meant "this failed after the
loop had moved on, so say so in the statusline and change nothing", and M4 gives it
two users that are not input: a close that failed and a save that failed.

**A tab with no page yet.** Between `Effect::OpenTab` and `Job::Opened` the session
has a tab and `Core` has no page for it. `Core` drops effects naming a page it does
not hold; the tab is in `State::Loading` throughout, so nothing is silently lost that
a user could have expected to land.

**The input pump stays one task.** Its channel carries `(Arc<Page>, Input)` rather
than `Input`, so ordering remains global: keys typed either side of a tab switch
cannot overtake each other, and the M3 property that `abc` never arrives as `acb`
survives having somewhere else to send it.

## 5. Chrome and the origin row

The chrome is two rows: the tab bar at the top and the statusline at the bottom.
`page_viewport` subtracts two instead of one, unconditionally, so opening or closing a
tab never resizes the page. `chrome::tab_bar` takes the titles, the focused index and
the column count, and paints one row: index and title per tab, elided with `…`, and a
window around the focused tab when there are more tabs than fit. It depends on
`wwt-frame` only, like everything else in `wwt-ui`.

Putting a row above the page breaks the coincidence that page row 0 and frame row 0
are the same row. `CLAUDE.md` makes `Viewport` the only thing allowed to convert
between CSS pixels and cells, so the shift belongs there rather than as a `+1`
sprinkled through `paint_run`, `Caret::cell` and `page_cell`:

```rust
Viewport::with_origin(grid, cell, origin_row)   // Viewport::new is origin 0
```

`to_cell`, `col_of` and `row_of` return **frame** rows, and `to_css` takes one and
subtracts the origin before converting. `css_width` and `css_height` are untouched,
because the page's size in CSS pixels has nothing to do with where the page sits on
our screen. The load-bearing property is unchanged in form and stronger in content:

    to_cell(to_css(c)) == c

for every cell in the grid, at every cell size, **and at every origin**. The existing
property test gains an origin to its loop.

`page_cell` becomes a bounds check rather than a translation: a terminal row outside
`[origin, origin + rows)` is chrome and has no page coordinate to become, which is now
true of the first row as well as the last.

## 6. Keys and commands

| Key | Action |
|---|---|
| shift and a digit | Focus the first tab through the ninth. Out of range does nothing. |
| `t` | Open the `:` line prefilled with `tabopen `, the way `o` prefills `open `. |
| `x` | Close the focused tab. |

`d` and `u` are half-page scroll, so qutebrowser's `d` is not available for close and
`x` takes it.

*Amended.* Switching was `J` and `K`, qutebrowser's own bindings, cycling one tab at a
time. It is now shift and a digit, going straight to the first tab through the ninth.
Going straight beats cycling because the tab you want is one keystroke away however
many are open, and where each one sits is already on screen in the bar. Past the ninth
tab, `:tabnext` and `:tabprev` still cycle.

*Amended.* Which keystroke that is depends on the keyboard layout, so `keymap.rs` takes
the digit and the glyph alike.

The digit is the one that carries, and it is taken with `SHIFT` or without. Nearly
every layout has digits on the unshifted number row, so the plain digit is that key.
The layouts that do not, French among them, are exactly the ones where shift and that
key is how a digit is typed at all. Between the two, every keyboard reaches every tab
with one keystroke and the terminal is asked nothing.

Binding a bare digit spends the count prefix a vim-like puts on digits. Reaching a tab
on every layout is worth more than a count that no command takes yet.

The glyph table is what the number row prints, which is US muscle memory (`!` through
`(`) plus the foreign glyphs that collide with none of it: `£`, `§`, `·`, `№`, `¤`. A
collision is resolved by leaving the glyph out rather than guessing, in both
directions. `&` is a US shift-7 and a German shift-6, so the US row keeps it; `"` and
`)` are a German shift-2 and shift-9 and also a US shift-apostrophe and shift-0, so
neither is bound, since binding them would move a US keyboard's tabs on a keystroke
that means nothing here. Nothing is lost either way: every layout in this paragraph has
the digit. `/` is a European shift-7 and stays unbound because find-in-page will want
it.

*Rejected: Kitty's keyboard protocol.* It reports the key and the modifier separately
rather than the glyph the pair prints, which is exactly the question being asked here,
and it makes things worse in two ways.

`REPORT_ALTERNATE_KEYS` offers the PC-101 key beside the layout's own, which is layout
independence outright, but crossterm 0.29 discards it: given the shift modifier it
takes the *shifted* codepoint, overwrites the keycode with it and clears `SHIFT`,
leaving the layout-dependent glyph and no modifier. That is a limit of the crate rather
than the protocol, and worth revisiting if crossterm ever surfaces the base key.

`DISAMBIGUATE_ESCAPE_CODES` alone reports the unshifted key, which costs more than it
buys: shift and `h` arrives as `Char('h')` with `SHIFT` rather than as `Char('H')`, so
`H`, `L` and `G` stop working, and insert mode types a lowercase letter and the wrong
punctuation, because the glyph a shifted key prints is precisely what the flag stops
reporting. Typing is worth more than a keystroke to a tab. `supports_keyboard_enhancement`
also takes the terminal's stdin to ask, for up to two seconds against something that
does not answer.

Commands: `:tabopen <url>`, `:tabclose`, `:tabnext`, `:tabprev`. `:tabopen` normalizes
its argument through the same `normalize_url` that `:open` uses.

*Amended.* `normalize_url` no longer refuses what is not a URL. A single word with a
dot in it, or a host and a port, is somewhere to go; anything else is a DuckDuckGo
search for it. So `:open banana`, `:tabopen banana` and `wwt banana` all search, and
the one thing that is still an error is nothing at all.

## 7. The profile

`$XDG_DATA_HOME/wwt/profile`, falling back to `$HOME/.local/share/wwt/profile`, in
place of the `tempfile::TempDir` M1 used. This is what makes logins durable, and OAuth
redirects work because it is a real browser with a real cookie jar.

**The profile is the lock.** Chromium refuses a `--user-data-dir` another Chromium
holds and exits without announcing a debugging endpoint, which is already the one
failure `read_ws_url` reports. So a second `wwt` needs no lock of its own: it tries
the persistent profile, and on failure relaunches on a temporary one and says
`private session` in the statusline. It is not logged in to anything, and it writes no
session file.

**The instance holding the profile owns the session file.** One rule covers both
resources, and there is no lock file of ours to go stale after a crash.

That Chromium really does refuse is an assumption, not a fact this design has
verified. The implementation plan's first task proves it, with a test in
`wwt-cdp/tests/browser.rs` that launches twice on one directory and asserts the second
falls back. If it turns out that Chromium proceeds instead, this section is wrong and
a lock file of our own is the fallback.

A second failure is reported as a failure, which is correct: if the browser is broken
rather than busy, the temporary profile fails too, and the error the user sees is the
real one.

## 8. The session file

`$XDG_DATA_HOME/wwt/session.json`, holding a snapshot in `wwt/src/store.rs`:

```rust
pub struct Snapshot {
    pub version: u32,          // 1
    pub focus: usize,
    pub tabs: Vec<SavedTab>,   // { url, title, scroll_y }
}
```

It is a `Snapshot`, not a session. `Session` already names the state machine and
`wwt-cdp` already calls an attached target a session id; a third meaning for the word
would make the glossary useless.

**Deciding is a rule, writing is machinery.** `Session` emits `Effect::Save(snapshot)`
when the tab set changes, when a navigation settles, or when the scroll offset moves.
`Core` coalesces those on a timer the way it already coalesces resizes, so a held `j`
costs one write rather than one per frame, and writes temp-then-rename in the same
directory so a crash mid-write cannot truncate what was already there. Quitting
flushes once more before exiting.

A save that fails is a `Job::Noted`. It is never a reason to change the frame or to
stop browsing.

## 9. Restore

`wwt` with no argument restores. `wwt <url>` restores and adds the URL as a new
foreground tab, because the session is the browser's state and a URL argument means
"and also open this": nothing you had is lost by typing `wwt example.com` out of
habit, which is the failure mode that actually costs something. `wwt --new <url>`
ignores the saved snapshot and starts one clean, then persists normally from there.
Argument parsing stays hand-rolled.

Every restored tab opens, navigates, scrolls to its saved offset through a new
`Page::scroll_to`, and extracts once. The bar therefore has real titles immediately
and the first switch to any tab is instant, which is the whole reason not to restore
lazily in this milestone.

A tab that fails to restore stays a tab, showing whatever its navigation produced,
exactly as a failed `:open` does today. Nothing is dropped for failing.

A missing session file is a first run: one tab, no notice. A malformed or
future-versioned one is a notice and one tab, never an exit. The snapshot is data from
disk and is not trusted: an empty tab list, a focus index past the end, and a URL that
does not parse are all handled rather than asserted.

## 10. Amendments to the parent spec

These change `2026-08-19-wwt-design.md` and land in the same commit as this document.

1. **Section 3, the coordinate model.** `Viewport` gains an origin row, and the
   roundtrip property is stated as holding at every origin. The page's CSS size is
   unaffected.
2. **Section 7, sessions and tabs.** The chrome is two rows, not one: the page
   viewport is the grid less two. The sentence about background tabs holding no
   screencast is left for M5, but "keep their target alive but idle" is given its
   precise meaning in section 3 above: a tab extracts once when it opens and then only
   while focused.
3. **Section 8, too many tabs.** Eviction moves from M4 to M7, with lazy restore,
   for the reason in section 3.
4. **Section 11, M4.** "Background-tab idling and eviction" becomes "background-tab
   idling"; eviction is listed under M7. Adoption of targets a page opens for itself
   is added, since the milestone list did not mention it and a browser with tabs that
   cannot follow a `target=_blank` link is not one.

## 11. Failure modes

Section 8 of the parent spec holds throughout: never blank the frame you are looking
at.

| Failure | Behaviour |
|---|---|
| The profile is held by another instance | Temporary profile, `private session` in the statusline, no session file written. |
| The data directory cannot be created | Temporary profile and a notice. Browsing is unaffected. |
| The session file is missing | First run: one tab, no notice. |
| The session file is malformed or a future version | One tab and a notice. Never an exit, and the bad file is left alone rather than overwritten until there is something to write. |
| A restored tab fails to navigate | It stays a tab showing its error page, like any failed navigation. |
| `Target.createTarget` fails | `Job::Opened` carries the error, the tab is removed, and the statusline says so. The focused tab is untouched. |
| A job returns for a closed tab | Dropped. This is what the non-reusing `TabId` is for. |
| `Target.closeTarget` fails | `Job::Noted`. The tab is already gone from the session's view; a leaked target is not worth a visible failure. |
| A save fails | `Job::Noted`. Browsing continues; the previous file is intact because the write was temp-then-rename. |
| An adopted target cannot be prepared | It is closed rather than kept, so there is never a tab whose document has no bootstrap in it. |
| A background tab hangs | It hangs alone. The foreground stays responsive, which is the visible payoff of every page operation being spawned. |

## 12. Testing

Per parent spec section 9, the subtle logic lives where it can be tested without a
browser.

| Crate | Tests |
|---|---|
| `wwt-frame` | The roundtrip property extended over origins. `to_css` of a frame row below the origin, and `to_cell` of a point above it. |
| `wwt-ui` | `tab_bar` painting: the focused tab marked, titles elided to fit, a window around the focus when there are more tabs than columns, one tab and zero tabs. |
| `wwt` | Everything in sections 3, 4 and 9 that does not need a browser, which is most of this milestone. Focus movement and wrapping; closing the focused tab, the last tab, and a background tab; a job returning for a closed tab being dropped; a dirty signal for a background tab setting a flag and emitting no effect; that flag being spent on switch; a late `Job::Hints` for an unfocused tab not opening hint mode; effects naming the focused tab and no other; a resize emitting one `SetViewport` per tab. `Snapshot` round-tripping through serde; XDG resolution with `XDG_DATA_HOME` set, unset, and both unset; a malformed file, an empty tab list and an out-of-range focus index all producing a usable state. |
| `wwt-cdp` | Two launches on one profile directory, asserting the second falls back rather than hanging or succeeding. |
| `wwt-page` | Browser tests: two targets on one client extracting independently; `scroll_to` landing at an offset a later extraction reports; adoption of a target opened by a click on a `target=_blank` link, asserting the bootstrap is present in the adopted document (the click is the point: `window.open` from an evaluation has no user activation behind it, so the popup blocker returns null, and a link opened without a gesture carries no `openerId` to recognise it by); `activate` making a target the one that answers input. |

The measurement M4 owns is the switch: how long from `J` to the neighbouring tab's
cached frame on screen, recorded in the plan the way extraction and hints were. It
should be a repaint and no round trip, and the number is what proves it.

## 13. Open questions

**Where the tab bar's colours come from.** The frame has a foreground colour per cell
and no background, so the focused tab is marked with a glyph and bold rather than with
a highlight. This is fine at three tabs and probably poor at twelve. Background
colours are a known limitation from M1 and out of scope here, but the tab bar is the
first piece of chrome that really wants one.

**Whether a tab should keep a scroll offset that Chromium already keeps.** A
background target holds its own scroll position, so `scroll_y` in the tab record is
duplication that exists for the session file. It is also the thing that makes restore
work, so it stays; but if M7's eviction lands, the two copies will need a rule about
which one wins.

**Session file growth.** Nothing prunes it, and there is no history in it. Whether
history belongs in the same file or a different one is a question for whenever history
becomes something you can search rather than walk.
