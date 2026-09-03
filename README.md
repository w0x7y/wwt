# World Wide Terminal

World Wide Terminal (`wwt`) is a terminal web browser written in Rust. It
drives headless Chromium through the Chrome DevTools Protocol, so Chromium
still handles layout and JavaScript. WWT renders the result as terminal text
by default and can show the original page as pixels when needed.

The project is at milestone M8. It supports:

- Keyboard navigation, mouse input, forms, links, and browser history
- Multiple tabs under one Chromium instance
- Persistent tabs, cookies, and logins between runs
- A reader mode that reflows the main article to the terminal width
- A pixel mode with Kitty graphics and a half-block fallback
- Recovery from broken page scripts, stalled pages, and Chromium crashes

## Requirements

- Rust 1.97 or later
- Chromium on `PATH`, or `WWT_CHROMIUM` set to a Chromium binary
- A terminal that reports its pixel dimensions

Kitty is the development target. Other terminals can display pixel mode with
half-block characters if they do not support the Kitty graphics protocol.

## Install

Build and install WWT system-wide:

```sh
make
sudo make install
```

The default installation puts `wwt` in `/usr/bin` and adds a desktop entry
for HTTP, HTTPS, and HTML files. Desktop launches open WWT in Kitty unless
`config.toml` selects another terminal.

To install under your account instead, run:

```sh
make
make install PREFIX="$HOME/.local"
```

To remove a system installation, run:

```sh
sudo make uninstall
```

To remove an installation under your account, run:

```sh
make uninstall PREFIX="$HOME/.local"
```

The icon is optional. Add an SVG at `assets/wwt.svg` before installation to
install it under the `wwt` name in the hicolor icon theme.

## Run

```sh
wwt                       # restore the previous session
wwt example.com           # restore it and open another tab
wwt rust terminal browser # search for the full phrase
wwt --new                 # start with one blank tab
```

To run from the source tree, replace `wwt` with `cargo run -p wwt --`.

## Key bindings

These keys work in normal mode:

| Key | Action |
|---|---|
| `j`, `k`, `Down`, `Up` | Scroll one line |
| `d`, `u` | Scroll half a page |
| `Space`, `b`, `PageDown`, `PageUp` | Scroll one page |
| `g`, `G`, `Home`, `End` | Go to the top or bottom |
| `H`, `L` | Go back or forward |
| `Ctrl-r` | Reload |
| `Alt-1` through `Alt-9` | Select a tab |
| `t` | Open a new tab |
| `x` | Close the current tab |
| `o` | Open a URL or search |
| `:` | Open the command line |
| `i` | Send keyboard input to the page |
| `f` | Label visible links and controls |
| `p` | Toggle pixel mode |
| `r` | Toggle reader mode |
| `q` | Quit |

Press `Esc` to return from insert, hint, or command mode. WWT never sends
`Esc` to the page, so a page cannot trap the keyboard. In insert mode, press
`Ctrl-]` to send an Escape key to the page.

Some terminals use `Alt` plus a number for their own tabs. Disable that
terminal binding if WWT does not receive the shortcut. Use `:tabnext` and
`:tabprev` to reach tabs after the ninth.

## Page modes

Text mode is the default. Press `p` to replace the text with a screenshot of
the same viewport. WWT keeps the current tab, scroll position, and hint labels.
On terminals without Kitty graphics, pixel mode uses colored half-block
characters at half the vertical resolution.

Press `r` to extract the dominant `article` or `main` element and reflow its
headings, paragraphs, lists, quotes, and code. Reader mode has its own scroll
position. Press `r` again to return to the live page at its original position.
Use `f` to follow a visible reader link, or press `i` to leave reader mode and
interact with the live page.

If WWT's injected script fails, the tab is marked `[degraded]` and WWT reads
the page from Chromium's DOM snapshot. If a page stops responding, the tab is
marked `[stalled]`. Switch away or press `Ctrl-r` to retry it. WWT keeps the
last rendered frame while it restarts Chromium after a browser crash.

