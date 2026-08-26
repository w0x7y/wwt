# wwt

World Wide Terminal: a web browser in Rust. It drives a real headless
Chromium over the Chrome DevTools Protocol and renders pages into the
terminal grid: crisp text by default, true pixels on demand.

**Status: M8, reader mode.** It renders a page, scrolls it, follows
history, opens other URLs, reaches every link from the keyboard, types
into forms, and clicks with the mouse. It keeps many pages open at once
under one Chromium, follows the links that want a new tab, and comes
back to the same tabs, still logged in, tomorrow. And on a keypress it
shows you the page as it really looks, pixels and all, with the
keyboard still yours. Reader mode selects the page's main readable content,
reflows it to the terminal width, and leaves the live page standing exactly
where it was. When a piece of it does not work, such as a page that
breaks wwt's own script, a page wedged in a loop, a terminal that cannot
show a picture, a Chromium that died — it keeps going with a worse
version rather than stopping, and starting it costs one page however
many tabs you left open.

## Requirements

- Rust 1.97+
- Chromium (`sudo pacman -S chromium`), or `WWT_CHROMIUM` set to a
  Chromium binary
- A terminal that reports its pixel dimensions; Kitty is the development
  target

## Install

Install WWT as a system application:

    make
    sudo make install

This installs `wwt` in `/usr/bin` and adds World Wide Terminal to the app
launcher. The desktop entry also advertises WWT as a handler for HTTP, HTTPS,
and HTML files. The launcher opens WWT in Kitty unless `config.toml` selects a
different terminal.

To install only for your account, use:

    make
    make install PREFIX="$HOME/.local"

To remove a system installation, run:

    sudo make uninstall

To remove a per-user installation, run:

    make uninstall PREFIX="$HOME/.local"

The icon is yours to create. Save a scalable SVG as `assets/wwt.svg`, then run
the install command again. The installer places it in the hicolor icon theme
under the name `wwt`.

## Usage

    wwt                       # the tabs you had open last time
    wwt example.com           # those, and this one beside them
    wwt rust terminal browser # search for an unquoted phrase
    wwt --new                 # one blank tab, keeping the old session on disk

To run the source tree without installing it, replace `wwt` with
`cargo run -p wwt --` in these commands.

| Key | |
|---|---|
| `j` `k` | scroll a line |
| `d` `u` | scroll half a screen |
| `space` `b` | scroll a screen |
| `g` `G` | top, bottom |
| `H` `L` | back, forward |
| `Alt-1` … `Alt-9` | go to the first tab through the ninth |
| `t` | open a tab |
| `x` | close this tab |
| `Ctrl-r` | reload |
| `o` | open a URL |
| `:` | command line |
| `i` | hand the keyboard to the page |
| `f` | label every link and button; type a label to click it |
| `p` | show the page as it really looks |
| `r` | reflow the main readable content; press again to return |
| `Esc` | take the keyboard back |
| `Ctrl-]` | send the page a literal Escape |
| `q` | quit |

`p` swaps the page between text and true pixels without moving it: the
same viewport, the same scroll offset, the same tab, and hint labels
still readable on top of the picture. On a terminal that speaks the
Kitty graphics protocol, which wwt asks about once at startup, it is a
picture; on one that does not it is half-block colour, which is the same
page at half the vertical resolution rather than a refusal. `:set pixel
on` and `:set pixel off` do the same from the command line.

`r` selects the dominant `article` or `main` content, removes site
furniture, and reflows headings, paragraphs, lists, quotes and code to the
terminal width. Reader scrolling is separate from the page underneath, so
a second `r` returns to the original page position. Press `f` to follow a
visible reader link. Press `i` when you need the live page and its controls;
it leaves reader mode and hands the keyboard to that page.

A page that breaks wwt's injected script is not lost: that tab says
`[degraded]` and is read through Chromium's own DOM snapshot instead.
You keep reading, scrolling, hinting and typing; you lose the insertion
point and the wrapping inside a text field until the tab navigates.

`Esc` is never forwarded to the page, so the keyboard is always one key
away from being yours again. `Ctrl-]` exists for pages that want an
Escape of their own, because a terminal transmits `Ctrl-[` as the byte
`0x1B`, which *is* Escape.

The mouse is captured at startup: clicks and the wheel go to the page,
which costs your terminal's own text selection. Most terminals hand it
back while shift is held; `:set mouse off` is there for the ones that do
not.

Alt and a digit goes straight to that tab, so the one you want is one
keystroke away however many are open and wherever you are now. Past the
ninth there is `:tabnext` and `:tabprev`, which still cycle.

The number row on its own is not a tab, and neither is the punctuation
above it. Both are being kept for what comes later. Alt is also the
modifier a terminal will actually tell us about: shift and `1` reaches
us as `!` and nothing more, so a shift binding would have to guess at
which glyph your keyboard prints there, while alt and `1` is alt and `1`
on every keyboard in the world. On a French one, where the number row is
punctuation, `Alt-Shift` and that key is the tab, because shift is how
you type a digit there anyway.

