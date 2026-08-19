# Webinal

A terminal web browser in Rust. It drives a real headless Chromium over the
Chrome DevTools Protocol and renders pages into the terminal grid: crisp text
by default, true pixels on demand.

**Status: M2, navigation and reading.** It renders a page, scrolls it,
follows history, and opens other URLs. Clicking, typing, tabs, and pixel
mode are M3 through M5.

## Requirements

- Rust 1.97+
- Chromium (`sudo pacman -S chromium`), or `WEBINAL_CHROMIUM` set to a
  Chromium binary
- A terminal that reports its pixel dimensions; Kitty is the development
  target

## Usage

    cargo run -p webinal -- example.com

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
| `wb-frame` | Coordinate model and the cell grid. No I/O. |
| `wb-cdp` | Chromium launcher and CDP client. |
| `wb-page` | Text-run extraction from a live page. |
| `wb-term` | Terminal probing and rendering. |
| `webinal` | The binary. |

## Documentation

- Design: `docs/superpowers/specs/2026-08-19-webinal-design.md`
- M2 design: `docs/superpowers/specs/2026-08-19-webinal-m2-design.md`
- M2 plan: `docs/superpowers/plans/2026-08-19-webinal-m2-navigation.md`
- M1 plan: `docs/superpowers/plans/2026-08-19-webinal-m1-walking-skeleton.md`