Mouse capture starts with WWT. Clicks and wheel events go to the page, which
prevents normal terminal text selection. Most terminals allow selection while
Shift is held. Run `:set mouse off` if yours does not.

## Commands

| Command | Action |
|---|---|
| `:open <url-or-search>` or `:o` | Open in the current tab |
| `:tabopen <url-or-search>` or `:t` | Open in a new tab |
| `:tabclose` | Close the current tab |
| `:tabnext`, `:tabprev` | Cycle through tabs |
| `:back` or `:b`, `:forward` or `:f` | Move through browser history |
| `:reload` | Reload the current tab |
| `:login` | Open Google Accounts in a visible Chromium window |
| `:set mouse on\|off` | Enable or disable mouse capture |
| `:set pixel on\|off` | Enable or disable pixel mode |
| `:quit` or `:q` | Quit |

A value with a scheme, a dot, or a host and port is treated as a URL. Other
input is sent to the configured search engine. For example, `:open banana`
searches DuckDuckGo, while `:open localhost:3000` opens a URL.

## Sessions and login

WWT stores its Chromium profile and open tabs in the XDG data directory:

- `${XDG_DATA_HOME:-$HOME/.local/share}/wwt/profile`
- `${XDG_DATA_HOME:-$HOME/.local/share}/wwt/session.json`

Only one WWT process can use the persistent Chromium profile at a time. A
second process starts a private session without saved logins and does not write
the session file.

Run `:login` to open Google Accounts. WWT saves the current tabs, stops
headless Chromium, and opens a visible Chromium window with the same profile.
Complete the login and close that window. WWT then restarts headless Chromium
and restores the tabs. Private sessions cannot use `:login`.

WWT initially loads only the focused tab. It loads other restored tabs when
you select them. If the number of live pages exceeds `max_tabs`, WWT unloads
the least recently viewed page but keeps its tab, title, URL, and last frame.

## Configuration

WWT reads `${XDG_CONFIG_HOME:-$HOME/.config}/wwt/config.toml`. The file is
optional. If a setting is invalid, WWT reports it in the status line and uses
the default value.

```toml
max_tabs = 8
search = "https://duckduckgo.com/?q={}"
chromium = "/usr/bin/chromium"
terminal = ["kitty", "-e"]
```

`max_tabs` limits live Chromium pages, not the number of tabs in the tab bar.
`search` must contain `{}` for the encoded query. `WWT_CHROMIUM` overrides the
`chromium` setting for the current run. `terminal` sets the command used for
desktop launches and does not affect WWT started inside an existing terminal.

## Project layout

| Crate | Responsibility |
|---|---|
| `wwt-frame` | Coordinate model and cell grid, with no I/O |
| `wwt-reader` | Semantic documents and terminal-width reflow |
| `wwt-png` | Dependency-free Base64 and PNG decoding |
| `wwt-cdp` | Chromium launcher and CDP client |
| `wwt-page` | Extraction and interaction for a live page |
| `wwt-term` | Terminal probing and rendering |
| `wwt-ui` | Modes, commands, chrome, and hint labels |
| `wwt` | Application state machine and binary |

## Development

Run the workspace checks before submitting a change:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The [architecture design](docs/superpowers/specs/2026-08-19-wwt-design.md)
describes the coordinate model, data flow, and milestone scope. Later design
notes cover the
[M8 reader mode](docs/superpowers/specs/2026-08-24-wwt-m8-design.md),
[lifecycle modules](docs/superpowers/specs/2026-08-28-wwt-lifecycle-modules-design.md),
and [pixel lifecycle](docs/superpowers/specs/2026-08-29-wwt-pixel-lifecycle-design.md).
The corresponding implementation plans are in
[`docs/superpowers/plans`](docs/superpowers/plans/).

WWT is licensed under the terms in [LICENSE](LICENSE).
