# Webminal

A terminal web browser in Rust. It drives a real headless Chromium over the
Chrome DevTools Protocol and renders pages into the terminal grid: crisp text
by default, true pixels on demand.

**Status: M1, the walking skeleton.** It renders one page's text and quits.
Navigation, input, tabs, and pixel mode are M2 through M5.

## Requirements

- Rust 1.97+
- Chromium (`sudo pacman -S chromium`), or `WEBMINAL_CHROMIUM` set to a
  Chromium binary
- A terminal that reports its pixel dimensions; Kitty is the development
  target

## Usage

    cargo run -p webminal -- https://example.com

Press `q` to quit.

## Layout

| Crate | Responsibility |
|---|---|
| `wm-frame` | Coordinate model and the cell grid. No I/O. |
| `wm-cdp` | Chromium launcher and CDP client. |
| `wm-page` | Text-run extraction from a live page. |
| `wm-term` | Terminal probing and rendering. |
| `webminal` | The binary. |

## Documentation

- Design: `docs/superpowers/specs/2026-08-19-webminal-design.md`
- M1 plan: `docs/superpowers/plans/2026-08-19-webminal-m1-walking-skeleton.md`
