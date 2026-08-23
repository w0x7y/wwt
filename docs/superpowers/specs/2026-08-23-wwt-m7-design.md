# wwt M7 — Hardening

**Date:** 2026-08-23
**Status:** Approved, pre-implementation
**Parent spec:** `2026-08-19-wwt-design.md` (sections 7, 8 and 11 govern here).

This is a delta against the system design, not a replacement for it. Where the two
disagree the parent spec wins and this document is wrong, except for the amendments in
section 10, which change the parent spec itself.

## 1. What M7 delivers

M4 through M6 built a browser worth using all day, and every milestone since M4 has
added something a crash now loses. M7 is the milestone that stops losing it.

Four failures have been named in section 8 and answered with nothing. Chromium dies and
every tab dies with it. A page wedges and swallows a keystroke for thirty seconds
without saying so. Twenty restored tabs are twenty Chromium targets opened at once,
whether or not you will look at any of them. And `State::Stalled` has existed in the
statusline since M1 with nothing in the codebase that sets it.

At the end of M7, killing Chromium out from under wwt costs you the frame you were
reading for as long as a relaunch takes, and nothing else. A page stuck in a JavaScript
loop is labeled within five seconds and stays switchable away from. A session of thirty
tabs holds eight targets. And the number is yours to choose, because M7 is also where
wwt grows a configuration file.

### In scope

A `Presence` on every tab, replacing `opened`. `Effect::Detach` and the reattach that
reuses `Effect::OpenTab`. The supervisor: websocket close as a signal, backoff,
relaunch, and a session that survives it. Per-command deadlines in two classes, a
typed timeout, and the `Stalled` rule that reads it. Eviction of background targets past
a limit. Lazy restore at startup. A `config.toml` with three keys, and the `toml` crate
that reads it.

### Out of scope

Reader mode and the reflow renderer, which are M8's and depend on none of this.
Restoring form contents or per-tab history across a restart, which would mean
serializing what a page is rather than where it is. A supervisor for anything but
Chromium. Recovering a wedged tab automatically, which section 5 rejects on purpose.
Any config key beyond the three named in section 8.

## 2. Architecture

### One state, three features

Eviction, lazy restore and the restart path all want the same thing: a tab that is real
to the person using it and has no Chromium target behind it. Built once it is a spine;
built three times it is the same hard problem solved three ways, each with its own
window in which an effect can name a page nobody holds.

So M7 builds **detachment** first, and the other three are pointings of it:

- Eviction detaches one background tab.
- A dead Chromium detaches all of them at once.
- Lazy restore starts every tab but the focused one already detached.

The discovery that makes this cheap is that most of it is already written. `Tab::opened`
already means "a target exists for this tab", and `Core::spawn` already drops every
effect naming a tab that has no page. That window is documented in CLAUDE.md as the
thing an in-flight flag must not outlive. Detachment is that same window, entered
deliberately instead of transiently.

The reattach is written too. `Effect::OpenTab` already carries a URL *and* a scroll
offset, for the reason M4 recorded: restoring a tab is opening it rather than opening
and then scrolling, or an extraction wins the race and writes down offset zero. It
already reports `Job::Opened`, and `Job::Opened` already starts the screencast on a tab
that has just appeared and triggers its first read. A reattach is an open, and it
inherits every rule that already holds for one.

What M7 adds to the tab lifecycle is therefore one effect and one widened flag.

### Crate deltas

| Crate | What changes |
|---|---|
| `wwt-frame` | Nothing. |
| `wwt-png` | Nothing. |
| `wwt-cdp` | `Chromium::launch` gains the configured binary path. A typed `Timeout` error. `Client::call_with` taking a deadline; `call` and `call_on` keep the default. |
| `wwt-page` | Each operation names its deadline class. Nothing else. |
| `wwt-term` | Nothing. |
| `wwt-ui` | `normalize_url` takes the search template as a parameter. |
| `wwt` | `Presence`, `Effect::Detach`, `Effect::Relaunch`, `Event::BrowserLost`, `Event::BrowserBack`, `Failure`, `Finished::Relaunched`, eviction, lazy restore, and `config.rs`. |

The weight is in the binary, which is where the rules are, which is where they can be
tested without a browser. That is the seam working.

## 3. Presence

`Tab::opened: bool` becomes:

```rust
enum Presence {
    /// A target was asked for and `Job::Opened` is coming.
    Opening,
    /// A target exists. The only state in which an effect naming this tab
    /// is not dropped.
    Attached,
    /// No target, and none is coming until this tab is focused.
    Detached,
}
```