Some terminals bind alt and a digit for their own tab switching, Konsole
among them. If yours does, it never reaches us, and that is a setting on
their side.

`:open` and `:tabopen` take a URL or anything else: `:open banana`
searches DuckDuckGo for it rather than telling you it is not a URL, and
so does `wwt banana`. A word with a dot in it, or a host and a port like
`localhost:3000`, is still somewhere to go.

Commands: `:open <url-or-search>`, `:tabopen <url-or-search>` (`:t`),
`:tabclose`, `:tabnext`, `:tabprev`, `:back`, `:forward`, `:reload`,
`:login`, `:set mouse on|off`, `:set pixel on|off`, `:quit`.

`:login` saves the current tabs, stops WWT's headless Chromium, and opens
Google Accounts in an ordinary Chromium window on the same profile. Sign in,
then close the Chromium window. WWT restarts its headless browser and restores
the tabs. The command is unavailable in a private session because that run
does not own the persistent profile.

wwt keeps a Chromium profile at `$XDG_DATA_HOME/wwt/profile` and the tabs
you had open at `$XDG_DATA_HOME/wwt/session.json`. The profile is what
makes logins durable, and it is also the lock: a second wwt cannot have
it, so it runs private, not logged in, and writes no session file.

Starting wwt loads one page, whatever the tab bar says: the tab you were
looking at. The rest are real tabs with their titles and addresses
already in the bar, and each loads when you first reach it. The same
thing happens in the other direction while you browse: past `max_tabs`
live pages, the tab you looked at longest ago gives up its page and
keeps everything else, so switching back paints what it looked like
straight away and reloads behind that.

A page stuck in a loop of its own says `[stalled]` rather than freezing
wwt: you can still switch away from it, and `Ctrl-r` is how it comes
back. If Chromium itself dies, the page you were reading stays on the
screen, wwt starts another one, and your tabs come back where they were.
If it cannot, the frame still stays and the next key tries again.

## Configuration

`$XDG_CONFIG_HOME/wwt/config.toml`, which wwt only ever reads. Having no
such file is the ordinary case; a file wwt cannot make sense of is a
notice in the statusline and the defaults, never a refusal to start.

    max_tabs = 8                            # live pages, the one in front included
    search = "https://duckduckgo.com/?q={}" # where anything that is not a URL goes
    chromium = "/usr/bin/chromium"          # which browser to launch
    terminal = ["kitty", "-e"]              # terminal used by the app launcher

`max_tabs` counts pages and not tabs: the bar goes on showing all of
them however many are loaded. `search` wants `{}` where the query goes.
`chromium` is a path, and `WWT_CHROMIUM` still beats it, because a
variable is set for one run and a file is written for all of them. `terminal`
is a command and its arguments. For example, use `["alacritty", "-e"]` to
open launcher requests in Alacritty. This setting does not affect `wwt`
commands that you run in an existing terminal.

## Layout

| Crate | Responsibility |
|---|---|
| `wwt-frame` | Coordinate model and the cell grid. No I/O. |
| `wwt-reader` | Semantic documents and pure terminal-width reflow. |
| `wwt-png` | Base64 and PNG, decoded here so nothing is depended on. |
| `wwt-cdp` | Chromium launcher and CDP client. |
| `wwt-page` | Text-run extraction from a live page. |
| `wwt-term` | Terminal probing and rendering. |
| `wwt-ui` | Modes, chrome, `:` commands, hint labels. |
| `wwt` | The binary. |

## Documentation

- Design: `docs/superpowers/specs/2026-08-19-wwt-design.md`
- M8 design: `docs/superpowers/specs/2026-08-24-wwt-m8-design.md`
- M8 plan: `docs/superpowers/plans/2026-08-24-wwt-m8-reader-mode.md`
- M7 design: `docs/superpowers/specs/2026-08-23-wwt-m7-design.md`
- M7 plan: `docs/superpowers/plans/2026-08-23-wwt-m7-hardening.md`
- M6 design: `docs/superpowers/specs/2026-08-23-wwt-m6-design.md`
- M6 plan: `docs/superpowers/plans/2026-08-23-wwt-m6-degradation.md`
- M5 design: `docs/superpowers/specs/2026-08-22-wwt-m5-design.md`
- M5 plan: `docs/superpowers/plans/2026-08-22-wwt-m5-pixel-mode.md`
- M4 design: `docs/superpowers/specs/2026-08-21-wwt-m4-design.md`
- M4 plan: `docs/superpowers/plans/2026-08-21-wwt-m4-tabs-and-sessions.md`
- M3 design: `docs/superpowers/specs/2026-08-19-wwt-m3-design.md`
- M3 plan: `docs/superpowers/plans/2026-08-19-wwt-m3-interaction.md`
- M2 design: `docs/superpowers/specs/2026-08-19-wwt-m2-design.md`
- M2 plan: `docs/superpowers/plans/2026-08-19-wwt-m2-navigation.md`
- M1 plan: `docs/superpowers/plans/2026-08-19-wwt-m1-walking-skeleton.md`
