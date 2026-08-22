# wwt

World Wide Terminal: a web browser in Rust. It drives a real headless
Chromium over the Chrome DevTools Protocol and renders pages into the
terminal grid: crisp text by default, true pixels on demand.

**Status: M5, pixel mode.** It renders a page, scrolls it, follows
history, opens other URLs, reaches every link from the keyboard, types
into forms, and clicks with the mouse. It keeps many pages open at once
under one Chromium, follows the links that want a new tab, and comes
back to the same tabs, still logged in, tomorrow. And on a keypress it
shows you the page as it really looks, pixels and all, with the
keyboard still yours. Reader mode is M6.

## Requirements

- Rust 1.97+
- Chromium (`sudo pacman -S chromium`), or `WWT_CHROMIUM` set to a
  Chromium binary
- A terminal that reports its pixel dimensions; Kitty is the development
  target

## Usage

    cargo run -p wwt                 # the tabs you had open last time
    cargo run -p wwt -- example.com  # those, and this one beside them
    cargo run -p wwt -- --new        # one blank tab, keeping the old session on disk

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
| `Esc` | take the keyboard back |
| `Ctrl-]` | send the page a literal Escape |
| `q` | quit |

`p` swaps the page between text and true pixels without moving it: the
same viewport, the same scroll offset, the same tab, and hint labels
still readable on top of the picture. It needs a terminal that speaks
the Kitty graphics protocol, which wwt asks about once at startup;
without one it says so and leaves your text where it was. `:set pixel
on` and `:set pixel off` do the same from the command line.

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
`:set mouse on|off`, `:set pixel on|off`, `:quit`.

wwt keeps a Chromium profile at `$XDG_DATA_HOME/wwt/profile` and the tabs
you had open at `$XDG_DATA_HOME/wwt/session.json`. The profile is what
makes logins durable, and it is also the lock: a second wwt cannot have
it, so it runs private, not logged in, and writes no session file.

## Layout

| Crate | Responsibility |
|---|---|
| `wwt-frame` | Coordinate model and the cell grid. No I/O. |
| `wwt-cdp` | Chromium launcher and CDP client. |
| `wwt-page` | Text-run extraction from a live page. |
| `wwt-term` | Terminal probing and rendering. |
| `wwt-ui` | Modes, chrome, `:` commands, hint labels. |
| `wwt` | The binary. |

## Documentation

- Design: `docs/superpowers/specs/2026-08-19-wwt-design.md`
- M5 design: `docs/superpowers/specs/2026-08-22-wwt-m5-design.md`
- M5 plan: `docs/superpowers/plans/2026-08-22-wwt-m5-pixel-mode.md`
- M4 design: `docs/superpowers/specs/2026-08-21-wwt-m4-design.md`
- M4 plan: `docs/superpowers/plans/2026-08-21-wwt-m4-tabs-and-sessions.md`
- M3 design: `docs/superpowers/specs/2026-08-19-wwt-m3-design.md`
- M3 plan: `docs/superpowers/plans/2026-08-19-wwt-m3-interaction.md`
- M2 design: `docs/superpowers/specs/2026-08-19-wwt-m2-design.md`
- M2 plan: `docs/superpowers/plans/2026-08-19-wwt-m2-navigation.md`
- M1 plan: `docs/superpowers/plans/2026-08-19-wwt-m1-walking-skeleton.md`