The two states without a target must be told apart or a detached tab is waited on
forever: `Opening` has an answer in flight and `Detached` has nothing, and the whole
difference between them is whether focusing the tab should ask for a target. A single
bool cannot carry that, which is why this is a widening rather than a second flag.

`Attached` is exactly what `opened == true` meant, so every existing rule that reads it
is unchanged in meaning. In particular the rule from M4 stands verbatim: an in-flight
flag must not be set beside an effect emitted while the tab is not `Attached`, because
`Core` will drop that effect and nothing will ever clear the flag.

Detaching sets `Presence::Detached` and emits `Effect::Detach(id)`, which closes the
target and keeps the tab. It also clears `reading`, `navigating` and `hinting`, whose
answers are no longer coming, and drops `hints`, which are geometry belonging to a
document that is about to stop existing. It keeps `runs`, `caret`, `title`, `url`,
`scroll_y`, `progress` and `state`, which are what the tab looked like, and what makes
switching back to it a repaint.

`dirty` is set. A reattached page is a new document, and nothing about the old one's
runs is authoritative any more, even though they are what gets painted first.

`degraded` is cleared, for the reason M6 clears it on navigation: a new document
reinstalls `bootstrap.js`, so the tab has earned another attempt at the fast path.

## 4. The supervisor

### Ownership

`Chromium` is owned by `main` today and `Core` never sees it. That cannot stay: the
thing that restarts a browser has to hold it. `main` hands the browser to `Core` in
`Startup`, and the first act of a relaunch is to drop the old one.

Dropping first is not tidiness. `Chromium` is `kill_on_drop`, and the profile directory
is the lock: Chromium refuses a `--user-data-dir` another Chromium holds. Relaunching
onto the profile while our own dying browser still holds it is the one failure this path
would inflict on itself, and it would present as the relaunch inexplicably falling back
to a private session.

### Detection

The websocket closing is the signal, as section 8 says. `Client::read_loop` already
clears its subscribers when the stream ends, so `cdp.recv()` returning `None` is that
signal arriving.

The arm must then be **guarded off** for the rest of the run, the way `resize_at` and
`save_at` already guard theirs. A closed `UnboundedReceiver` returns `None` immediately
and forever, and an unguarded arm would take the `select!` every time it is polled: the
loop would spin at one hundred percent CPU while showing a frozen page, which is a worse
failure than the one being handled.

One signal, not two. A CDP call that fails with "the connection is closed" before the
subscriber channel drops arrives as an ordinary failed job and is treated as one. Making
every call site a second death detector would mean every one of them needs a rule about
which of the two paths runs first.

### The turn

1. `Event::BrowserLost`. `Session` marks every tab `Detached` with the clearing described
   in section 3, sets a notice, and emits `Effect::Relaunch`. Nothing else about the
   frame changes: the runs, the tab bar and the focus are all still true, and the only
   thing that is not is the statusline.
2. `Effect::Relaunch` spawns off the loop. Nothing blocks the loop, including this. The
   task drops the old `Chromium`, then attempts `Chromium::launch(profile)`,
   `Client::connect` and `auto_attach`, backing off between attempts.
3. The result comes back on the `Finished` channel rather than as a `Job`, for the reason
   a `Page` never reaches `Session`: a `Chromium` and a `Client` belong to `Core`.
   `Finished::Relaunched` mirrors `Finished::Opened` exactly, including that it is
   handled in `run` rather than in `apply`, because `run` is where the `cdp` subscription
   is in scope and it is the subscription that has to be replaced.
4. On success `Core` installs the browser and the client, resubscribes, and reports
   `Event::BrowserBack`. `Session` reattaches the focused tab with `Effect::OpenTab`
   carrying its URL and scroll offset, and leaves every other tab detached. **The restart
   path is lazy restore**, arrived at from the other direction.
5. On exhausted backoff `Core` reports `Job::Relaunched(Err(_))`. `Session` says so in
   the statusline and stays up on stale frames. The job carries a
   `Result<(), String>` and is only ever reported with the error: success is
   `Event::BrowserBack`, because the browser arriving is a thing that happened to
   `Core` before it is a thing the session is told about, exactly as `Finished::Opened`
   files a page before reporting `Job::Opened`.

Backoff is three attempts at 250ms, 1s and 4s. `Chromium::launch` carries its own 20s
startup timeout, which dominates the worst case: a browser that starts and never
announces an endpoint costs a minute before wwt gives up. That is accepted. The
alternative is a second deadline over the first, and the case it would improve is a
machine that is already not working.

