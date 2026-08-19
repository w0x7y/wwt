# wwt

World Wide Terminal: a web browser in Rust. It drives a real headless
Chromium over the Chrome DevTools Protocol and renders pages into the
terminal grid: crisp text by default, true pixels on demand.

**Status: M2, navigation and reading.** It renders a page, scrolls it,
follows history, and opens other URLs. Clicking, typing, tabs, and pixel
mode are M3 through M5.

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
| `q` | quit |

Commands: `:open <url>`, `:back`, `:forward`, `:reload`, `:quit`.

## Layout

| Crate | Responsibility |
|---|---|
| `wwt-frame` | Coordinate model and the cell grid. No I/O. |
| `wwt-cdp` | Chromium launcher and CDP client. |
| `wwt-page` | Text-run extraction from a live page. |
| `wwt-term` | Terminal probing and rendering. |
| `wwt` | The binary. |

## Documentation

- Design: `docs/superpowers/specs/2026-08-19-wwt-design.md`
- M2 design: `docs/superpowers/specs/2026-08-19-wwt-m2-design.md`
- M2 plan: `docs/superpowers/plans/2026-08-19-wwt-m2-navigation.md`
- M1 plan: `docs/superpowers/plans/2026-08-19-wwt-m1-walking-skeleton.md`
