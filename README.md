# wwt

World Wide Terminal: a web browser in Rust. It drives a real headless
Chromium over the Chrome DevTools Protocol and renders pages into the
terminal grid: crisp text by default, true pixels on demand.

**Status: M3, interaction.** It renders a page, scrolls it, follows
history, opens other URLs, reaches every link from the keyboard, types
into forms, and clicks with the mouse. Tabs and pixel mode are M4 and
M5.

## Requirements

- Rust 1.97+
- Chromium (`sudo pacman -S chromium`), or `WWT_CHROMIUM` set to a
  Chromium binary
- A terminal that reports its pixel dimensions; Kitty is the development
  target

## Usage

    cargo run -p wwt -- example.com

| Key | |
|---|---|
| `j` `k` | scroll a line |
| `d` `u` | scroll half a screen |
| `space` `b` | scroll a screen |
| `g` `G` | top, bottom |
| `H` `L` | back, forward |
| `Ctrl-r` | reload |
| `o` | open a URL |
| `:` | command line |
| `i` | hand the keyboard to the page |
| `f` | label every link and button; type a label to click it |
| `Esc` | take the keyboard back |
| `Ctrl-]` | send the page a literal Escape |
| `q` | quit |

`Esc` is never forwarded to the page, so the keyboard is always one key
away from being yours again. `Ctrl-]` exists for pages that want an
Escape of their own, because a terminal transmits `Ctrl-[` as the byte
`0x1B`, which *is* Escape.

The mouse is captured at startup: clicks and the wheel go to the page,
which costs your terminal's own text selection. Most terminals hand it
back while shift is held; `:set mouse off` is there for the ones that do
not.

Commands: `:open <url>`, `:back`, `:forward`, `:reload`,
`:set mouse on|off`, `:quit`.

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
- M3 design: `docs/superpowers/specs/2026-08-19-wwt-m3-design.md`
- M3 plan: `docs/superpowers/plans/2026-08-19-wwt-m3-interaction.md`
- M2 design: `docs/superpowers/specs/2026-08-19-wwt-m2-design.md`
- M2 plan: `docs/superpowers/plans/2026-08-19-wwt-m2-navigation.md`
- M1 plan: `docs/superpowers/plans/2026-08-19-wwt-m1-walking-skeleton.md`