`Session` decides *that* a relaunch is attempted; `Core` decides how many times and how
far apart. A count and a delay are machinery.

### Trying again

After a failed relaunch, any keystroke whose action would touch the page emits
`Effect::Relaunch` again instead of what it would have done. Scrolling a page there is
no browser for is not a thing to report an error about; it is a thing to interpret as
"please try".

A `relaunching` flag guards it, and it is a fourth instance of the pattern named in
CLAUDE.md: a held `j` must not spawn thirty relaunches. It is cleared by
`Event::BrowserBack` and by `Job::Relaunched(Err)`, which is the same shape as
`Job::Hints` being the one thing that clears `hinting` however it went.

Retrying on a keystroke and never on a timer is deliberate. An idle wwt costs ~zero CPU,
and that rule does not get an exception for the state where there is nothing to be busy
about.

## 5. Deadlines and stalling

### Two classes

`CALL_TIMEOUT` is a flat 30 seconds today. Navigation keeps it, because a real page on a
bad network legitimately takes that long. Everything else drops to **5 seconds**.

The numbers are already measured: an extraction is ~4ms, a status read is under 1ms, the
worst snapshot of `heavy.html` is ~26ms, and a hint query is in the same range. Five
seconds is two hundred times the slowest thing ever measured on this codebase's own
slowest fixture. What the flat 30s costs is the case the deadline exists for: press `j`
on a wedged page and nothing whatsoever happens for half a minute, no keystroke, no
label, no explanation.

Two classes rather than five. Each of the five would need its own reason and its own
measurement, and the two that exist here are "how long a network may take" and "how long
our own script may take", which really are different questions.

### A typed timeout

A timeout must be distinguishable from a script that threw, or `Session` cannot tell
`Stalled` from `Error` and cannot make the decision below about degrading at all. `wwt-cdp`
returns a typed error for it, using the `thiserror` already in the dependency set, and
`Core` asks `anyhow` to downcast when it builds the job.

Jobs that can carry a failure carry a `Failure` rather than a `String`:

```rust
enum Failure {
    TimedOut,
    Failed(String),
}
```

on `Job::Extracted`, `Job::Status`, `Job::Hints` and `Job::Failed`. `Core` reports what
happened and `Session` decides what it means, which is the seam M6 drew when the effect
started naming its source rather than the page carrying a flag.

### A timeout does not degrade

M6's rule sends a failed `Source::Script` read to `DOMSnapshot`. **A timeout is exempt.**

A script that threw is a page our extractor cannot read, and the snapshot is a different
extractor that might. A page that did not answer in five seconds is a page whose main
thread is not running, and `DOMSnapshot.captureSnapshot` needs that same main thread.
Asking it would cost a second deadline to learn the same thing, and would end with the
tab marked degraded for the rest of its life over a wedge that may last a second.

`Failure::TimedOut` sets `State::Stalled` and stops. `Failure::Failed` keeps M6's
behaviour exactly.

### A stalled tab needs no retry policy

There is nothing to schedule, which is the pleasant surprise in this milestone.

A page wedged in a JavaScript loop cannot run its own `MutationObserver`, its own scroll
listener or its own field-state events, so it emits no dirty signal and nothing re-asks.
A page that comes back emits one on the first mutation and is read normally, clearing
`Stalled` the way any successful read clears it. The event-driven rule from M2 is what
makes a retry policy unnecessary: polling would have needed one.

Keystrokes still reach a stalled tab and still cost a deadline each, bounded by how often
a person presses a key. The statusline says `[stalled]`, so what is happening is legible.

Recovery is `:reload` or `r`, and it is never automatic. A wedged page may be one you are
half way through a form on, and detaching its target unasked throws that away to fix
something the label already described. It also keeps a rule worth keeping: a target dies
when the session decided it should.

## 6. Eviction

`Tab` gains `focused_at: u64`, taken from a counter in `Session` and never from a clock,
so the recency rule is asserted with data and the tests need neither a browser nor time.

After any focus change, if the number of `Attached` tabs exceeds `max_tabs`, the least
recently focused **eligible** tab is detached. Eligible means `Attached`, not focused,
and carrying no in-flight flag.

The in-flight exclusion is the part that is easy to get wrong. A background tab mid
navigation has a `url` field that still names where it is leaving, so detaching it and
reattaching later would take you back to the page you navigated away from. A tab with a
read in flight would have its answer dropped by `Core` and its `reading` flag cleared by
the detach, which is correct but wasteful.

If nothing is eligible, nothing is evicted. **The limit is a target and not a
guarantee.** A wwt holding nine targets because the ninth is mid-navigation will hold
eight again the next time you switch tabs, and the alternative is racing an answer that
is already on its way in order to honour a number that exists to bound memory.

`max_tabs` counts live targets including the focused one, because that is what costs
memory. It does not count tabs, which are cheap and which the tab bar shows all of.

Eviction emits no `Effect::Save`. A detached tab's URL, title and scroll offset are
exactly what they were, and section 7 of the parent spec says a write happens when one of
those changed.

## 7. Lazy restore

`Session::begin` opens the focused tab and leaves every other restored tab `Detached`.

The tab bar is complete on the first frame, because titles and URLs come from the session
file, which has stored both since M4. Startup launches one page rather than however many
were open, which on a thirty tab session is the difference between a browser that is
ready and a browser that is opening thirty targets you have not asked to see.

A tab restored this way has a title and no runs, so switching to it looks exactly like
opening a new tab looks today: `State::Loading` and an empty page area for one round
trip. That is a different experience from switching to an *evicted* tab, which paints its
cached runs immediately and reattaches behind them, keeping M4's repaint guarantee and
the `measure_switch` number that holds it down. Both are correct, and the difference is
real: one of them has been read and one of them has not.

A URL given on the command line opens a tab beside the restored ones, as it does today,
and that tab is focused, so it is the one that opens eagerly.

## 8. Configuration

`$XDG_CONFIG_HOME/wwt/config.toml`, and `~/.config/wwt/config.toml` when that variable is
unset or empty. Resolution passes the environment in as parameters, matching
`store::data_dir_from` and for the same reason: environment variables are process global
and tests run in threads.

```toml
max_tabs = 8                              # live targets, the focused one included
search = "https://duckduckgo.com/?q={}"   # {} takes the percent-encoded query
chromium = "/usr/bin/chromium"            # WWT_CHROMIUM still wins over this
```

**A new dependency, on purpose.** CLAUDE.md fixes the dependency set and says to stop and
ask. Asked and answered: `toml` goes into the workspace dependencies for this. The
alternatives were a hand-rolled `key = value` parser in the spirit of `wwt-png`, and JSON
through the `serde_json` already present, and TOML was chosen as the format people expect
from a terminal tool and the one they can annotate.

Three keys and no more. Every other candidate stays a constant until something needs it,
because a configuration file fills up with settings nobody asked for unless it is
defended.

- `max_tabs` is section 6's limit. Default 8.
- `search` is where anything that is not a URL goes. It replaces the `SEARCH` constant in
  `wwt-ui/src/command.rs`, whose own doc comment says "making this a setting is a
  configuration question, and there is still no configuration". `{}` is required and is
  substituted with the output of the existing `as_query`, which already percent-encodes
  per byte. A template without `{}` is a bad value: notice, and the default is used.
  `normalize_url` takes the template as a parameter and reads nothing, because `wwt-ui`
  depends on `wwt-frame` alone and that rule is not being spent on a string.
- `chromium` is a binary path. Precedence is `WWT_CHROMIUM`, then this, then the `PATH`
  search over the three candidate names. The environment wins because it is the more
  specific thing: a variable is set for one run, a file is written for all of them.

A missing file is the defaults, silently, and is the normal case. A malformed file, an
unknown key or a bad value is a **notice in the statusline and the defaults**, never an
exit. This is the treatment the session file already gets, for the reason section 8 gives
for everything: a browser that will not start because of a typo in a config file is worse
than one that starts and tells you.

## 9. What survives, and what does not

Across a Chromium restart, or an eviction and a reattach:

**Kept:** the tabs, their order, the focus, each tab's URL, title and scroll offset, and
the runs that were last painted. Pixel mode, which is global. Cookies and logins, because
the profile is a directory and is relaunched onto.

**Lost:** form contents, which would mean serializing what a page *is* rather than where
it is. Per-tab back and forward history, which is Chromium's and dies with the target.
The `degraded` flag, deliberately, because a new document deserves the fast path. Hint
targets, which are geometry belonging to a document that no longer exists.

Section 8 of the parent spec already promised exactly this: "scroll positions survive;
form contents do not". M7 adds history to the list of casualties, which is the honest
part of eviction and is written into the amendments below.

## 10. Amendments to the parent spec

Four, to be made in the same commit as the implementation they describe.

1. **Section 8, "Chromium dies."** The restart rebuilds tabs from the live session state
   and not from the session file. The file is a debounced copy, up to `SAVE_DEBOUNCE`
   behind, so rebuilding from it would discard up to a second of navigation at the moment
   least worth discarding it. The file remains what a cold start reads.
2. **Section 8, "Too many tabs."** The deferral to M7 is discharged. "Configurable" is
   now `max_tabs` in `config.toml`, and the limit is a target rather than a guarantee: a
   tab with work in flight is not evicted.
3. **Section 8, "Page hangs."** Deadlines are two classes, 30s for navigation and 5s for
   everything else, and a timeout is typed so that it can be told from a script that
   threw. A timed-out read does not fall back to `DOMSnapshot`; that fallback answers a
   different failure.
4. **Section 7.** Restore is lazy: the focused tab opens and the rest start detached. The
   sentence about a tab being read once when it opens still holds, and now says something
   slightly different, because a lazily restored tab has not opened yet.

A fifth, smaller: the parent spec's statement that the whole configuration surface is one
flag and two environment variables is no longer true, and section 8 above is what
replaces it.

## 11. Failure modes

- **The relaunch will not take.** Three attempts, then a notice, then stale frames and a
  retry on the next keystroke that would have touched the page. Never an exit, and never
  a blank frame.
- **The relaunch takes but the profile is gone.** Chromium creates a profile directory it
  cannot find, so this presents as a session with no cookies rather than as an error.
  Nothing to do about it and nothing that needs doing.
- **The relaunch falls back to a private session.** It does not: the fallback in `main`
  is a startup path. A relaunch that cannot have the profile is a failed attempt and
  backs off, because the alternative is silently continuing without the cookie jar that
  was the reason for holding a profile at all.
- **A tab reattaches to an error page.** It is a navigation like any other, and
  `Core::on_job` already sets `State::Error` from the `chrome-error://` scheme.
- **Eviction with one tab open.** Not reachable: the focused tab is never eligible, and
  `max_tabs` is validated to be at least one when the file is read. A configured
  `max_tabs = 0` is a bad value, so it is a notice and the default.
- **A frame arrives for a detached tab.** Acked and dropped, exactly as a frame for a
  background tab already is, because Chromium counts acks and not paints. A frame for a
  tab whose target is gone is the existing exception and needs nothing new.
- **The browser dies during a relaunch.** Not reachable: there is no browser to die, and
  the subscription that would report it has not been made yet.

## 12. Testing

Everything that is a rule is a `Session` test with no browser, which is most of the
milestone:

- Detachment clears the in-flight flags, drops hints, keeps the runs, sets dirty, clears
  degraded.
- An effect naming a detached tab is not emitted at all, which is the `Tab::opened` rule
  under its new name.
- `Event::BrowserLost` detaches every tab and asks for exactly one relaunch.
- `Event::BrowserBack` reattaches exactly the focused tab, with its scroll offset.
- A keystroke after a failed relaunch asks again; a held key asks once.
- Eviction picks the least recently focused, skips the focused tab, and skips a tab with
  work in flight; nothing eligible means nothing evicted.
- `Session::begin` on a restored snapshot opens one page and detaches the rest.
- `Failure::TimedOut` sets `Stalled` and does *not* ask for a snapshot;
  `Failure::Failed` still does.
- Switching to an evicted tab paints its cached runs before the reattach lands.

The config parser is asserted on with data and no file: each key, an unknown key, a bad
value, a `search` without `{}`, and an empty file.

Two integration tests earn a browser. One kills a Chromium out from under a live core and
asserts the tabs come back with their scroll offsets. One asserts that a `wwt-cdp` call
past its deadline produces the typed timeout and not a string, which is what the whole of
section 5 rests on.

`measure_switch` gains a detached case, so the difference between a repaint and a round
trip stays a number rather than a claim.

## 13. Open questions

None blocking. Two noted:

- **Whether `max_tabs` should have a default of 8 at all.** It is a guess at what a
  terminal browser user has open, made without telemetry and without a second opinion. It
  is trivially changed and now trivially overridden, which is the whole reason section 8
  exists.
- **Whether eviction should prefer a degraded or stalled tab over a merely old one.** It
  probably should: those tabs cost the most and are worth the least. Left out because
  least-recently-focused is one rule and this would be two, and the second one has no
  measurement behind it yet.
