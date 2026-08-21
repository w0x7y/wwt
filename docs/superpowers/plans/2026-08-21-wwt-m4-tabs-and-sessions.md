# wwt M4 — Tabs and Sessions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn M3's single-page browser into one you keep open: many pages at once under one Chromium, and the same pages, still logged in, tomorrow.

**Architecture:** The seam is unchanged. `Session` still owns all state, still reaches nothing, and still answers `on(Event) -> Vec<Effect>` and `compose() -> Frame`; `Core` is still the adapter that decides nothing. What M4 changes is that both sides now say *which page* they mean: a `Tab` record per target, a `TabId` on every effect and job, and a `HashMap<TabId, Arc<Page>>` in `Core` where one `Arc<Page>` used to be. Persistence is two files on disk, a profile directory Chromium holds and a JSON snapshot we write.

**Tech Stack:** Rust 2024, tokio, tokio-tungstenite, crossterm (with `event-stream`), futures-util, serde/serde_json, anyhow. Chromium as an external process.

**Spec:** `docs/superpowers/specs/2026-08-21-wwt-m4-design.md` — read it in full before starting. Its parent, `docs/superpowers/specs/2026-08-19-wwt-design.md`, governs where the two disagree; sections 3, 5, 7 and 8 of the parent are the relevant ones, and section 10 of this plan's spec lists the four places M4 amends them.

## Global Constraints

- Rust edition **2024**, toolchain **1.97+**.
- Dependency versions, exact, unchanged from M3: `tokio = "1.53"`, `tokio-tungstenite = "0.30"`, `futures-util = "0.3"`, `serde = "1.0"` (feature `derive`), `serde_json = "1.0"`, `crossterm = "0.29"` (feature `event-stream`), `rustix = "1.1"` (feature `termios`), `anyhow = "1.0"`, `thiserror = "2.0"`, `tempfile = "3"` (dev-dependency).
- **M4 adds no dependencies at all.** Task 2 adds `serde` and `serde_json` to `crates/wwt/Cargo.toml`, both already fixed in the workspace `Cargo.toml` and already used by `wwt-cdp` and `wwt-page`. The XDG data directory is resolved by hand; do not add `dirs`. If a task tempts you to add a crate, stop and ask.
- `wwt-frame` has **no I/O and no dependencies**. Unchanged, non-negotiable.
- `wwt-ui` depends on `wwt-frame` only. It must never learn about pages, CDP, or the terminal.
- Chromium is located via `WWT_CHROMIUM`, falling back to the first of `chromium`, `chromium-browser`, `google-chrome-stable` on `PATH`. Never download anything.
- `cargo clippy --workspace --all-targets -- -D warnings` must be clean at the end of **every** task, not only at the end of the plan.
- Tests that need a browser live in `tests/`, never in `src/`. Unit tests in `src/` must run without Chromium.
- Follow the existing comment style: explain *why*, in prose, where the reason is not obvious from the code. Do not add comments that restate the code.
- **No em-dashes** in prose, comments or commit messages.
- Commits are conventional with a crate scope: `feat(wwt):`, `feat(page):`, `refactor(wwt):`, `perf(page):`.
- Test names are sentences describing the property, as in `cell_css_cell_roundtrip_is_identity`.

## Baseline

Before Task 1, confirm the starting state:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 233 tests pass, clippy clean. The integration tests launch Chromium; `cargo test -p wwt-frame -p wwt-ui` is the browser-free subset (38 + 38) and is the fast loop for tasks 3, 4 and 5.

If tests fail with paths naming a directory that no longer exists, the target directory holds stale artifacts: `touch crates/*/tests/*.rs` and run again.

## File structure

| File | Responsibility |
|---|---|
| `crates/wwt-cdp/src/launch.rs` | `Chromium::launch` takes an optional profile directory. The caller decides where it is. |
| `crates/wwt-cdp/src/target.rs` | **New.** `TargetId`: a CDP target's identity, travelling through the vocabulary the way `Input` already does. |
| `crates/wwt-cdp/tests/browser.rs` | Gains the proof that a second launch on a held profile fails. |
| `crates/wwt-frame/src/geom.rs` | `Viewport` gains an origin row. Every CSS-to-cell conversion now yields a **frame** row. |
| `crates/wwt-frame/src/caret.rs` | `Caret::cell` bounds-checks against the origin, not against zero. |
| `crates/wwt-frame/src/target.rs` | `HintTarget::label_cell` clamps into the page's rows, not the frame's. |
| `crates/wwt-ui/src/chrome.rs` | `tab_bar`: the top row. `TabLabel`, the one shape it takes. |
| `crates/wwt-ui/src/command.rs` | `Command::TabOpen`, `TabClose`, `TabNext`, `TabPrev`. |
| `crates/wwt-page/src/extract.rs` | `Page::adopt`, `scroll_to`, `activate`, `close`. |
| `crates/wwt/src/tab.rs` | **New.** `TabId` and `Tab`: everything that belongs to one page rather than to the browser. |
| `crates/wwt/src/store.rs` | **New.** XDG paths, `Snapshot`, reading and atomic writing. |
| `crates/wwt/src/session.rs` | Shrinks to what is global: grid, mode, the `:` line, `Vec<Tab>`, focus. |
| `crates/wwt/src/effect.rs` | Effects name a tab. `OpenTab`, `AdoptTab`, `CloseTab`, `Activate`, `Save`. |
| `crates/wwt/src/event.rs` | Jobs name a tab. `Opened`, `TargetOpened`, `Noted`. |
| `crates/wwt/src/core.rs` | A map of pages, target auto-attach, the save timer. |
| `crates/wwt/src/keymap.rs` | `Action::TabNext`, `TabPrev`, `TabClose`; `J`, `K`, `t`, `x`. |
| `crates/wwt/src/main.rs` | Argument parsing, the profile fallback, restore. |

---

### Task 1: The profile is the lock

The persistent profile is what makes logins durable, and the reason a second `wwt` cannot have one. This task does the smaller half, which is making `launch` take a directory, and proves the assumption the whole fallback rests on: that Chromium refuses a `--user-data-dir` another Chromium holds. **If that proof fails, stop and report it.** Section 7 of the spec says so explicitly, and the design's alternative is a lock file of our own.

**Files:**
- Modify: `crates/wwt-cdp/src/launch.rs`
- Modify: `crates/wwt-cdp/tests/browser.rs`
- Modify: `crates/wwt/src/lib.rs:22` (the one other caller)

**Interfaces:**
- Produces: `Chromium::launch(profile: Option<&std::path::Path>) -> Result<Chromium>`. `None` means a temporary profile that is deleted on drop, which is exactly M1's behaviour.

- [ ] **Step 1: Write the failing test**

Add to `crates/wwt-cdp/tests/browser.rs`:

```rust
/// The whole fallback in spec section 7 rests on this: Chromium refuses a
/// profile directory another Chromium is holding, so a second `wwt` needs no
/// lock file of ours to go stale after a crash. If this test ever fails, the
/// design is wrong and section 7 has to be rewritten around an explicit lock.
#[tokio::test]
async fn a_second_browser_cannot_have_a_profile_the_first_one_holds() {
    let profile = tempfile::tempdir().expect("a profile directory");

    let first = Chromium::launch(Some(profile.path()))
        .await
        .expect("the first browser takes the profile");

    let second = Chromium::launch(Some(profile.path())).await;
    assert!(
        second.is_err(),
        "a held profile must be refused, or the private-session fallback never triggers"
    );

    // Released on drop, so the next instance can have it.
    drop(first);
}

#[tokio::test]
async fn a_browser_with_no_profile_directory_gets_a_temporary_one() {
    let browser = Chromium::launch(None).await.expect("launch on a temp profile");
    assert!(browser.ws_url().starts_with("ws://"));
}
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p wwt-cdp --test browser`
Expected: compilation fails, `this function takes 0 arguments but 1 argument was supplied`.

- [ ] **Step 3: Write the implementation**

In `crates/wwt-cdp/src/launch.rs`, replace the `_profile` field and the head of `launch`:

```rust
/// A running headless Chromium. Killed on drop.
pub struct Chromium {
    child: Child,
    ws_url: String,
    /// Held so a temporary profile outlives the browser. `None` when the
    /// profile is a directory the caller owns and expects to survive us.
    _profile: Option<tempfile::TempDir>,
}

impl Chromium {
    /// Launch a browser on `profile`, or on a temporary directory when it is
    /// `None`.
    ///
    /// Where a persistent profile lives is the binary's business, not this
    /// crate's: `wwt-cdp` launches browsers and has no opinion about the
    /// user's data directory.
    ///
    /// A profile another Chromium already holds is refused by Chromium
    /// itself, which exits without announcing an endpoint, so this returns an
    /// error rather than a second browser sharing a cookie jar. That is the
    /// whole of the locking in spec section 7.
    pub async fn launch(profile: Option<&std::path::Path>) -> Result<Self> {
        let binary = find_chromium()?;
        let temporary = match profile {
            Some(_) => None,
            None => Some(tempfile::tempdir().context("create a temporary profile directory")?),
        };
        let dir = match (profile, &temporary) {
            (Some(path), _) => path.to_path_buf(),
            (None, Some(temp)) => temp.path().to_path_buf(),
            (None, None) => unreachable!("a temporary profile is created when none is given"),
        };

        let mut child = Command::new(&binary)
            .arg("--headless=new")
            // Port 0 lets the OS pick; we read the real one back off stderr.
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", dir.display()))
```

The rest of the builder chain is unchanged. Then the constructor at the end of `launch`:

```rust
        Ok(Self {
            child,
            ws_url,
            _profile: temporary,
        })
    }
```

- [ ] **Step 4: Fix the other caller**

`crates/wwt/src/lib.rs:22` calls `Chromium::launch()`. It is M1's `render_url`, which owns no profile:

```rust
    let browser = Chromium::launch(None).await.context("launch chromium")?;
```

`crates/wwt/src/main.rs` also calls it; leave it as `Chromium::launch(None)` for now. Task 13 gives it the real profile.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p wwt-cdp`
Expected: PASS, 10 tests.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

If `a_second_browser_cannot_have_a_profile_the_first_one_holds` fails, **stop here and report it**. Do not work around it.

- [ ] **Step 6: Commit**

```bash
git add crates/wwt-cdp crates/wwt/src/lib.rs crates/wwt/src/main.rs
git commit -m "feat(cdp): let the caller say where the profile lives"
```

---

### Task 2: XDG paths and the session snapshot

Where our two files live, and what one of them holds. Pure logic and one `fs` call each way, so it is all testable without a browser.

Environment variables are process-global and `cargo test` runs tests in threads, so the resolution logic takes its inputs as parameters and only the thin wrapper reads the environment. That is what makes these tests safe to run in parallel.

**Files:**
- Create: `crates/wwt/src/store.rs`
- Modify: `crates/wwt/src/lib.rs`
- Modify: `crates/wwt/Cargo.toml`

**Interfaces:**
- Produces: `wwt::store::{Snapshot, SavedTab, data_dir, profile_path, session_path, load, save}`.
- `Snapshot { version: u32, focus: usize, tabs: Vec<SavedTab> }`, `SavedTab { url: String, title: String, scroll_y: f64 }`.
- `load(path: &Path) -> Result<Option<Snapshot>, String>`: `Ok(None)` for a missing file, `Err` for an unreadable or malformed one.
- `save(path: &Path, snapshot: &Snapshot) -> Result<(), String>`: creates the parent directory and writes temp-then-rename.

- [ ] **Step 1: Add the dependencies**

In `crates/wwt/Cargo.toml`, under `[dependencies]`, beside the existing ones:

```toml
serde.workspace = true
serde_json.workspace = true
```

Both are already in the workspace `[workspace.dependencies]` block. Nothing is added to it.

- [ ] **Step 2: Write the failing tests**

Create `crates/wwt/src/store.rs` containing only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Snapshot {
        Snapshot {
            version: VERSION,
            focus: 1,
            tabs: vec![
                SavedTab {
                    url: "https://example.com".to_string(),
                    title: "Example".to_string(),
                    scroll_y: 0.0,
                },
                SavedTab {
                    url: "https://news.ycombinator.com".to_string(),
                    title: "Hacker News".to_string(),
                    scroll_y: 240.0,
                },
            ],
        }
    }

    #[test]
    fn xdg_data_home_wins_when_it_is_set() {
        let dir = data_dir_from(Some("/xdg".as_ref()), Some("/home/someone".as_ref()));
        assert_eq!(dir, Some(PathBuf::from("/xdg/wwt")));
    }

    #[test]
    fn home_is_the_fallback_when_xdg_data_home_is_not_set() {
        let dir = data_dir_from(None, Some("/home/someone".as_ref()));
        assert_eq!(dir, Some(PathBuf::from("/home/someone/.local/share/wwt")));
    }

    #[test]
    fn an_empty_xdg_data_home_counts_as_unset() {
        // The XDG spec says an empty value is to be treated as unset, and a
        // relative path called "wwt" in the working directory is not what
        // anyone meant.
        let dir = data_dir_from(Some("".as_ref()), Some("/home/someone".as_ref()));
        assert_eq!(dir, Some(PathBuf::from("/home/someone/.local/share/wwt")));
    }

    #[test]
    fn with_neither_variable_there_is_nowhere_to_put_anything() {
        assert_eq!(data_dir_from(None, None), None);
    }

    #[test]
    fn a_snapshot_survives_a_round_trip_through_the_file() {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("nested").join("session.json");

        save(&path, &snapshot()).expect("write it");
        let read = load(&path).expect("read it").expect("a file that exists");

        assert_eq!(read, snapshot());
    }

    #[test]
    fn a_missing_session_file_is_a_first_run_and_not_a_failure() {
        let dir = tempfile::tempdir().expect("a directory");
        assert_eq!(load(&dir.path().join("session.json")), Ok(None));
    }

    #[test]
    fn a_malformed_session_file_is_reported_rather_than_ignored() {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("session.json");
        std::fs::write(&path, b"{ not json").expect("write it");

        assert!(load(&path).is_err(), "a corrupt file must be a notice, not silence");
    }

    #[test]
    fn writing_never_leaves_a_half_written_file_behind() {
        // The write goes to a temporary name in the same directory and is
        // renamed into place, so the previous snapshot is either wholly
        // replaced or wholly intact.
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("session.json");

        save(&path, &snapshot()).expect("first write");
        save(&path, &snapshot()).expect("second write");

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("list the directory")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name())
            .filter(|name| name != "session.json")
            .collect();
        assert!(leftovers.is_empty(), "temporary files were left behind: {leftovers:?}");
    }
}
```

- [ ] **Step 3: Run the tests and watch them fail**

Run: `cargo test -p wwt --lib store`
Expected: compilation fails, `cannot find type Snapshot in this scope`.

- [ ] **Step 4: Write the implementation**

Put this above the test module in `crates/wwt/src/store.rs`:

```rust
//! Where wwt keeps its two files, and what is in the smaller one.
//!
//! The profile directory is Chromium's and we only name it. The session file
//! is ours: the tabs that were open, so a restart comes back to them.
//!
//! Resolution takes its inputs as parameters rather than reading the
//! environment, because environment variables are process-global and tests
//! run in threads. Only `data_dir` reads them.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The snapshot format. A file claiming anything else is not ours to read.
pub const VERSION: u32 = 1;

/// One tab, as much of it as survives a restart.
///
/// Not the runs, not the caret, not the hint targets: those are what the page
/// looked like, and the page will be laid out again anyway.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedTab {
    pub url: String,
    pub title: String,
    #[serde(rename = "scrollY")]
    pub scroll_y: f64,
}

/// The open tabs, on their way to or from disk.
///
/// Called a snapshot rather than a session because `Session` already names
/// the state machine and `wwt-cdp` already calls an attached target a session
/// id. A third meaning would make the glossary useless.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub focus: usize,
    pub tabs: Vec<SavedTab>,
}

/// Our directory under the user's data home, or `None` when there is no
/// home to put it in.
pub fn data_dir() -> Option<PathBuf> {
    data_dir_from(
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// The arithmetic of `data_dir`, with the environment passed in.
fn data_dir_from(xdg: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    // An empty variable is unset, per the XDG basedir spec. Honouring it
    // literally would put the profile in a relative directory called `wwt`
    // wherever the terminal happened to be.
    if let Some(xdg) = xdg.filter(|value| !value.is_empty()) {
        return Some(Path::new(xdg).join("wwt"));
    }
    let home = home.filter(|value| !value.is_empty())?;
    Some(Path::new(home).join(".local/share/wwt"))
}

/// Chromium's persistent profile: the cookie jar that makes logins durable.
pub fn profile_path() -> Option<PathBuf> {
    Some(data_dir()?.join("profile"))
}

/// The tabs that were open last time.
pub fn session_path() -> Option<PathBuf> {
    Some(data_dir()?.join("session.json"))
}

/// Read a snapshot. `Ok(None)` is a first run; `Err` is a file that exists
/// and cannot be used, which is a notice rather than an exit.
pub fn load(path: &Path) -> Result<Option<Snapshot>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    serde_json::from_str(&text).map(Some).map_err(|error| format!("{}: {error}", path.display()))
}

/// Write a snapshot, atomically.
///
/// Temp file in the same directory, then rename: a rename within one
/// filesystem is atomic, so a crash mid-write leaves the previous snapshot
/// wholly intact rather than a truncated one that reads as corrupt.
pub fn save(path: &Path, snapshot: &Snapshot) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| format!("{} has no directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;

    let text = serde_json::to_string_pretty(snapshot).map_err(|error| error.to_string())?;
    let temp = path.with_extension("json.new");
    std::fs::write(&temp, text).map_err(|error| format!("{}: {error}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|error| format!("{}: {error}", path.display()))
}
```

- [ ] **Step 5: Declare the module**

In `crates/wwt/src/lib.rs`, add `pub mod store;` to the module list, keeping it alphabetical:

```rust
pub mod core;
pub mod effect;
pub mod event;
pub mod input;
pub mod keys;
pub mod keymap;
pub mod session;
pub mod store;
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p wwt --lib store`
Expected: PASS, 8 tests.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/wwt/Cargo.toml crates/wwt/src/store.rs crates/wwt/src/lib.rs
git commit -m "feat(wwt): give the session file a shape and a place to live"
```

---

### Task 3: `Viewport` gains an origin row

The load-bearing change. Until now the chrome was the last row, so page row 0 and frame row 0 were the same row and nothing translated. A tab bar above the page ends that. `CLAUDE.md` makes `Viewport` the only thing allowed to convert between CSS pixels and cells, so the shift goes there rather than as a `+1` sprinkled through the three places that convert.

After this task, `to_cell`, `col_of` and `row_of` return **frame** rows, and `to_css` takes one. `css_width` and `css_height` are untouched: how big the page is has nothing to do with where it sits on our screen.

**Files:**
- Modify: `crates/wwt-frame/src/geom.rs`
- Modify: `crates/wwt-frame/src/caret.rs:29-33`
- Modify: `crates/wwt-frame/src/target.rs:40-47`

**Interfaces:**
- Consumes: nothing new.
- Produces: `Viewport::with_origin(grid: GridSize, cell: CellSize, origin_row: u16) -> Viewport` and `Viewport::origin_row(&self) -> u16`. `Viewport::new(grid, cell)` is unchanged and means origin 0.

- [ ] **Step 1: Write the failing tests**

In `crates/wwt-frame/src/geom.rs`, add to the existing `mod tests`:

```rust
    fn offset_vp(cols: u16, rows: u16, w: u16, h: u16, origin: u16) -> Viewport {
        Viewport::with_origin(GridSize { cols, rows }, CellSize { w, h }, origin)
    }

    #[test]
    fn an_origin_row_moves_the_page_down_the_screen_without_resizing_it() {
        let v = offset_vp(80, 22, 9, 20, 1);
        // The page's own size is what Chromium is told, and it is unaffected
        // by where the page sits on our screen.
        assert_eq!(v.css_height(), 440);
        assert_eq!(v.origin_row(), 1);
    }

    #[test]
    fn the_top_of_the_page_lands_one_row_below_the_top_of_the_frame() {
        let v = offset_vp(80, 22, 9, 20, 1);
        assert_eq!(
            v.to_cell(CssPoint { x: 0.0, y: 0.0 }),
            Some(CellPos { col: 0, row: 1 })
        );
        assert_eq!(v.row_of(0.0), 1);
    }

    #[test]
    fn a_frame_row_above_the_origin_is_not_part_of_the_page() {
        let v = offset_vp(80, 22, 9, 20, 1);
        // Row 0 is the tab bar. Asking for its CSS position gives a point
        // above the page, and no CSS point maps back to it.
        assert!(v.to_css(CellPos { col: 0, row: 0 }).y < 0.0);
        assert_eq!(v.to_cell(CssPoint { x: 0.0, y: -1.0 }), None);
    }

    #[test]
    fn a_point_below_the_last_page_row_is_off_the_page() {
        let v = offset_vp(80, 22, 9, 20, 1);
        // 22 rows of 20px is 440; the last page row is frame row 22.
        assert_eq!(
            v.to_cell(CssPoint { x: 0.0, y: 439.0 }),
            Some(CellPos { col: 0, row: 22 })
        );
        assert_eq!(v.to_cell(CssPoint { x: 0.0, y: 440.0 }), None);
    }

    /// The property from spec section 3, now over origins as well. This is
    /// the one that must not be allowed to fail.
    #[test]
    fn cell_css_cell_roundtrip_is_identity_at_every_origin() {
        for origin in [0u16, 1, 2, 7] {
            for (w, h) in [(8u16, 16u16), (9, 20), (12, 26), (1, 1)] {
                let v = offset_vp(180, 46, w, h, origin);
                for page_row in 0..v.grid().rows {
                    for col in 0..v.grid().cols {
                        let c = CellPos { col, row: page_row + origin };
                        assert_eq!(
                            v.to_cell(v.to_css(c)),
                            Some(c),
                            "roundtrip failed at {c:?}, cell {w}x{h}, origin {origin}"
                        );
                    }
                }
            }
        }
    }
```

In `crates/wwt-frame/src/caret.rs`, add to `mod tests`:

```rust
    #[test]
    fn the_caret_lands_below_a_chrome_row_that_is_above_the_page() {
        let vp = Viewport::with_origin(
            GridSize { cols: 80, rows: 22 },
            CellSize { w: 9, h: 20 },
            1,
        );
        let caret = Caret { x: 90.0, baseline: 16.0, offset: 0 };
        assert_eq!(caret.cell(&vp), Some(CellPos { col: 10, row: 1 }));
    }

    #[test]
    fn a_caret_below_the_last_page_row_still_has_no_cell() {
        let vp = Viewport::with_origin(
            GridSize { cols: 80, rows: 22 },
            CellSize { w: 9, h: 20 },
            1,
        );
        let caret = Caret { x: 90.0, baseline: 441.0, offset: 0 };
        assert_eq!(caret.cell(&vp), None, "row 23 of a page ending at row 22");
    }
```

In `crates/wwt-frame/src/target.rs`, add to `mod tests`:

```rust
    #[test]
    fn a_label_clamps_into_the_page_rather_than_onto_the_chrome() {
        let vp = Viewport::with_origin(
            GridSize { cols: 80, rows: 22 },
            CellSize { w: 9, h: 20 },
            1,
        );
        // A box above the viewport keeps a reachable label, but it must not
        // land on the tab bar, which the page does not own.
        let t = HintTarget {
            rect: CssRect { x: -500.0, y: -500.0, w: 40.0, h: 20.0 },
            kind: TargetKind::Clickable,
        };
        assert_eq!(t.label_cell(&vp), CellPos { col: 0, row: 1 });

        let t = HintTarget {
            rect: CssRect { x: 100_000.0, y: 100_000.0, w: 40.0, h: 20.0 },
            kind: TargetKind::Clickable,
        };
        assert_eq!(t.label_cell(&vp), CellPos { col: 79, row: 22 });
    }
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p wwt-frame`
Expected: compilation fails, `no function or associated item named with_origin found`.

- [ ] **Step 3: Write the implementation**

In `crates/wwt-frame/src/geom.rs`, replace the `Viewport` struct and its impl block down to `row_of`:

```rust
/// Binds the terminal grid to the CSS viewport we ask Chromium to lay out.
///
/// `grid` is the *page's* size in cells, which is the terminal less the rows
/// the chrome occupies. `origin_row` is the frame row the page's first row
/// lands on, so a conversion out of CSS gives a row you can paint at and a
/// conversion into CSS takes one. The page is never told either: how big it
/// is has nothing to do with where it sits on our screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    grid: GridSize,
    cell: CellSize,
    origin_row: u16,
}

impl Viewport {
    pub fn new(grid: GridSize, cell: CellSize) -> Self {
        Self::with_origin(grid, cell, 0)
    }

    pub fn with_origin(grid: GridSize, cell: CellSize, origin_row: u16) -> Self {
        assert!(cell.w > 0 && cell.h > 0, "cell size must be non-zero");
        Self { grid, cell, origin_row }
    }

    pub fn grid(&self) -> GridSize {
        self.grid
    }

    pub fn cell(&self) -> CellSize {
        self.cell
    }

    /// The frame row the page's first row is painted on.
    pub fn origin_row(&self) -> u16 {
        self.origin_row
    }

    /// The viewport width in CSS pixels: what Chromium is told the window is.
    pub fn css_width(&self) -> u32 {
        u32::from(self.grid.cols) * u32::from(self.cell.w)
    }

    /// The viewport height in CSS pixels.
    pub fn css_height(&self) -> u32 {
        u32::from(self.grid.rows) * u32::from(self.cell.h)
    }

    /// The CSS point at the *center* of a frame cell. Center rather than
    /// corner so that dispatching a click at this point lands unambiguously
    /// inside the cell, and so the roundtrip below is exact.
    ///
    /// A row above the origin is chrome, and the point this returns for one
    /// is above the page, which is what makes `to_cell` refuse it.
    pub fn to_css(&self, c: CellPos) -> CssPoint {
        let page_row = f64::from(c.row) - f64::from(self.origin_row);
        CssPoint {
            x: (f64::from(c.col) + 0.5) * f64::from(self.cell.w),
            y: (page_row + 0.5) * f64::from(self.cell.h),
        }
    }

    /// The frame cell containing a CSS point, or `None` if it falls outside
    /// the page.
    pub fn to_cell(&self, p: CssPoint) -> Option<CellPos> {
        if p.x < 0.0 || p.y < 0.0 {
            return None;
        }
        let col = (p.x / f64::from(self.cell.w)) as u32;
        let row = (p.y / f64::from(self.cell.h)) as u32;
        if col >= u32::from(self.grid.cols) || row >= u32::from(self.grid.rows) {
            return None;
        }
        Some(CellPos {
            col: col as u16,
            row: row as u16 + self.origin_row,
        })
    }

    /// The column a CSS x-coordinate falls in, unclamped by the grid's right
    /// edge. Painting uses this so a run starting off-screen still places its
    /// visible tail correctly.
    pub fn col_of(&self, x: f64) -> i64 {
        (x / f64::from(self.cell.w)).floor() as i64
    }

    /// The frame row a CSS y-coordinate falls in, unclamped.
    pub fn row_of(&self, y: f64) -> i64 {
        (y / f64::from(self.cell.h)).floor() as i64 + i64::from(self.origin_row)
    }
}
```

- [ ] **Step 4: Fix `Caret::cell`**

In `crates/wwt-frame/src/caret.rs`, the bounds check compared a row against zero and the grid height. Both ends move with the origin:

```rust
    pub fn cell(&self, vp: &Viewport) -> Option<CellPos> {
        let row = vp.row_of(self.baseline);
        let col = vp.col_of(self.x) + i64::try_from(self.offset).ok()?;
        let grid = vp.grid();
        let top = i64::from(vp.origin_row());
        let on_grid =
            row >= top && row < top + i64::from(grid.rows) && col >= 0 && col < i64::from(grid.cols);
        on_grid.then_some(CellPos { col: col as u16, row: row as u16 })
    }
```

- [ ] **Step 5: Fix `HintTarget::label_cell`**

In `crates/wwt-frame/src/target.rs`, a label clamped to row 0 would land on the tab bar, which belongs to us and not to the page:

```rust
    pub fn label_cell(&self, vp: &Viewport) -> CellPos {
        let grid = vp.grid();
        let top = i64::from(vp.origin_row());
        let last_col = i64::from(grid.cols.saturating_sub(1));
        let last_row = top + i64::from(grid.rows.saturating_sub(1));
        CellPos {
            col: vp.col_of(self.rect.x).clamp(0, last_col) as u16,
            row: vp.row_of(self.rect.y).clamp(top, last_row) as u16,
        }
    }
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p wwt-frame`
Expected: PASS, 46 tests. Every pre-existing test used `Viewport::new`, which is origin 0, so none of them change.

Run: `cargo test --workspace`
Expected: PASS, 241 tests. Nothing else has an origin yet.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/wwt-frame
git commit -m "feat(frame): let the page start below the top of the screen

The chrome is about to be two rows with one of them above the page, which
ends the coincidence that page row 0 and frame row 0 are the same row. The
shift belongs in Viewport, which is the only thing allowed to convert
between CSS pixels and cells; a +1 in paint_run, Caret::cell and page_cell
would be three chances to drift apart.

The roundtrip property now holds at every origin as well as every cell size."
```

---

### Task 4: The chrome becomes two rows

The tab bar takes the top row and the page moves down one. With a single tab there is not much to see yet, but the geometry is the part that has to be right before anything depends on it, and every existing test that knows where the page starts is fixed here rather than later.

The focused tab is marked by *not* being reversed while the rest of the bar is. The frame carries a foreground colour and no background, so an inverted run against an inverted bar is the only highlight available. Section 13 of the spec records that this is thin at a dozen tabs.

**Files:**
- Modify: `crates/wwt-ui/src/chrome.rs`
- Modify: `crates/wwt/src/session.rs` (`page_viewport`, `page_cell`, `compose`, and five tests)

**Interfaces:**
- Consumes: `Viewport::with_origin`, `Viewport::origin_row` from Task 3.
- Produces: `wwt_ui::chrome::{TabSlot, tab_slots, paint_tabs}`.
  - `TabSlot { col: u16, text: String, focused: bool }`
  - `tab_slots(titles: &[String], focus: usize, cols: u16) -> Vec<TabSlot>`
  - `paint_tabs(frame: &mut Frame, titles: &[String], focus: usize)`
- Produces: `wwt::session::CHROME_ROWS: u16 = 2`.

- [ ] **Step 1: Write the failing tests**

In `crates/wwt-ui/src/chrome.rs`, add to `mod tests`:

```rust
    fn titles(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("Tab {i}")).collect()
    }

    #[test]
    fn every_tab_gets_a_slot_when_they_all_fit() {
        let slots = tab_slots(&titles(3), 0, 80);
        assert_eq!(slots.len(), 3);
        assert_eq!(slots[0].col, 0);
        assert!(slots[0].text.contains('1'), "tabs are numbered from one: {:?}", slots[0].text);
        assert!(slots[2].text.contains("Tab 2"));
    }

    #[test]
    fn exactly_one_slot_is_the_focused_one() {
        let slots = tab_slots(&titles(4), 2, 80);
        let focused: Vec<usize> = slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.focused)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(focused, vec![2]);
    }

    #[test]
    fn the_slots_never_run_past_the_right_edge() {
        for cols in [4u16, 10, 40, 80, 200] {
            for count in [1usize, 2, 5, 30] {
                for slot in tab_slots(&titles(count), 0, cols) {
                    let end = usize::from(slot.col) + slot.text.chars().count();
                    assert!(end <= usize::from(cols), "{count} tabs in {cols} columns overran");
                }
            }
        }
    }

    #[test]
    fn a_long_title_is_elided_rather_than_stealing_the_next_tabs_room() {
        let long = vec!["a".repeat(200), "second".to_string()];
        let slots = tab_slots(&long, 0, 40);
        assert_eq!(slots.len(), 2);
        assert!(slots[0].text.contains('…'), "slot 0 was {:?}", slots[0].text);
        assert!(slots[1].col >= 20, "the second tab kept its half: {:?}", slots[1].col);
    }

    #[test]
    fn more_tabs_than_fit_show_a_window_around_the_focused_one() {
        // Twenty tabs cannot fit in forty columns, so the bar shows the
        // neighbourhood of wherever you are rather than the first few tabs
        // forever.
        let slots = tab_slots(&titles(20), 15, 40);
        assert!(slots.iter().any(|slot| slot.focused), "the focused tab must always be visible");
        assert!(slots.len() < 20, "it cannot possibly show them all");
    }

    #[test]
    fn one_tab_still_gets_a_bar() {
        let slots = tab_slots(&titles(1), 0, 80);
        assert_eq!(slots.len(), 1);
        assert!(slots[0].focused);
    }

    #[test]
    fn no_tabs_is_an_empty_bar_rather_than_a_panic() {
        assert_eq!(tab_slots(&[], 0, 80).len(), 0);
    }

    #[test]
    fn a_focus_index_past_the_end_does_not_panic() {
        // The index can come off a session file, which is data from disk.
        let slots = tab_slots(&titles(3), 99, 80);
        assert_eq!(slots.len(), 3);
    }

    #[test]
    fn paint_puts_the_tab_bar_on_the_first_row() {
        let mut frame = Frame::new(GridSize { cols: 40, rows: 4 });
        paint_tabs(&mut frame, &titles(2), 1);
        assert!(frame.row_text(0).contains("Tab 0"), "row 0 was {:?}", frame.row_text(0));
        assert!(frame.row_text(0).contains("Tab 1"), "row 0 was {:?}", frame.row_text(0));
        assert_eq!(frame.row_text(1), "", "the page's rows are not the bar's");
    }

    #[test]
    fn painting_the_tab_bar_never_moves_the_cursor() {
        let mut frame = Frame::new(GridSize { cols: 40, rows: 4 });
        paint_tabs(&mut frame, &titles(2), 0);
        assert_eq!(frame.cursor(), None);
    }
```

In `crates/wwt/src/session.rs`, replace the two viewport tests and add one, in `mod tests`:

```rust
    #[test]
    fn the_chrome_owns_a_row_at_each_end_and_the_page_knows_of_neither() {
        let session = ready();
        assert_eq!(session.viewport().grid().rows, 22);
        assert_eq!(session.viewport().origin_row(), 1);
        assert_eq!(session.compose().grid().rows, 24);
    }

    #[test]
    fn the_page_viewport_is_two_rows_shorter_than_the_terminal() {
        let vp = page_viewport(GRID, CELL);
        assert_eq!(vp.grid(), GridSize { cols: 80, rows: 22 });
        assert_eq!(vp.css_height(), 22 * 20);
    }

    #[test]
    fn a_click_on_the_tab_bar_belongs_to_no_page_cell() {
        // Row 0 is the tab bar and row 23 is the statusline. The page does
        // not know either exists, so there is nothing to convert a click
        // there into.
        let vp = page_viewport(GRID, CELL);
        assert_eq!(page_cell(&vp, 5, 0), None);
        assert_eq!(page_cell(&vp, 5, 23), None);
    }
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p wwt-ui`
Expected: compilation fails, `cannot find function tab_slots in this scope`.

- [ ] **Step 3: Write the tab bar**

In `crates/wwt-ui/src/chrome.rs`, change the module doc and add the bar above `paint`:

```rust
//! The rows the page does not own: a tab bar on top, and a statusline, or
//! the `:` command line, underneath.
```

```rust
/// The narrowest slot worth painting: a number, a space, and a character or
/// two of title. Below this a tab says nothing, so fewer tabs are shown
/// instead of more unreadable ones.
const MIN_SLOT: usize = 8;

/// One tab's place on the bar.
pub struct TabSlot {
    pub col: u16,
    pub text: String,
    pub focused: bool,
}

/// The focused tab, against a bar that is reversed everywhere else.
///
/// The frame carries a foreground colour and no background, so an unreversed
/// run inside a reversed row is the only highlight there is.
fn focus_style() -> Style {
    Style {
        fg: Rgb { r: 0xd0, g: 0xd0, b: 0xd0 },
        bold: true,
        reverse: false,
    }
}

/// Where each visible tab goes, and which one is yours.
///
/// Titles come from pages, and the focus index can come off a session file,
/// so neither is trusted: an absurd index still produces a paintable bar.
pub fn tab_slots(titles: &[String], focus: usize, cols: u16) -> Vec<TabSlot> {
    if titles.is_empty() || cols == 0 {
        return Vec::new();
    }
    let cols = usize::from(cols);
    let focus = focus.min(titles.len() - 1);

    // How many fit at a readable width, and how wide they are once that many
    // share the row.
    let visible = (cols / MIN_SLOT).clamp(1, titles.len());
    let width = cols / visible;

    // Centre the window on the focused tab, then push it back inside the
    // list. Without the second step, focusing the last tab would scroll the
    // window off the end and show nothing.
    let start = focus.saturating_sub(visible / 2).min(titles.len() - visible);

    titles
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .enumerate()
        .map(|(offset, (index, title))| TabSlot {
            col: u16::try_from(offset * width).unwrap_or(u16::MAX),
            text: segment(index, title, width),
            focused: index == focus,
        })
        .collect()
}

/// One tab's text, exactly `width` characters: its number, then as much of
/// its title as is left.
fn segment(index: usize, title: &str, width: usize) -> String {
    let head = format!(" {} ", index + 1);
    let room = width.saturating_sub(head.chars().count() + 1);
    let shown: String = if title.chars().count() > room {
        title.chars().take(room.saturating_sub(1)).chain(std::iter::once('…')).collect()
    } else {
        title.to_string()
    };
    fit(&format!("{head}{shown}"), width)
}

/// Paint the top row of the frame.
pub fn paint_tabs(frame: &mut Frame, titles: &[String], focus: usize) {
    let GridSize { cols, rows } = frame.grid();
    if rows == 0 || cols == 0 {
        return;
    }
    // The whole row first, so the unused end of the bar is part of the bar
    // rather than a hole onto the page behind it.
    let blank = " ".repeat(usize::from(cols));
    frame.paint_text(CellPos { col: 0, row: 0 }, &blank, chrome_style());

    for slot in tab_slots(titles, focus, cols) {
        let style = if slot.focused { focus_style() } else { chrome_style() };
        frame.paint_text(CellPos { col: slot.col, row: 0 }, &slot.text, style);
    }
}
```

- [ ] **Step 4: Move the page down a row**

In `crates/wwt/src/session.rs`, replace `page_viewport` and `page_cell`:

```rust
/// The rows the page does not get: the tab bar above it and the statusline
/// below. Unconditional, so opening a tab never reflows a page.
pub const CHROME_ROWS: u16 = 2;

/// The page viewport: the terminal grid, less the rows chrome occupies, and
/// sitting below the tab bar.
///
/// Chromium is told this is the whole window, so the page genuinely does not
/// know either chrome row exists.
pub fn page_viewport(grid: GridSize, cell: CellSize) -> Viewport {
    let rows = grid.rows.saturating_sub(CHROME_ROWS).max(1);
    Viewport::with_origin(GridSize { cols: grid.cols, rows }, cell, 1)
}

/// The page cell a terminal cell refers to, or `None` when it is one of ours.
///
/// The first row is the tab bar and the last is the statusline. The page does
/// not know either exists, so a click on one has no page coordinate to become.
pub fn page_cell(vp: &Viewport, column: u16, row: u16) -> Option<CellPos> {
    let grid = vp.grid();
    let top = vp.origin_row();
    let below = top.checked_add(grid.rows)?;
    (column < grid.cols && row >= top && row < below).then_some(CellPos { col: column, row })
}
```

- [ ] **Step 5: Paint the bar**

In `Session::compose`, add the bar before the statusline. With one tab it shows one tab; Task 5 gives it the rest.

```rust
        chrome::paint_tabs(&mut frame, std::slice::from_ref(&self.title), 0);
        chrome::paint(
            &mut frame,
            &self.mode,
            &self.state,
            &self.url,
            &self.title,
            self.progress,
        );
```

- [ ] **Step 6: Fix the tests that knew where the page started**

Three existing tests in `crates/wwt/src/session.rs` assert positions that have moved by one row:

`the_wheel_scrolls_three_rows_a_notch` pointed at row 0, which is now the tab bar:

```rust
    #[test]
    fn the_wheel_scrolls_three_rows_a_notch() {
        let mut session = ready();
        let effects = session.on(mouse(MouseEventKind::ScrollDown, 0, 1));
        let at = session.viewport().to_css(CellPos { col: 0, row: 1 });
        assert_eq!(effects, vec![Effect::Send(Input::Mouse(MouseInput::wheel(at, 60.0)))]);
    }
```

`the_caret_shows_in_insert_mode_only` ends with a caret one row further down:

```rust
        session.on(key('i'));
        assert_eq!(session.compose().cursor(), Some(CellPos { col: 12, row: 3 }));
```

`a_click_on_the_statusline_is_not_the_pages_to_see` keeps its assertion; only its comment is now half the story, so fold it into the new `a_click_on_the_tab_bar_belongs_to_no_page_cell` and delete the old `a_click_on_the_chrome_row_belongs_to_no_page_cell`.

Delete `the_statusline_owns_the_last_row_and_the_page_does_not_know_it_exists` and `the_page_viewport_is_one_row_shorter_than_the_terminal`; their replacements were written in Step 1.

- [ ] **Step 7: Run the tests**

Run: `cargo test -p wwt-ui`
Expected: PASS, 48 tests.

Run: `cargo test --workspace`
Expected: PASS. Every test that knew the old geometry has been moved.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/wwt-ui/src/chrome.rs crates/wwt/src/session.rs
git commit -m "feat(ui): give the browser a tab bar to put tabs in

The bar is unconditional rather than appearing at the second tab, so the
page viewport never changes size for a reason that is not a real resize and
opening a tab never reflows anything.

The focused tab is the one part of the bar that is not reversed. A frame
carries a foreground colour and no background, so that is the only highlight
available; it is thin past a dozen tabs, which spec section 13 records."
```

---

### Task 5: The tab record

The mechanical half of the milestone: everything that belongs to one page moves out of `Session` and into a `Tab`, and `Session` holds a vector of exactly one. **No behaviour changes in this task.** Every existing test must still pass with its assertions untouched, which is what makes it safe to do all at once.

`session.rs` is 982 lines and is two things in one. This is the split.

**Files:**
- Create: `crates/wwt/src/tab.rs`
- Modify: `crates/wwt/src/session.rs`
- Modify: `crates/wwt/src/lib.rs`

**Interfaces:**
- Produces: `wwt::tab::{TabId, Tab}`.
  - `TabId(u32)`, deriving `Debug, Clone, Copy, PartialEq, Eq, Hash`.
  - `Tab::new(id: TabId, url: String) -> Tab`, `Tab::mark_dirty(&mut self)`.
  - Fields as listed in spec section 2.
- Produces on `Session`: `focused(&self) -> &Tab`, `focused_mut(&mut self) -> &mut Tab`, `focused_id(&self) -> TabId`.

- [ ] **Step 1: Write the failing test**

Create `crates/wwt/src/tab.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_tab_has_not_been_read_yet() {
        let tab = Tab::new(TabId(0), "https://example.com".to_string());
        assert!(tab.dirty, "a page nobody has looked at is dirty by definition");
        assert!(!tab.extracting);
        assert_eq!(tab.url, "https://example.com");
    }

    #[test]
    fn a_page_that_moved_has_invalidated_its_hint_targets() {
        // Targets are geometry. A page that changed has moved them, and a
        // click at a remembered rect would land on whatever is there now.
        let mut tab = Tab::new(TabId(0), String::new());
        tab.hints = Some(Vec::new());
        tab.mark_dirty();
        assert!(tab.dirty);
        assert_eq!(tab.hints, None);
    }

    #[test]
    fn tab_ids_are_compared_by_value_and_not_by_position() {
        assert_eq!(TabId(3), TabId(3));
        assert_ne!(TabId(3), TabId(4));
    }
}
```

- [ ] **Step 2: Run the test and watch it fail**

Run: `cargo test -p wwt --lib tab`
Expected: compilation fails, `cannot find type Tab in this scope`.

- [ ] **Step 3: Write the tab**

Put this above the test module in `crates/wwt/src/tab.rs`:

```rust
//! One page, and everything true of it rather than of the browser.
//!
//! What is left in `Session` after this is what is global: the grid, the
//! mode, the `:` line, and which of these is in front. Splitting it this way
//! is also what lets a background tab keep its runs, so a switch is a
//! repaint rather than a round trip.

use wwt_frame::{Caret, HintTarget, TextRun};
use wwt_ui::chrome::State;

/// A tab's identity, for as long as the tab exists.
///
/// A counter and never a position. A page operation outlives the state that
/// asked for it: close a tab while its extraction is in flight and every
/// later tab shifts down one, so an index would let the answer land on a page
/// that never asked. A value that is never reused makes a stale answer
/// identifiable, which is the difference between dropping it and painting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(pub u32);

/// One page: what it is showing, and what we have asked it for.
#[derive(Debug, Clone)]
pub struct Tab {
    pub id: TabId,
    pub url: String,
    pub title: String,
    pub state: State,

    pub runs: Vec<TextRun>,
    /// Where typing would land, when the page has a field focused.
    pub caret: Option<Caret>,
    /// How far down the document we are, for the statusline.
    pub progress: f64,
    /// Where the document is scrolled to, for the session file.
    pub scroll_y: f64,

    /// The page says it changed and we have not caught up yet. A background
    /// tab sets this and spends it when focus arrives.
    pub dirty: bool,
    /// An extraction is in flight; a second would race it.
    pub extracting: bool,
    /// A navigation is in flight.
    pub navigating: bool,
    /// The last hint query's targets, held so that pressing `f` twice on a
    /// page that has not moved costs one round trip rather than two.
    pub hints: Option<Vec<HintTarget>>,
    /// A hint query is in flight. Every other effect answers to itself, but
    /// this one comes back and changes the mode, so it needs to be known
    /// about while it is away.
    pub hinting: bool,
}

impl Tab {
    pub fn new(id: TabId, url: String) -> Self {
        Self {
            id,
            url,
            title: String::new(),
            state: State::Loading,
            runs: Vec::new(),
            caret: None,
            progress: 0.0,
            scroll_y: 0.0,
            dirty: true,
            extracting: false,
            navigating: false,
            hints: None,
            hinting: false,
        }
    }

    /// Note that the page has changed under us.
    ///
    /// Hint targets are geometry, so a page that moved has invalidated them.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.hints = None;
    }
}
```

- [ ] **Step 4: Declare the module**

In `crates/wwt/src/lib.rs`, add `pub mod tab;` after `pub mod store;`.

- [ ] **Step 5: Move the state**

In `crates/wwt/src/session.rs`, replace the struct and `new`:

```rust
pub struct Session {
    grid: GridSize,
    cell: CellSize,
    vp: Viewport,

    mode: Mode,

    tabs: Vec<Tab>,
    focus: usize,
    /// Never reused, which is what makes a job from a closed tab safe to
    /// drop rather than plausible to paint.
    next_id: u32,
}
```

```rust
impl Session {
    pub fn new(grid: GridSize, cell: CellSize) -> Self {
        let mut session = Self {
            grid,
            cell,
            vp: page_viewport(grid, cell),
            mode: Mode::Normal,
            tabs: Vec::new(),
            focus: 0,
            next_id: 0,
        };
        let id = session.mint();
        session.tabs.push(Tab::new(id, String::new()));
        session
    }

    /// The next unused tab id.
    fn mint(&mut self) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        id
    }

    /// The tab you are looking at. There is always one: closing the last tab
    /// quits, so a session with no tabs never reaches a caller.
    pub fn focused(&self) -> &Tab {
        &self.tabs[self.focus]
    }

    fn focused_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.focus]
    }

    pub fn focused_id(&self) -> TabId {
        self.focused().id
    }
```

Add the imports at the top of the file:

```rust
use crate::tab::{Tab, TabId};
```

- [ ] **Step 6: Route every reader and writer through the focused tab**

This is mechanical. Every `self.url`, `self.title`, `self.progress`, `self.runs`, `self.caret`, `self.state`, `self.dirty`, `self.extracting`, `self.navigating`, `self.hints` and `self.hinting` becomes `self.focused().<field>` or `self.focused_mut().<field>`. `self.mark_dirty()` becomes `self.focused_mut().mark_dirty()` and the private `Session::mark_dirty` is deleted, since `Tab::mark_dirty` replaced it.

Four places need more than a rename:

`state()` and `notice()`:

```rust
    pub fn state(&self) -> &State {
        &self.focused().state
    }

    /// Say something in the statusline.
    pub fn notice(&mut self, message: &str) {
        self.focused_mut().state = State::Notice(message.to_string());
    }
```

`compose` reads several fields off one tab, so bind it once. The borrow checker will not let you hold `&Tab` across `chrome::paint(&mut frame, ...)` while also passing `&self.mode`, so collect the titles first:

```rust
    pub fn compose(&self) -> Frame {
        let mut frame = Frame::new(self.grid);
        let tab = self.focused();
        frame.paint_runs(&self.vp, &tab.runs);

        // After the page and before the chrome: labels cover the text they
        // point at, which is what makes them readable, and the chrome still
        // owns its rows.
        if let Mode::Hint(session) = &self.mode {
            session.paint(&mut frame, &self.vp);
        }

        let titles: Vec<String> = self.tabs.iter().map(|tab| tab.title.clone()).collect();
        chrome::paint_tabs(&mut frame, &titles, self.focus);
        chrome::paint(
            &mut frame,
            &self.mode,
            &tab.state,
            &tab.url,
            &tab.title,
            tab.progress,
        );

        // One place decides where the cursor goes, though two modes have an
        // insertion point. Splitting that between here and the chrome would
        // leave the two exclusive only by accident of paint order.
        frame.set_cursor(match &self.mode {
            // A page can focus a field without your asking, and a caret
            // there would promise that your typing lands in it when in
            // normal mode it does not.
            Mode::Insert => tab.caret.and_then(|caret| caret.cell(&self.vp)),
            Mode::Command(buffer) => chrome::command_caret(buffer, self.grid),
            Mode::Normal | Mode::Hint(_) => None,
        });
        frame
    }
```

`on_job(Job::Extracted)` gains one line, because the tab now remembers where the page is scrolled to and Task 12 needs it:

```rust
                self.focused_mut().scroll_y = extraction.scroll_y;
```

`start_extract` and `navigate` operate on the focused tab throughout.

- [ ] **Step 7: Run the tests**

Run: `cargo test -p wwt`
Expected: PASS. **Every assertion in `session.rs`'s test module is unchanged from before this task.** If you needed to change one, the move was not behaviour-preserving; find out why before continuing.

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/wwt/src/tab.rs crates/wwt/src/session.rs crates/wwt/src/lib.rs
git commit -m "refactor(wwt): put a page's state in a tab and the browser's in the session

No behaviour change: one tab, in a vector, and every assertion in the test
module is untouched. What moves is the boundary. Session keeps the grid, the
mode, the : line and which tab is in front; everything that is true of a page
rather than of the browser now belongs to the page it is true of.

session.rs had grown to 982 lines by being two things at once."
```

---

### Task 6: Every effect and every job names a tab

Still one tab, and still no behaviour change, but the vocabulary can now say which page it means and `Core` can hold more than one. This is the seam widening; opening a second tab in Task 8 is then just arithmetic.

`Job::InputFailed` is renamed `Job::Noted`. It always meant "this failed after the loop had moved on, so say so in the statusline and change nothing", and Tasks 8 and 12 give it two users that are not input.

**Files:**
- Modify: `crates/wwt/src/effect.rs`
- Modify: `crates/wwt/src/event.rs`
- Modify: `crates/wwt/src/session.rs`
- Modify: `crates/wwt/src/core.rs`
- Modify: `crates/wwt/src/input.rs`

**Interfaces:**
- Consumes: `wwt::tab::TabId` from Task 5.
- Produces: the enums below, and `InputPump::spawn(jobs: mpsc::UnboundedSender<Job>) -> InputPump` with `InputPump::send(&self, page: Arc<Page>, input: Input)`.
- Produces: `Core::new(page: Arc<Page>, client: Arc<Client>, grid: GridSize, cell: CellSize) -> Core`, unchanged in signature; it now files that page under `session.focused_id()`.

- [ ] **Step 1: Write the failing test**

In `crates/wwt/src/session.rs`, add to `mod tests`:

```rust
    /// The id of the tab a fresh session starts with.
    fn tab0() -> TabId {
        TabId(0)
    }

    #[test]
    fn every_effect_says_which_page_it_is_for() {
        let mut session = session();
        assert_eq!(session.begin(), vec![Effect::Extract(tab0())]);

        let mut session = ready();
        assert_eq!(
            session.on(key('j')),
            vec![Effect::Scroll(tab0(), Scroll::By(20.0))]
        );
    }

    #[test]
    fn a_job_for_a_tab_that_is_gone_is_dropped_rather_than_painted() {
        // Nothing can close a tab yet, but the guard is what makes Task 8
        // safe, and a job carrying an unknown id must never be looked up.
        let mut session = ready();
        let stale = Job::Extracted(TabId(999), Box::new(extraction("https://elsewhere.test")));
        assert_eq!(session.on(Event::Done(stale)), vec![]);
        assert_eq!(session.focused().url, "https://example.com", "the frame is untouched");
    }
```

- [ ] **Step 2: Run the test and watch it fail**

Run: `cargo test -p wwt --lib session`
Expected: compilation fails, `this enum variant takes 0 arguments but 1 argument was supplied`.

- [ ] **Step 3: Widen the effect vocabulary**

In `crates/wwt/src/effect.rs`:

```rust
use wwt_frame::Viewport;
use wwt_page::Input;

use crate::tab::TabId;

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Read the page.
    Extract(TabId),
    /// Ask the page for its interactive boxes.
    Hints(TabId),
    Scroll(TabId, Scroll),
    Navigate(TabId, Navigation),
    /// Send one key or click to the page, in order with the others.
    Send(TabId, Input),
    /// Take focus off whatever has it.
    Blur(TabId),
    /// Tell the page the window is this size. The terminal has already
    /// changed; this is the page catching up. Emitted once per tab, because
    /// a background tab has to be the right size already when you reach it.
    SetViewport(TabId, Viewport),
    /// Turn terminal mouse reporting on or off.
    MouseCapture(bool),
    Quit,
}
```

`Scroll` and `Navigation` are unchanged.

- [ ] **Step 4: Widen the event vocabulary**

In `crates/wwt/src/event.rs`:

```rust
use crossterm::event::{KeyEvent, MouseEvent};
use wwt_frame::{CellSize, GridSize, HintTarget};
use wwt_page::Extraction;

use crate::tab::TabId;

/// Something that happened. Everything that can move the browser arrives
/// as one of these.
#[derive(Debug, Clone)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    /// The terminal has been re-measured after a resize.
    Resized(GridSize, CellSize),
    /// A page says it changed under us. Which page matters: one browser
    /// serves all of them and they all report on one subscription.
    Dirty(TabId),
    /// Something that ran off the loop's thread finished.
    Done(Job),
}

/// The result of something that ran off the loop's thread.
///
/// Every variant that came from a page names it, and a job whose tab is no
/// longer open is dropped. A page operation outlives the state that asked
/// for it, so the answer has to say what it was an answer to.
#[derive(Debug, Clone)]
pub enum Job {
    Extracted(TabId, Box<Extraction>),
    Failed(TabId, String),
    /// A navigation, history move, or reload finished.
    Settled(TabId),
    /// The page reported its interactive boxes, or could not. One variant
    /// rather than two, so there is exactly one place that can forget the
    /// query is no longer in flight.
    Hints(TabId, Result<Vec<HintTarget>, String>),
    /// The page has been told the window changed size.
    Resized(TabId),
    /// Something failed after the loop had moved on: a keystroke, a click, a
    /// blur. Say so in the statusline and change nothing else.
    Noted(String),
}
```

- [ ] **Step 5: Teach the session to say which tab**

In `crates/wwt/src/session.rs`, every effect the session pushes now carries `self.focused_id()`. Bind it once at the top of each function that pushes more than one, because `focused_id()` borrows `self` and the pushes do not.

`on` gains the routing that makes a stale job harmless. Add a helper and use it from `on_job`:

```rust
    /// The tab a job is about, or `None` if it has since been closed.
    ///
    /// A page operation outlives the state that asked for it. Looking the id
    /// up rather than assuming the focused tab is what lets a slow load in a
    /// backgrounded tab land in that tab, and a load in a closed one land
    /// nowhere.
    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id == id)
    }
```

`on_job` becomes a match that resolves the tab first. `Job::Noted` has no tab and keeps going to the focused one, because the statusline you are looking at is the focused tab's:

```rust
    fn on_job(&mut self, job: Job, effects: &mut Vec<Effect>) {
        let id = match &job {
            Job::Extracted(id, _)
            | Job::Failed(id, _)
            | Job::Settled(id)
            | Job::Hints(id, _)
            | Job::Resized(id) => *id,
            // The frame stays exactly as it was; only the statusline
            // changes. Spec section 8. Deliberately not `Job::Failed`: that
            // one clears the extraction and navigation flags, and a
            // keystroke that failed has finished neither of those.
            Job::Noted(message) => {
                self.focused_mut().state = State::Error(message);
                return;
            }
        };
        if self.tab_mut(id).is_none() {
            // The tab was closed while this was in flight. Its id is never
            // reused, so there is no page this could belong to instead.
            return;
        }
        // ... the existing per-variant handling, against `self.tab_mut(id)`
    }
```

Write the per-variant handling against the resolved tab. Two of the arms also push effects, and `start_extract` needs to take an id rather than assume the focused tab:

```rust
    fn start_extract(&mut self, id: TabId, effects: &mut Vec<Effect>) {
        let focused = self.focused_id() == id;
        let Some(tab) = self.tab_mut(id) else { return };
        if tab.extracting || !tab.dirty {
            return;
        }
        // A background tab keeps its flag and spends it when focus arrives.
        // Reading a page nobody is looking at is a round trip for a frame
        // nobody will see, and spec section 3 is explicit that an idle
        // background tab must cost what an idle foreground tab costs.
        if !focused {
            return;
        }
        tab.extracting = true;
        tab.dirty = false;
        effects.push(Effect::Extract(id));
    }
```

`Event::Dirty` carries the id, so it marks that tab and tries to extract it:

```rust
            Event::Dirty(id) => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.mark_dirty();
                }
                self.start_extract(id, &mut effects);
            }
```

- [ ] **Step 6: Give the core a map of pages**

In `crates/wwt/src/core.rs`:

```rust
use std::collections::HashMap;

use crate::tab::TabId;

pub struct Core {
    pages: HashMap<TabId, Arc<Page>>,
    client: Arc<Client>,
    renderer: Renderer,
    session: Session,

    jobs_tx: mpsc::UnboundedSender<Job>,
    jobs_rx: mpsc::UnboundedReceiver<Job>,

    /// Ordered delivery of keys and clicks, across every page.
    input: InputPump,
}

impl Core {
    pub fn new(page: Arc<Page>, client: Arc<Client>, grid: GridSize, cell: CellSize) -> Self {
        let (jobs_tx, jobs_rx) = mpsc::unbounded_channel();
        let input = InputPump::spawn(jobs_tx.clone());
        let session = Session::new(grid, cell);
        let pages = HashMap::from([(session.focused_id(), page)]);

        Self {
            pages,
            client,
            renderer: Renderer::new(),
            session,
            jobs_tx,
            jobs_rx,
            input,
        }
    }
```

The CDP arm asks the map which page reported, rather than the one page there used to be. It still borrows only one field, so the other futures in the `select!` are unaffected:

```rust
                Some(event) = cdp.recv() => self
                    .pages
                    .iter()
                    .find(|(_, page)| page.is_dirty(&event))
                    .map(|(id, _)| Event::Dirty(*id)),
```

`spawn` takes the tab it is for, and drops the request if there is no page for it:

```rust
    /// Run one page operation off the loop's thread and report what it did.
    ///
    /// The one place anything is spawned. A thirty-second load still leaves
    /// keys responsive because nothing here is awaited by the loop, and each
    /// operation says for itself what its failure means by choosing the
    /// `Job` it reports, or reporting none.
    ///
    /// An effect naming a page we do not hold is dropped. That is reachable
    /// only between asking for a tab and being told it opened, where the tab
    /// is marked loading and nothing could have expected to land.
    fn spawn<F, Fut>(&self, id: TabId, make: F)
    where
        F: FnOnce(Arc<Page>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Option<Job>> + Send,
    {
        let Some(page) = self.pages.get(&id).map(Arc::clone) else {
            return;
        };
        let tx = self.jobs_tx.clone();
        tokio::spawn(async move {
            if let Some(job) = make(page).await {
                let _ = tx.send(job);
            }
        });
    }
```

Every arm of `apply` passes its id through. For example:

```rust
                Effect::Send(id, input) => {
                    if let Some(page) = self.pages.get(&id) {
                        self.input.send(Arc::clone(page), input);
                    }
                }

                Effect::Extract(id) => self.spawn(id, move |page| async move {
                    Some(match page.extract().await {
                        Ok(extraction) => Job::Extracted(id, Box::new(extraction)),
                        Err(error) => Job::Failed(id, error.to_string()),
                    })
                }),

                Effect::Blur(id) => self.spawn(id, move |page| async move {
                    page.blur().await.err().map(|e| Job::Noted(e.to_string()))
                }),
```

`resize_page` takes an id too, and `Effect::SetViewport(id, vp)` calls it with that id.

- [ ] **Step 7: Let the pump carry a page with each input**

In `crates/wwt/src/input.rs`, the channel carries the page as well as the input, so ordering stays global across a tab switch rather than becoming per-page:

```rust
/// The sending half of the pump. The core holds one.
pub struct InputPump {
    tx: mpsc::UnboundedSender<(Arc<Page>, Input)>,
}

impl InputPump {
    /// Start the pump.
    ///
    /// One pump for every page, not one per page: keys typed either side of
    /// a tab switch must not overtake each other, and two channels would make
    /// their order a matter of which task woke first.
    ///
    /// Failures are reported as a `Job` rather than returned: by the time a
    /// keystroke fails, whoever typed it has typed three more. They go on
    /// the channel every other finished page operation goes on, so the loop
    /// has one thing to select on rather than two.
    pub fn spawn(jobs: mpsc::UnboundedSender<Job>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<(Arc<Page>, Input)>();

        tokio::spawn(async move {
            while let Some((page, input)) = rx.recv().await {
                if let Err(error) = page.dispatch(&input).await {
                    let _ = jobs.send(Job::Noted(error.to_string()));
                }
            }
        });

        Self { tx }
    }

    /// Queue one input. Never blocks, never fails: a closed channel means
    /// the pump task is gone, which only happens on the way out.
    pub fn send(&self, page: Arc<Page>, input: Input) {
        let _ = self.tx.send((page, input));
    }
}
```

- [ ] **Step 8: Update the tests**

Every assertion in `session.rs`'s test module that names an effect or builds a job gains `tab0()`. This is mechanical: `Effect::Extract` becomes `Effect::Extract(tab0())`, `Job::Settled` becomes `Job::Settled(tab0())`, `Job::Hints(Ok(t))` becomes `Job::Hints(tab0(), Ok(t))`, and the `hinted` helper becomes:

```rust
    /// The page answering a hint query.
    fn hinted(targets: Vec<HintTarget>) -> Event {
        Event::Done(Job::Hints(tab0(), Ok(targets)))
    }
```

`crates/wwt/tests/smoke.rs` asserts on one effect and needs the same treatment:

```rust
    assert_eq!(
        effects,
        vec![Effect::Navigate(
            wwt::tab::TabId(0),
            Navigation::Open("https://example.com".to_string())
        )]
    );
```

- [ ] **Step 9: Run the tests**

Run: `cargo test --workspace`
Expected: PASS. Assertions gained an id and nothing else; if a test's *meaning* had to change, something in Step 5 was more than a rename.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 10: Commit**

```bash
git add crates/wwt/src
git commit -m "refactor(wwt): make every effect and job say which page it means

Still one tab. What changes is that a page operation's answer can be
identified: Core holds a map of pages instead of one, and a job whose tab has
since been closed is dropped rather than looked up. Ids are never reused, so
there is never a page a stale answer could plausibly belong to.

Job::InputFailed becomes Job::Noted. It always meant that something failed
after the loop had moved on; a close and a save are about to need it too."
```

---

### Task 7: What a page can be asked to do

Four small CDP calls that the tab machinery needs and does not have. They are here rather than inline in the tasks that use them because each needs a browser to test and the tasks that use them do not.

**Files:**
- Modify: `crates/wwt-page/src/extract.rs`
- Modify: `crates/wwt-page/tests/interaction.rs`

**Interfaces:**
- Produces: `Page::scroll_to(&self, y: f64) -> Result<()>`, `Page::activate(&self) -> Result<()>`, `Page::close(&self) -> Result<()>`, `Page::target_id(&self) -> &str`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/wwt-page/tests/interaction.rs`, following the file's existing harness usage:

```rust
#[tokio::test]
async fn scrolling_to_an_offset_lands_there() {
    let page = harness().page("tall.html").await;

    page.scroll_to(400.0).await.expect("scroll to an offset");
    let extraction = page.extract().await.expect("extract");

    // Chromium clamps to the scrollable range and rounds to device pixels,
    // so this asserts the neighbourhood rather than the exact number.
    assert!(
        (extraction.scroll_y - 400.0).abs() < 2.0,
        "scrolled to {}, wanted 400",
        extraction.scroll_y
    );
}

#[tokio::test]
async fn two_pages_on_one_browser_read_their_own_documents() {
    // One client, one websocket, one event stream. Every command a page
    // issues is `call_on` its own session, and this is what says so: two
    // targets alive at once, each extracting what is in it and not what is
    // in the other.
    let first = harness().open(&fixture_url("hello.html")).await;
    let second = harness().open(&fixture_url("blank.html")).await;

    let one = first.extract().await.expect("extract the first");
    let two = second.extract().await.expect("extract the second");

    assert!(one.runs.iter().any(|run| run.text.contains("hello")), "{:?}", one.runs);
    assert!(two.runs.iter().any(|run| run.text.contains("opener")), "{:?}", two.runs);
    assert_ne!(one.url, two.url);
}

#[tokio::test]
async fn a_closed_page_is_gone_from_the_browser() {
    let page = harness().open("about:blank").await;
    let target = page.target_id().to_string();

    page.close().await.expect("close the target");

    let targets = harness().client().call("Target.getTargets", serde_json::json!({}))
        .await
        .expect("list targets");
    let still_there = targets["targetInfos"]
        .as_array()
        .expect("an array of targets")
        .iter()
        .any(|info| info["targetId"] == target.as_str());
    assert!(!still_there, "the target outlived the close");
}

#[tokio::test]
async fn activating_a_page_makes_it_the_one_the_browser_has_in_front() {
    // Input dispatch is answered by whichever target is in front, which is
    // why switching tabs has to activate. Two targets, activate the first,
    // and it is the one that takes the click.
    let first = harness().open(&fixture_url("click.html")).await;
    let _second = harness().open("about:blank").await;

    first.activate().await.expect("activate the first page");

    let at = wwt_frame::CssPoint { x: 40.0, y: 20.0 };
    first.dispatch_mouse(&MouseInput::press(at)).await.expect("press");
    first.dispatch_mouse(&MouseInput::release(at)).await.expect("release");

    let clicked = first.eval("window.__clicked === true").await.expect("read the flag");
    assert_eq!(clicked, serde_json::Value::Bool(true));
}
```

`hello.html` and `blank.html` are written out in Task 11 Step 2; create them here instead, since this task needs them first.

Read `crates/wwt-page/tests/interaction.rs` and `crates/wwt-page/tests/common` first and adapt the harness calls to whatever the existing helpers are actually named; the three tests above assume a `page(fixture)`, an `open(url)`, and a `client()`. If the harness has no `open` or `client`, add the smallest one that serves these tests, in `tests/common`, rather than reshaping the harness.

`tall.html` and `click.html` may already exist in `crates/wwt-page/tests/fixtures`. If not, create them:

```html
<!-- tall.html -->
<!doctype html><meta charset="utf-8"><title>Tall</title>
<style>body { margin: 0 } p { margin: 0; height: 40px }</style>
<script>for (let i = 0; i < 200; i++) document.write("<p>line " + i + "</p>")</script>
```

```html
<!-- click.html -->
<!doctype html><meta charset="utf-8"><title>Click</title>
<style>body { margin: 0 } button { width: 200px; height: 60px }</style>
<button onclick="window.__clicked = true">press me</button>
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p wwt-page --test interaction`
Expected: compilation fails, `no method named scroll_to found`.

- [ ] **Step 3: Write the implementation**

In `crates/wwt-page/src/extract.rs`, the private `scroll_to` helper needs a name of its own before the public one can have that one. Rename it and add the four methods:

```rust
    pub async fn scroll_to_top(&self) -> Result<()> {
        self.scroll_to_expression("0").await
    }

    /// Jump to the end of the document.
    ///
    /// This is the one place M2 does not scroll natively: the distance to the
    /// document's end is not known to us, and on an infinite-scroll page it
    /// changes as we go. The consequence is that this reaches the end of what
    /// has loaded, which is the correct behavior; it is simply not
    /// wheel-driven.
    pub async fn scroll_to_end(&self) -> Result<()> {
        self.scroll_to_expression("document.documentElement.scrollHeight").await
    }

    /// Put the document at an absolute offset.
    ///
    /// Restoring a scroll position, and nothing else. A wheel event would be
    /// the wrong tool: we know exactly where the page should be, and letting
    /// Chromium animate its way there would mean the extraction after it
    /// reads a position on the way rather than the one asked for.
    pub async fn scroll_to(&self, y: f64) -> Result<()> {
        self.scroll_to_expression(&y.to_string()).await
    }

    async fn scroll_to_expression(&self, y_expression: &str) -> Result<()> {
        self.client
            .call_on(
                &self.session_id,
                "Runtime.evaluate",
                json!({
                    "expression": format!("window.scrollTo(0, {y_expression})"),
                    "returnByValue": true,
                }),
            )
            .await
            .context("scroll to a document position")?;
        Ok(())
    }
```

Then, beside `session_id`:

```rust
    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    /// Make this page the one the browser has in front.
    ///
    /// `Input.dispatchMouseEvent` is answered by whichever target is
    /// foreground, so switching tabs without this would leave clicks landing
    /// on the page you just left. M5's screencast will want the same
    /// guarantee.
    pub async fn activate(&self) -> Result<()> {
        self.client
            .call(
                "Target.activateTarget",
                json!({ "targetId": self.target_id }),
            )
            .await
            .context("activate the target")?;
        Ok(())
    }

    /// Close this page's target.
    ///
    /// Browser-level rather than `call_on`: a session cannot outlive the
    /// target it is attached to, so asking the target to close itself races
    /// its own answer.
    pub async fn close(&self) -> Result<()> {
        self.client
            .call("Target.closeTarget", json!({ "targetId": self.target_id }))
            .await
            .context("close the target")?;
        Ok(())
    }
```

`Page` must keep its target id for these, so add the field and set it in `open`:

```rust
pub struct Page {
    client: Arc<Client>,
    session_id: String,
    target_id: String,
}
```

In `Page::open`, `target_id` is already a local; pass it into the struct instead of dropping it:

```rust
        let page = Page { client, session_id, target_id };
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p wwt-page`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/wwt-page
git commit -m "feat(page): let a page be scrolled to a point, activated and closed

Activation is not cosmetic. Input.dispatchMouseEvent is answered by whichever
target the browser has in front, which was a test-harness quirk with one
target and is a correctness rule with several: switching tabs without it
leaves clicks landing on the page you just left."
```

---

### Task 8: Opening and closing tabs

The first task where there is more than one page. Opening is asynchronous and closing is not, which is why opening needs somewhere for the new `Page` to arrive.

A `Page` belongs to `Core` and must never reach `Session`, so the loop's one result channel grows a second shape: most of what arrives is a `Job` on its way to the session unchanged, and a target that finished opening is not. The session hears only that the tab opened.

**Files:**
- Modify: `crates/wwt/src/effect.rs`, `event.rs`, `session.rs`, `core.rs`, `keymap.rs`
- Modify: `crates/wwt-ui/src/command.rs`
- Modify: `crates/wwt-page/src/extract.rs` (a `Debug` for `Page`)

**Interfaces:**
- Produces: `Effect::OpenTab { id: TabId, url: String }`, `Effect::CloseTab(TabId)`, `Effect::Activate(TabId)`.
- Produces: `Job::Opened(TabId, Result<(), String>)`.
- Produces: `Action::TabClose`; `Command::TabOpen(String)`, `Command::TabClose`.
- Produces on `Session`: `tabs(&self) -> &[Tab]`.

- [ ] **Step 1: Write the failing tests**

In `crates/wwt-ui/src/command.rs`, add to `mod tests`:

```rust
    #[test]
    fn tabopen_normalizes_its_url_the_way_open_does() {
        assert_eq!(
            parse("tabopen example.com"),
            Ok(Command::TabOpen("https://example.com".to_string()))
        );
    }

    #[test]
    fn tabopen_without_a_url_is_an_error_rather_than_a_blank_tab() {
        assert!(parse("tabopen").is_err());
    }

    #[test]
    fn tabclose_takes_no_argument() {
        assert_eq!(parse("tabclose"), Ok(Command::TabClose));
    }
```

In `crates/wwt/src/session.rs`, add to `mod tests`:

```rust
    // Opening and closing.

    #[test]
    fn opening_a_tab_asks_for_a_page_and_moves_you_to_it() {
        let mut session = ready();
        let effects = session.on(key('t'));
        assert!(matches!(session.mode(), Mode::Command(buffer) if buffer == "tabopen "));
        assert_eq!(effects, vec![], "`t` only opens the : line");

        typed(&mut session, "example.org");
        let effects = session.on(code(KeyCode::Enter));

        assert_eq!(session.tabs().len(), 2);
        assert_eq!(session.focused().id, TabId(1), "a new tab is the one you are looking at");
        assert_eq!(
            effects,
            vec![Effect::OpenTab {
                id: TabId(1),
                url: "https://example.org".to_string()
            }]
        );
    }

    #[test]
    fn a_tab_that_finished_opening_is_activated_and_read() {
        let mut session = ready();
        typed(&mut session, ":tabopen example.org");
        session.on(code(KeyCode::Enter));

        let effects = session.on(Event::Done(Job::Opened(TabId(1), Ok(()))));
        assert_eq!(
            effects,
            vec![Effect::Activate(TabId(1)), Effect::Extract(TabId(1))]
        );
    }

    #[test]
    fn a_tab_that_could_not_be_opened_leaves_you_where_you_were() {
        let mut session = ready();
        typed(&mut session, ":tabopen example.org");
        session.on(code(KeyCode::Enter));

        session.on(Event::Done(Job::Opened(TabId(1), Err("no target".to_string()))));
        assert_eq!(session.tabs().len(), 1, "a tab with no page is not a tab");
        assert_eq!(session.focused().id, tab0());
        assert!(matches!(session.state(), State::Error(_)));
    }

    #[test]
    fn closing_the_focused_tab_lands_you_on_its_right_hand_neighbour() {
        let mut session = ready();
        open_two_more(&mut session);
        // Three tabs, focused on the middle one.
        session.on(key('K'));
        assert_eq!(session.focused().id, TabId(1));

        let effects = session.on(key('x'));
        assert!(effects.contains(&Effect::CloseTab(TabId(1))));
        assert_eq!(session.tabs().len(), 2);
        assert_eq!(session.focused().id, TabId(2), "the right-hand neighbour took its place");
    }

    #[test]
    fn closing_a_tab_to_your_left_leaves_you_looking_at_the_same_page() {
        let mut session = ready();
        open_two_more(&mut session);
        assert_eq!(session.focused().id, TabId(2));

        session.close_tab(tab0(), &mut Vec::new());
        assert_eq!(session.focused().id, TabId(2), "you did not move");
    }

    #[test]
    fn closing_the_last_tab_quits() {
        let mut session = ready();
        let effects = session.on(key('x'));
        assert!(effects.contains(&Effect::CloseTab(tab0())));
        assert!(effects.contains(&Effect::Quit), "a browser with no page in it is not a state");
    }

    #[test]
    fn a_closed_tabs_late_answer_lands_nowhere() {
        let mut session = ready();
        open_two_more(&mut session);
        session.on(key('x')); // closes TabId(2), leaving 0 and 1

        let late = Job::Extracted(TabId(2), Box::new(extraction("https://gone.test")));
        assert_eq!(session.on(Event::Done(late)), vec![]);
        assert_eq!(session.tabs().len(), 2);
    }
```

And the helper, beside the other test helpers:

```rust
    /// Two more tabs, both opened and settled, focus left on the last.
    fn open_two_more(session: &mut Session) {
        for (n, url) in [(1u32, "one.test"), (2, "two.test")] {
            typed(session, &format!(":tabopen {url}"));
            session.on(code(KeyCode::Enter));
            session.on(Event::Done(Job::Opened(TabId(n), Ok(()))));
            session.on(Event::Done(Job::Extracted(
                TabId(n),
                Box::new(extraction(&format!("https://{url}"))),
            )));
        }
    }
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p wwt --lib session`
Expected: compilation fails, `no variant named OpenTab found for enum Effect`.

- [ ] **Step 3: Add the commands**

In `crates/wwt-ui/src/command.rs`, add the variants and the parse arms:

```rust
pub enum Command {
    Open(String),
    TabOpen(String),
    TabClose,
    Back,
    Forward,
    Reload,
    Set(Setting),
    Quit,
}
```

```rust
        "tabopen" | "t" => {
            if rest.is_empty() {
                return Err("tabopen needs a URL".to_string());
            }
            Ok(Command::TabOpen(normalize_url(rest)?))
        }
        "tabclose" => Ok(Command::TabClose),
```

- [ ] **Step 4: Add the keys**

In `crates/wwt/src/keymap.rs`, add the action and the bindings. `d` and `u` are half-page scroll, so qutebrowser's `d` is not free for close and `x` takes it:

```rust
    TabClose,
```

```rust
        KeyCode::Char('t') => Some(Action::EnterCommand("tabopen ".to_string())),
        KeyCode::Char('x') => Some(Action::TabClose),
```

Add a test in that file's `mod tests`:

```rust
    #[test]
    fn t_and_x_open_and_close_tabs_in_normal_mode_only() {
        assert_eq!(
            action_for(&normal_mode(), key('t'), vp()),
            Some(Action::EnterCommand("tabopen ".to_string()))
        );
        assert_eq!(action_for(&normal_mode(), key('x'), vp()), Some(Action::TabClose));
        // Insert mode types them, as it types everything.
        assert!(matches!(action_for(&Mode::Insert, key('x'), vp()), Some(Action::Send(_))));
    }
```

- [ ] **Step 5: Add the effects and the job**

In `crates/wwt/src/effect.rs`:

```rust
    /// Create a target for a tab the session has already made room for, and
    /// navigate it.
    OpenTab { id: TabId, url: String },
    CloseTab(TabId),
    /// Make this tab the one the browser has in front. Input dispatch is
    /// answered by whichever target is foreground, so ours and the browser's
    /// have to be the same one.
    Activate(TabId),
```

In `crates/wwt/src/event.rs`:

```rust
    /// A tab's target was created and navigated, or could not be. The page
    /// itself never reaches the session; `Core` keeps it.
    Opened(TabId, Result<(), String>),
```

- [ ] **Step 6: Teach the session to open, close and focus**

In `crates/wwt/src/session.rs`:

```rust
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Make room for a tab and ask for its page.
    ///
    /// The tab exists before its page does. Between here and `Job::Opened`
    /// it is marked loading and `Core` holds nothing for it, so effects
    /// naming it are dropped; nothing could have expected to land.
    fn open_tab(&mut self, url: String, effects: &mut Vec<Effect>) {
        let id = self.mint();
        let mut tab = Tab::new(id, url.clone());
        tab.navigating = true;
        self.tabs.push(tab);
        self.focus = self.tabs.len() - 1;
        effects.push(Effect::OpenTab { id, url });
    }

    /// Close a tab, and go wherever that leaves you.
    fn close_tab(&mut self, id: TabId, effects: &mut Vec<Effect>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        effects.push(Effect::CloseTab(id));
        self.tabs.remove(index);

        if self.tabs.is_empty() {
            // A browser with no page in it is not a state worth having, and
            // it is the same rule `q` follows.
            effects.push(Effect::Quit);
            return;
        }

        if index < self.focus {
            // Something to the left went. You are still looking at the same
            // page; only its index moved.
            self.focus -= 1;
            return;
        }
        if index > self.focus {
            return;
        }
        // The page you were looking at went, and its right-hand neighbour
        // has taken its index, which is where the eye already is.
        self.focus = index.min(self.tabs.len() - 1);
        let id = self.focused_id();
        effects.push(Effect::Activate(id));
        self.start_extract(id, effects);
    }
```

The command and action arms:

```rust
            Action::TabClose => {
                let id = self.focused_id();
                self.close_tab(id, effects);
            }
```

```rust
            Command::TabOpen(url) => self.open_tab(url, effects),
            Command::TabClose => {
                let id = self.focused_id();
                self.close_tab(id, effects);
            }
```

And the job arm. Opening finishes a navigation, so it does what `Job::Settled` does, plus the activation the new foreground needs:

```rust
            Job::Opened(id, Ok(())) => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.navigating = false;
                    tab.state = State::Ready;
                    tab.mark_dirty();
                }
                if self.focused_id() == id {
                    effects.push(Effect::Activate(id));
                }
                self.start_extract(id, effects);
            }
            Job::Opened(id, Err(message)) => {
                // A tab with no page behind it is not a tab. Drop it and say
                // why, without disturbing the one you were on.
                self.close_tab(id, &mut Vec::new());
                self.focused_mut().state = State::Error(message);
            }
```

`Job::Opened(id, Err(_))` calls `close_tab` with a throwaway effect vector on purpose: the target was never created, so asking `Core` to close it would be asking about something that does not exist.

- [ ] **Step 7: Give the core somewhere to put a new page**

In `crates/wwt-page/src/extract.rs`, `Page` has to be printable for the channel type below to keep its derives:

```rust
/// Its identity and nothing else: a `Page` is a handle on a browser, and the
/// browser is not something to print.
impl std::fmt::Debug for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Page")
            .field("target", &self.target_id)
            .field("session", &self.session_id)
            .finish()
    }
}
```

In `crates/wwt/src/core.rs`, the channel carries one more shape, and the loop unwraps it where it can borrow `self`:

```rust
/// What arrives on the loop's one result channel.
///
/// Most of it is a `Job` on its way to the session unchanged. A target that
/// finished opening is not: the `Page` it produced belongs to `Core`, and the
/// session must never hold one, so the page is filed here and the session
/// hears only that the tab opened.
#[derive(Debug)]
enum Finished {
    Job(Job),
    Opened(TabId, Result<Arc<Page>, String>),
}
```

`jobs_tx` and `jobs_rx` become `mpsc::{UnboundedSender, UnboundedReceiver}<Finished>`, and every existing `tx.send(job)` becomes `tx.send(Finished::Job(job))`. `spawn`'s closure return type stays `Option<Job>` and `spawn` wraps it.

The select arm and the line after it:

```rust
                Some(finished) = self.jobs_rx.recv() => Some(Incoming::Finished(finished)),
```

where `Incoming` is the other half of the same idea:

```rust
/// What one turn of the loop picked up. An arm produces one of these and
/// touches nothing, because borrowing `self` in one while the other futures
/// are alive is what used to force a whole spawned task to merge two
/// channels into one.
enum Incoming {
    Event(Event),
    Finished(Finished),
}
```

Every other arm wraps its event in `Incoming::Event`. After the `select!`:

```rust
            // A page is `Core`'s. This is where one is filed, because it is
            // the first point in the turn that can borrow `self` mutably.
            let event = match incoming {
                Some(Incoming::Event(event)) => Some(event),
                Some(Incoming::Finished(Finished::Job(job))) => Some(Event::Done(job)),
                Some(Incoming::Finished(Finished::Opened(id, Ok(page)))) => {
                    self.pages.insert(id, page);
                    Some(Event::Done(Job::Opened(id, Ok(()))))
                }
                Some(Incoming::Finished(Finished::Opened(id, Err(error)))) => {
                    Some(Event::Done(Job::Opened(id, Err(error))))
                }
                None => None,
            };
```

The three new effect arms:

```rust
                Effect::OpenTab { id, url } => {
                    let vp = self.session.viewport();
                    let client = Arc::clone(&self.client);
                    let tx = self.jobs_tx.clone();
                    tokio::spawn(async move {
                        let opened = Page::open(client, &url, vp)
                            .await
                            .map(Arc::new)
                            .map_err(|error| error.to_string());
                        let _ = tx.send(Finished::Opened(id, opened));
                    });
                }

                Effect::CloseTab(id) => {
                    // Taken out of the map first: whatever happens to the
                    // target, nothing may still be sent to a tab the session
                    // has already let go of.
                    if let Some(page) = self.pages.remove(&id) {
                        let tx = self.jobs_tx.clone();
                        tokio::spawn(async move {
                            if let Err(error) = page.close().await {
                                let _ = tx.send(Finished::Job(Job::Noted(error.to_string())));
                            }
                        });
                    }
                }

                Effect::Activate(id) => self.spawn(id, |page| async move {
                    page.activate().await.err().map(|e| Job::Noted(e.to_string()))
                }),
```

`Effect::OpenTab` cannot go through `spawn`, which exists to hand an operation a page that already exists. This is the one effect whose whole purpose is that there is not one yet.

- [ ] **Step 8: Run the tests**

Run: `cargo test -p wwt -p wwt-ui`
Expected: PASS.

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 9: Try it**

Run: `cargo run -p wwt -- example.com`, press `t`, type `news.ycombinator.com`, press Enter. Two tabs in the bar, the second focused and readable. Press `x`; you are back on the first. Press `x` again; it quits.

- [ ] **Step 10: Commit**

```bash
git add crates/wwt crates/wwt-ui crates/wwt-page
git commit -m "feat(wwt): open and close tabs

t opens the : line prefilled with tabopen, the way o prefills open; x closes
the focused tab, because d is half-page scroll and not free. Closing the last
tab quits, which is the rule q already follows.

A Page belongs to Core and must never reach Session, so the loop's one result
channel grew a second shape: a target that finished opening files its page in
Core and the session hears only that the tab opened. Opening is also the one
effect that cannot go through spawn, whose whole job is handing an operation a
page that already exists."
```

---

### Task 9: Switching tabs

`J` and `K`, and what it means for the tab you left. A background tab keeps its runs, so a switch paints immediately and the round trip only refreshes; and it keeps its dirty flag rather than spending it, so an idle background tab costs what an idle foreground tab costs, which is nothing.

`J` and `K` are bound in normal mode only, so a switch always begins and ends there and no rule is needed for what happens to insert mode when the page under it changes.

**Files:**
- Modify: `crates/wwt/src/keymap.rs`, `session.rs`
- Modify: `crates/wwt-ui/src/command.rs`

**Interfaces:**
- Produces: `Action::TabNext`, `Action::TabPrev`; `Command::TabNext`, `Command::TabPrev`.

- [ ] **Step 1: Write the failing tests**

In `crates/wwt/src/session.rs`, add to `mod tests`:

```rust
    // Switching.

    #[test]
    fn j_and_k_move_between_tabs_and_wrap() {
        let mut session = ready();
        open_two_more(&mut session);
        assert_eq!(session.focused().id, TabId(2));

        session.on(key('J'));
        assert_eq!(session.focused().id, tab0(), "past the last tab is the first");

        session.on(key('K'));
        assert_eq!(session.focused().id, TabId(2), "before the first is the last");
    }

    #[test]
    fn switching_activates_the_tab_you_switched_to() {
        // Input dispatch is answered by whichever target the browser has in
        // front, so a switch that does not activate leaves clicks landing on
        // the page you just left.
        let mut session = ready();
        open_two_more(&mut session);
        let effects = session.on(key('J'));
        assert!(effects.contains(&Effect::Activate(tab0())));
    }

    #[test]
    fn a_switch_paints_the_page_you_switched_to_before_anyone_asks_the_browser() {
        let mut session = ready();
        session.focused_mut().runs = vec![run("first tab")];
        open_two_more(&mut session);

        session.on(key('J'));
        let frame = session.compose();
        assert!(
            (0..frame.grid().rows).any(|r| frame.row_text(r).contains("first tab")),
            "the cached frame is what makes a switch a repaint rather than a round trip"
        );
    }

    #[test]
    fn a_background_tab_that_changed_is_not_read_until_you_look_at_it() {
        let mut session = ready();
        open_two_more(&mut session);

        // Tab 0 is in the background and its page says it moved.
        assert_eq!(
            session.on(Event::Dirty(tab0())),
            vec![],
            "an idle background tab must cost what an idle foreground tab costs"
        );

        // Switching to it spends the flag.
        let effects = session.on(key('J'));
        assert!(effects.contains(&Effect::Extract(tab0())));
    }

    #[test]
    fn switching_to_a_tab_that_did_not_change_costs_no_round_trip() {
        let mut session = ready();
        open_two_more(&mut session);
        session.on(key('J')); // to tab 0, spending its flag
        session.on(Event::Done(Job::Extracted(tab0(), Box::new(extraction("https://example.com")))));

        let effects = session.on(key('K'));
        assert_eq!(effects, vec![Effect::Activate(TabId(2))], "nothing to re-read");
    }

    #[test]
    fn a_hint_answer_for_a_tab_you_have_left_does_not_put_labels_over_another_page() {
        let mut session = ready();
        open_two_more(&mut session);

        session.on(key('f'));
        session.on(key('J'));
        session.on(hinted_for(TabId(2), vec![target(TargetKind::Clickable)]));

        assert_eq!(
            session.mode(),
            &Mode::Normal,
            "labels measured against one page must not be painted over another"
        );
    }
```

Add two helpers beside the others:

```rust
    fn hinted_for(id: TabId, targets: Vec<HintTarget>) -> Event {
        Event::Done(Job::Hints(id, Ok(targets)))
    }

    fn run(text: &str) -> TextRun {
        TextRun {
            text: text.to_string(),
            rect: CssRect { x: 0.0, y: 0.0, w: 400.0, h: 20.0 },
            baseline: 16.0,
            style: Style { fg: Rgb { r: 0xd0, g: 0xd0, b: 0xd0 }, bold: false, reverse: false },
            z: 0,
        }
    }
```

`TextRun` is defined in `crates/wwt-frame/src/run.rs` and those are its five fields. The test module already imports `TextRun`; add `Rgb` and `Style` to its `wwt_frame` import.

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p wwt --lib session`
Expected: compilation fails, `no variant named TabNext found for enum Action`.

- [ ] **Step 3: Add the keys and commands**

In `crates/wwt/src/keymap.rs`:

```rust
    TabNext,
    TabPrev,
```

```rust
        // qutebrowser's own bindings. vim's `gt` would mean a pending-prefix
        // state and rebinding `g`, which is scroll-top, to buy nothing.
        KeyCode::Char('J') => Some(Action::TabNext),
        KeyCode::Char('K') => Some(Action::TabPrev),
```

In `crates/wwt-ui/src/command.rs`, `TabNext` and `TabPrev` variants and their arms:

```rust
        "tabnext" => Ok(Command::TabNext),
        "tabprev" => Ok(Command::TabPrev),
```

- [ ] **Step 4: Write the switch**

In `crates/wwt/src/session.rs`:

```rust
    /// Look at another tab.
    ///
    /// The cached runs are painted the moment this returns, so a switch is a
    /// repaint; the extraction only refreshes what is already on screen. That
    /// is what a background tab keeps its runs for.
    fn focus_tab(&mut self, index: usize, effects: &mut Vec<Effect>) {
        if index >= self.tabs.len() || index == self.focus {
            return;
        }
        self.focus = index;
        let id = self.focused_id();
        // The browser's foreground and ours have to be the same target, or
        // input lands on the page you just left.
        effects.push(Effect::Activate(id));
        // Spends the dirty flag this tab has been accumulating in the
        // background, and does nothing if it has none.
        self.start_extract(id, effects);
    }

    /// The tab `steps` along from the focused one, wrapping.
    fn neighbour(&self, steps: isize) -> usize {
        let count = self.tabs.len() as isize;
        if count == 0 {
            return 0;
        }
        (self.focus as isize + steps).rem_euclid(count) as usize
    }
```

The action and command arms:

```rust
            Action::TabNext => {
                let index = self.neighbour(1);
                self.focus_tab(index, effects);
            }
            Action::TabPrev => {
                let index = self.neighbour(-1);
                self.focus_tab(index, effects);
            }
```

```rust
            Command::TabNext => {
                let index = self.neighbour(1);
                self.focus_tab(index, effects);
            }
            Command::TabPrev => {
                let index = self.neighbour(-1);
                self.focus_tab(index, effects);
            }
```

- [ ] **Step 5: Keep a late hint answer off another page**

In `on_job`, the `Job::Hints` arm gains a clause. M3 established that a late answer opens hint mode only if the mode is still normal, because a round trip is long enough to have typed half a `:` command. It must now also be true that the answering tab is still the one you are looking at:

```rust
            Job::Hints(id, result) => {
                // However it went, that tab's query is over and `f` must
                // work on it again.
                if let Some(tab) = self.tab_mut(id) {
                    tab.hinting = false;
                }
                match result {
                    Ok(targets) => {
                        if let Some(tab) = self.tab_mut(id) {
                            tab.hints = Some(targets.clone());
                        }
                        // A query is a round trip, and the keystroke that
                        // asked for it was normal mode's, on a tab that was
                        // in front. Landing the answer in whatever mode you
                        // have since entered would take the command line out
                        // from under you mid-word, and landing it on another
                        // tab would paint one page's labels over another's
                        // text.
                        if self.mode == Mode::Normal && self.focused_id() == id {
                            self.enter_hints(targets);
                        }
                    }
                    Err(message) => {
                        if let Some(tab) = self.tab_mut(id) {
                            tab.state = State::Error(message);
                        }
                    }
                }
            }
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p wwt -p wwt-ui`
Expected: PASS.

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Try it**

Run: `cargo run -p wwt -- example.com`, open two more tabs with `t`, then `J` and `K` around them. Switching should be instantaneous, because it is a repaint. Scroll one tab, switch away and back, and the scroll position is where you left it.

- [ ] **Step 8: Commit**

```bash
git add crates/wwt crates/wwt-ui
git commit -m "feat(wwt): move between tabs with J and K

A background tab keeps its runs, so a switch paints before anything asks the
browser and the extraction only refreshes what is already there. It also
keeps its dirty flag rather than spending it, so an idle background tab costs
what an idle foreground tab costs, which is nothing.

Switching activates: input dispatch is answered by whichever target the
browser has in front, so ours and the browser's must be the same one.

A late hint answer now needs its tab to still be focused as well as the mode
to still be normal, or labels measured against one page get painted over
another's text."
```

---

### Task 10: A resize reaches every tab

Small, and easy to forget until a tab you switch to is laid out for the terminal you had ten minutes ago. A background tab has to be the right size already when you reach it, not a round trip after.

**Files:**
- Modify: `crates/wwt/src/session.rs`

- [ ] **Step 1: Write the failing test**

Replace `a_resize_tells_the_page_before_reading_it` in `crates/wwt/src/session.rs`'s `mod tests`:

```rust
    #[test]
    fn a_resize_tells_every_tab_before_reading_any_of_them() {
        let mut session = ready();
        open_two_more(&mut session);
        let grid = GridSize { cols: 100, rows: 30 };
        let vp = page_viewport(grid, CELL);

        let effects = session.on(Event::Resized(grid, CELL));
        assert_eq!(
            effects,
            vec![
                Effect::SetViewport(tab0(), vp),
                Effect::SetViewport(TabId(1), vp),
                Effect::SetViewport(TabId(2), vp),
            ],
            "a tab you switch to must already be the size of the terminal you have"
        );

        assert_eq!(
            session.on(Event::Done(Job::Resized(TabId(2)))),
            vec![Effect::Extract(TabId(2))],
            "reading before the page has reflowed reads the old layout"
        );
        assert_eq!(
            session.on(Event::Done(Job::Resized(tab0()))),
            vec![],
            "a background tab keeps the flag until you look at it"
        );
    }
```

- [ ] **Step 2: Run the test and watch it fail**

Run: `cargo test -p wwt --lib a_resize_tells_every_tab`
Expected: FAIL, the effects vector has one element rather than three.

- [ ] **Step 3: Write the implementation**

In `crates/wwt/src/session.rs`:

```rust
    fn on_resize(&mut self, grid: GridSize, cell: CellSize, effects: &mut Vec<Effect>) {
        if grid == self.grid && cell == self.cell {
            return;
        }
        self.grid = grid;
        self.cell = cell;
        self.vp = page_viewport(grid, cell);
        // Every tab, not just the one in front: a background tab laid out
        // for the terminal you used to have would be wrong the moment you
        // reached it, and reaching it is the one moment there is no time to
        // fix it in. The page genuinely reflows; extraction waits for
        // `Job::Resized`, because reading before the page has been resized
        // reads the old layout.
        for tab in &self.tabs {
            effects.push(Effect::SetViewport(tab.id, self.vp));
        }
    }
```

And the job arm:

```rust
            Job::Resized(id) => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.mark_dirty();
                }
                self.start_extract(id, effects);
            }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/wwt/src/session.rs
git commit -m "fix(wwt): resize the tabs you are not looking at too

A background tab laid out for the terminal you used to have is wrong the
moment you reach it, and reaching it is the one moment there is no time to
fix it in."
```

---

### Task 11: Tabs the page opens for itself

`target=_blank` and `window.open` create targets we never asked for. Without this a browser that has tabs still cannot follow the links that use them, which reads as broken rather than as a limitation.

The fiddly part is timing. A target opened by a page starts loading its document immediately, and `Page.addScriptToEvaluateOnNewDocument` only affects documents that have not started, so an adopted tab attached late has no bootstrap in it and cannot be extracted at all. `Target.setAutoAttach` with `waitForDebuggerOnStart` is what buys the window: the target is held before it runs, we install, and then we let it go.

Auto-attach delivers a session for **every** new target, including the ones `Page::open` creates, so this task unifies the two paths: `open` stops attaching for itself and waits for the same event adoption waits for. Targets a page opened are told apart by `targetInfo.openerId`, which `Target.createTarget` does not set.

**Files:**
- Create: `crates/wwt-cdp/src/target.rs`
- Modify: `crates/wwt-cdp/src/client.rs`, `crates/wwt-cdp/src/lib.rs`
- Modify: `crates/wwt-page/src/extract.rs`
- Modify: `crates/wwt/src/event.rs`, `effect.rs`, `session.rs`, `core.rs`, `main.rs`
- Modify: `crates/wwt-page/tests/interaction.rs`

**Interfaces:**
- Produces: `wwt_cdp::{TargetId, Attached}`. `TargetId(pub String)`; `Attached { pub target: TargetId, pub session: String }`, deriving `Debug, Clone, PartialEq, Eq`.
- Produces: `Client::auto_attach(&self) -> Result<()>`, `Client::attached_page(event: &Event) -> Option<Attached>`, `Client::opened_by_a_page(event: &Event) -> Option<Attached>`.
- Produces: `Page::adopt(client: Arc<Client>, attached: Attached, vp: Viewport) -> Result<Page>`.
- Produces: `Event::TargetOpened(Attached)`, `Effect::AdoptTab { id: TabId, target: Attached }`.

- [ ] **Step 1: Find out how the browser actually behaves**

Before writing anything, confirm the two facts this task rests on. Write this as a temporary test in `crates/wwt-page/tests/interaction.rs`, run it with `--nocapture`, read the output, then delete it:

```rust
#[tokio::test]
async fn spike_what_auto_attach_reports() {
    let client = harness().client();
    client
        .call(
            "Target.setAutoAttach",
            serde_json::json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
        )
        .await
        .expect("turn on auto-attach");

    let mut events = client.subscribe();
    let page = harness().open("about:blank").await;
    page.eval("window.open('about:blank')").await.expect("open a tab");

    for _ in 0..8 {
        let Ok(Some(event)) =
            tokio::time::timeout(std::time::Duration::from_secs(2), events.recv()).await
        else {
            break;
        };
        if event.method.starts_with("Target.") {
            eprintln!("{} {}", event.method, event.params);
        }
    }
}
```

You are checking two things:
1. `Target.attachedToTarget` fires for a target `window.open` created, and its `params.targetInfo.openerId` is a non-empty string.
2. It also fires for a target `Target.createTarget` created, and *that* one's `openerId` is absent or empty.

If `openerId` does not tell the two apart, stop and report it: the discriminator is what keeps `Page::open` from adopting its own targets, and there is no second candidate in the event.

- [ ] **Step 2: Write the failing test**

In `crates/wwt-page/tests/interaction.rs`:

```rust
#[tokio::test]
async fn a_tab_the_page_opened_for_itself_is_adopted_with_our_script_in_it() {
    let client = harness().client();
    client.auto_attach().await.expect("turn on auto-attach");

    let mut events = client.subscribe();
    let page = harness().open(&fixture_url("blank.html")).await;

    page.eval(&format!("window.open('{}')", fixture_url("hello.html")))
        .await
        .expect("open a tab");

    let attached = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.expect("the browser stayed up");
            if let Some(attached) = wwt_cdp::Client::opened_by_a_page(&event) {
                return attached;
            }
        }
    })
    .await
    .expect("the browser reported the tab its page opened");

    let vp = Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
    let adopted = Page::adopt(client, attached, vp).await.expect("adopt it");

    // The assertion that matters: the bootstrap is in the document the tab
    // loaded, not merely in the next one it navigates to. A target attached
    // after it started running would fail exactly here.
    let extraction = adopted.extract().await.expect("extract the adopted page");
    assert!(
        extraction.runs.iter().any(|run| run.text.contains("hello")),
        "adopted page had no text: {:?}",
        extraction.runs
    );
}
```

`hello.html` and `blank.html` in `crates/wwt-page/tests/fixtures`, if Task 7 did not already create them:

```html
<!-- hello.html -->
<!doctype html><meta charset="utf-8"><title>Hello</title>
<style>body { margin: 0 }</style>
<p>hello from an adopted tab</p>
```

```html
<!-- blank.html -->
<!doctype html><meta charset="utf-8"><title>Blank</title><style>body{margin:0}</style><p>opener</p>
```

- [ ] **Step 3: Run the test and watch it fail**

Run: `cargo test -p wwt-page --test interaction a_tab_the_page_opened`
Expected: compilation fails, `no method named auto_attach found`.

- [ ] **Step 4: Name a target**

Create `crates/wwt-cdp/src/target.rs`:

```rust
//! Which target, and which session on it.
//!
//! A CDP fact that travels out through the vocabulary and back: the browser
//! reports a target it attached to, and the answer is a page opened on it.
//! It is a value with no behaviour on purpose, so carrying one costs the
//! carrier no knowledge of what a target is.

/// One target in the browser, for as long as it exists.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TargetId(pub String);

/// A target the browser has attached us to, and the session to speak to it
/// on. Every command a page issues is `call_on` its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attached {
    pub target: TargetId,
    pub session: String,
}
```

Add `pub mod target;` and `pub use target::{Attached, TargetId};` to `crates/wwt-cdp/src/lib.rs`, keeping the file's existing ordering.

- [ ] **Step 5: Turn auto-attach on and read its events**

In `crates/wwt-cdp/src/client.rs`:

```rust
    /// Attach to every page target the browser opens, and hold each one
    /// before it runs.
    ///
    /// The hold is the point. A target a page opened starts loading its
    /// document at once, and a script registered with
    /// `Page.addScriptToEvaluateOnNewDocument` only reaches documents that
    /// have not started, so a target attached late has no bootstrap in it and
    /// cannot be read at all. `Runtime.runIfWaitingForDebugger` lets it go
    /// once we have installed one.
    pub async fn auto_attach(&self) -> Result<()> {
        self.call(
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
        )
        .await
        .context("turn on auto-attach")?;
        Ok(())
    }

    /// The target this event says the browser attached us to, if it is a
    /// page rather than a worker or an iframe.
    pub fn attached_page(event: &Event) -> Option<Attached> {
        if event.method != "Target.attachedToTarget" {
            return None;
        }
        let info = event.params.get("targetInfo")?;
        if info.get("type")?.as_str()? != "page" {
            return None;
        }
        Some(Attached {
            target: TargetId(info.get("targetId")?.as_str()?.to_string()),
            session: event.params.get("sessionId")?.as_str()?.to_string(),
        })
    }

    /// The same, narrowed to targets a page opened rather than ones we asked
    /// for.
    ///
    /// `openerId` is the discriminator: a page that calls `window.open` is
    /// the opener of what it opens, and `Target.createTarget` has none. It is
    /// what keeps `Page::open` from adopting its own target.
    pub fn opened_by_a_page(event: &Event) -> Option<Attached> {
        let attached = Self::attached_page(event)?;
        let opener = event.params["targetInfo"].get("openerId")?.as_str()?;
        (!opener.is_empty()).then_some(attached)
    }
```

Add unit tests in that file's `mod tests`, beside the existing event-parsing ones, using the same `one(...)` helper:

```rust
    #[test]
    fn a_target_a_page_opened_is_told_apart_from_one_we_asked_for() {
        let ours = one(
            r#"{"method":"Target.attachedToTarget","params":{"sessionId":"S1",
                "targetInfo":{"targetId":"T1","type":"page","openerId":""}}}"#,
        );
        assert!(Client::attached_page(&ours).is_some());
        assert_eq!(Client::opened_by_a_page(&ours), None, "we created this one");

        let theirs = one(
            r#"{"method":"Target.attachedToTarget","params":{"sessionId":"S2",
                "targetInfo":{"targetId":"T2","type":"page","openerId":"T1"}}}"#,
        );
        assert_eq!(
            Client::opened_by_a_page(&theirs),
            Some(Attached { target: TargetId("T2".to_string()), session: "S2".to_string() })
        );
    }

    #[test]
    fn a_worker_is_not_a_tab() {
        let worker = one(
            r#"{"method":"Target.attachedToTarget","params":{"sessionId":"S3",
                "targetInfo":{"targetId":"T3","type":"worker","openerId":"T1"}}}"#,
        );
        assert_eq!(Client::attached_page(&worker), None);
    }
```

- [ ] **Step 6: Give `Page` one way to be prepared**

In `crates/wwt-page/src/extract.rs`, widen the `wwt_cdp` import to `use wwt_cdp::{Attached, Client, Event, TargetId};`. `open` and `adopt` differ only in where the session came from, so they share everything after that. Replace `Page::open`:

```rust
    /// Create a target, prepare it, and navigate.
    ///
    /// The session is not asked for: auto-attach delivers one for every new
    /// target, so waiting for it here is what keeps a tab we opened and a tab
    /// a page opened on the same path. Subscribed before the create, because
    /// the attach for a fast target arrives before the create's answer does.
    pub async fn open(client: Arc<Client>, url: &str, vp: Viewport) -> Result<Page> {
        let mut events = client.subscribe();
        let created = client
            .call("Target.createTarget", json!({ "url": "about:blank" }))
            .await
            .context("create a page target")?;
        let target = TargetId(
            created["targetId"]
                .as_str()
                .ok_or_else(|| anyhow!("Target.createTarget returned no targetId"))?
                .to_string(),
        );

        let attached = timeout(LOAD_TIMEOUT, async {
            loop {
                let event = events.recv().await.ok_or_else(|| anyhow!("the browser went away"))?;
                if let Some(attached) = Client::attached_page(&event) {
                    if attached.target == target {
                        return Ok::<_, anyhow::Error>(attached);
                    }
                }
            }
        })
        .await
        .map_err(|_| anyhow!("the browser did not attach to the target it created"))??;

        let page = Page::prepare(client, attached, vp).await?;
        page.navigate(url).await?;
        Ok(page)
    }

    /// Take over a target the browser opened for a page.
    ///
    /// It is held before its first document runs, so the bootstrap installed
    /// here is in the document the tab actually loaded rather than in the
    /// next one it navigates to.
    pub async fn adopt(client: Arc<Client>, attached: Attached, vp: Viewport) -> Result<Page> {
        Page::prepare(client, attached, vp).await
    }

    /// Everything a target needs before it is worth looking at.
    ///
    /// The order is load-bearing: the binding exists before the bootstrap
    /// that calls it, the bootstrap is registered before the document that
    /// should contain it runs, and nothing runs until the last line.
    async fn prepare(client: Arc<Client>, attached: Attached, vp: Viewport) -> Result<Page> {
        let page = Page {
            client,
            session_id: attached.session,
            target_id: attached.target.0,
        };
        page.client
            .call_on(&page.session_id, "Page.enable", json!({}))
            .await
            .context("enable the Page domain")?;
        page.client
            .call_on(&page.session_id, "Runtime.enable", json!({}))
            .await
            .context("enable the Runtime domain")?;
        page.client
            .call_on(
                &page.session_id,
                "Runtime.addBinding",
                json!({ "name": DIRTY_BINDING }),
            )
            .await
            .context("install the dirty-signal binding")?;
        page.install_bootstrap().await?;
        page.set_viewport(vp).await?;
        // Let it go. Until this, the target is held before its first script
        // runs, which is the whole reason the bootstrap above is in the
        // document it loads.
        page.client
            .call_on(&page.session_id, "Runtime.runIfWaitingForDebugger", json!({}))
            .await
            .context("release the target")?;
        Ok(page)
    }
```

- [ ] **Step 7: Adopt it in the browser**

In `crates/wwt/src/event.rs`:

```rust
    /// A page opened a tab for itself. The session has to make room for it
    /// before it can be prepared, because ids are minted on that side.
    TargetOpened(Attached),
```

In `crates/wwt/src/effect.rs`:

```rust
    /// Prepare a target the browser already attached us to, as the tab the
    /// session has just made for it.
    AdoptTab { id: TabId, target: Attached },
```

In `crates/wwt/src/session.rs`, adoption is `open_tab` with the target already in hand. The tab opens in the foreground, which is what clicking such a link does in any other browser:

```rust
            Event::TargetOpened(target) => {
                let id = self.mint();
                let mut tab = Tab::new(id, String::new());
                tab.navigating = true;
                self.tabs.push(tab);
                self.focus = self.tabs.len() - 1;
                effects.push(Effect::AdoptTab { id, target });
            }
```

In `crates/wwt/src/core.rs`, the CDP arm learns a second question, and adoption reuses the `Finished::Opened` path so a tab that arrived unasked and a tab you asked for are filed the same way:

```rust
                Some(event) = cdp.recv() => {
                    if let Some(attached) = Client::opened_by_a_page(&event) {
                        Some(Incoming::Event(Event::TargetOpened(attached)))
                    } else {
                        self.pages
                            .iter()
                            .find(|(_, page)| page.is_dirty(&event))
                            .map(|(id, _)| Incoming::Event(Event::Dirty(*id)))
                    }
                }
```

```rust
                Effect::AdoptTab { id, target } => {
                    let vp = self.session.viewport();
                    let client = Arc::clone(&self.client);
                    let tx = self.jobs_tx.clone();
                    tokio::spawn(async move {
                        let opened = Page::adopt(client, target, vp)
                            .await
                            .map(Arc::new)
                            .map_err(|error| error.to_string());
                        let _ = tx.send(Finished::Opened(id, opened));
                    });
                }
```

In `crates/wwt/src/main.rs`, turn it on before the first target exists:

```rust
    client.auto_attach().await.context("watch for tabs the page opens")?;
```

That line goes immediately after `Client::connect` and before `Page::open`, or the first tab races the setting.

- [ ] **Step 8: Close what cannot be prepared**

A target we attached to and then failed to prepare is held before its first script and will sit there forever. `Job::Opened(id, Err(_))` already drops the tab; make the adopted case also close the target, in `Core`:

```rust
                Some(Incoming::Finished(Finished::Opened(id, Err(error)))) => {
                    // A target held before its first script and never
                    // released is a tab nobody can see and nothing can stop.
                    // We have no page to close it with, so it is closed by
                    // id, and a failure to close is not worth a second
                    // notice on top of the first.
                    if let Some(target) = self.opening.remove(&id) {
                        let client = Arc::clone(&self.client);
                        tokio::spawn(async move {
                            let _ = client
                                .call("Target.closeTarget", serde_json::json!({ "targetId": target.0 }))
                                .await;
                        });
                    }
                    Some(Event::Done(Job::Opened(id, Err(error))))
                }
```

`opening: HashMap<TabId, TargetId>` is a field on `Core`, written when `Effect::AdoptTab` is applied and removed here. `Effect::OpenTab` does not write to it: a target whose creation failed does not exist to close.

- [ ] **Step 9: Run the tests**

Run: `cargo test -p wwt-cdp -p wwt-page`
Expected: PASS.

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 10: Try it**

Write a scratch fixture with `<a href="https://example.com" target="_blank">new tab</a>`, open it, press `f`, and follow the link. A second tab appears in the bar, focused, with the page in it.

- [ ] **Step 11: Commit**

```bash
git add crates/wwt-cdp crates/wwt-page crates/wwt
git commit -m "feat(wwt): adopt the tabs a page opens for itself

A browser with tabs that cannot follow a target=_blank link is not one. The
timing is the whole difference: a target a page opened starts loading at
once, and addScriptToEvaluateOnNewDocument only reaches documents that have
not started, so a target attached late has no bootstrap in it and cannot be
read at all. Auto-attach with waitForDebuggerOnStart holds it until we have
installed one.

Auto-attach delivers a session for every new target, so Page::open now waits
for the same event adoption waits for and the two paths are one. They are
told apart by openerId, which Target.createTarget does not set.

Adopted tabs open in the foreground, like clicking such a link anywhere else.
A middle click wants a background tab and targetCreated does not say so; it
is not bound, so it does not yet arise."
```

---

### Task 12: Writing the session down

Deciding when it is worth saving is a rule and belongs in `Session`; coalescing the writes and making the syscalls is machinery and belongs in `Core`. That division is why a held `j` costs one write rather than one per frame.

**Files:**
- Modify: `crates/wwt/src/effect.rs`, `session.rs`, `core.rs`

**Interfaces:**
- Consumes: `wwt::store::{Snapshot, SavedTab, save}` from Task 2.
- Produces: `Effect::Save(Snapshot)`; `Session::snapshot(&self) -> Snapshot`.

- [ ] **Step 1: Write the failing tests**

In `crates/wwt/src/session.rs`, add to `mod tests`:

```rust
    // The session file.

    fn saved(effects: &[Effect]) -> Option<&Snapshot> {
        effects.iter().find_map(|effect| match effect {
            Effect::Save(snapshot) => Some(snapshot),
            _ => None,
        })
    }

    #[test]
    fn a_snapshot_is_the_tabs_you_have_and_the_one_you_are_looking_at() {
        let mut session = ready();
        open_two_more(&mut session);

        let snapshot = session.snapshot();
        assert_eq!(snapshot.version, crate::store::VERSION);
        assert_eq!(snapshot.focus, 2);
        assert_eq!(snapshot.tabs.len(), 3);
        assert_eq!(snapshot.tabs[0].url, "https://example.com");
        assert_eq!(snapshot.tabs[0].title, "Example");
    }

    #[test]
    fn opening_a_tab_is_worth_writing_down() {
        let mut session = ready();
        typed(&mut session, ":tabopen example.org");
        let effects = session.on(code(KeyCode::Enter));
        assert!(saved(&effects).is_some(), "the tab set changed");
    }

    #[test]
    fn closing_a_tab_is_worth_writing_down() {
        let mut session = ready();
        open_two_more(&mut session);
        let effects = session.on(key('x'));
        assert_eq!(saved(&effects).map(|s| s.tabs.len()), Some(2));
    }

    #[test]
    fn switching_tabs_is_worth_writing_down() {
        let mut session = ready();
        open_two_more(&mut session);
        let effects = session.on(key('J'));
        assert_eq!(saved(&effects).map(|s| s.focus), Some(0));
    }

    #[test]
    fn an_extraction_that_moved_the_page_is_worth_writing_down() {
        let mut session = ready();
        let mut moved = extraction("https://example.com");
        moved.scroll_y = 240.0;

        let effects = session.on(Event::Done(Job::Extracted(tab0(), Box::new(moved))));
        assert_eq!(saved(&effects).map(|s| s.tabs[0].scroll_y), Some(240.0));
    }

    #[test]
    fn an_extraction_that_changed_nothing_is_not_worth_a_write() {
        let mut session = ready();
        session.focused_mut().dirty = true;
        let effects = session.on(Event::Done(Job::Extracted(
            tab0(),
            Box::new(extraction("https://example.com")),
        )));
        assert!(
            saved(&effects).is_none(),
            "an idle page must not turn into a write per extraction"
        );
    }
```

`session.rs` needs `use crate::store::{SavedTab, Snapshot};` at the top, beside the other `crate::` imports.

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p wwt --lib session`
Expected: compilation fails, `no variant named Save found for enum Effect`.

- [ ] **Step 3: Add the effect**

In `crates/wwt/src/effect.rs`:

```rust
use crate::store::Snapshot;
```

```rust
    /// Write the open tabs down. Coalesced by the loop, so asking often is
    /// cheap and asking on every scroll frame is what keeps a crash from
    /// costing you your place.
    Save(Snapshot),
```

- [ ] **Step 4: Decide when it is worth saving**

In `crates/wwt/src/session.rs`:

```rust
    /// The open tabs, as they would be restored.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            version: crate::store::VERSION,
            focus: self.focus,
            tabs: self
                .tabs
                .iter()
                .map(|tab| SavedTab {
                    url: tab.url.clone(),
                    title: tab.title.clone(),
                    scroll_y: tab.scroll_y,
                })
                .collect(),
        }
    }

    /// Note that what a restart would come back to has changed.
    fn save(&self, effects: &mut Vec<Effect>) {
        effects.push(Effect::Save(self.snapshot()));
    }
```

Call it from `open_tab`, `close_tab` and `focus_tab`, after the state has changed and before returning. In the `Job::Extracted` arm, only when something a restart would notice actually moved:

```rust
                let moved = tab.scroll_y != extraction.scroll_y
                    || tab.url != extraction.url
                    || tab.title != extraction.title;
                // ... the existing assignments ...
                if moved {
                    self.save(effects);
                }
```

Compute `moved` before the assignments, from the old values against the new. An extraction of a page that has not moved must not become a write, or an idle page costs a syscall per dirty signal.

- [ ] **Step 5: Fix the assertions that now have a save in them**

`open_tab`, `close_tab` and `focus_tab` push one effect more than they did, so three tests from Tasks 8 and 9 that assert on an exact effect vector need `Effect::Save(session.snapshot())` added, or their assertion loosened to `contains`:

- `opening_a_tab_asks_for_a_page_and_moves_you_to_it`
- `switching_to_a_tab_that_did_not_change_costs_no_round_trip`
- any other `assert_eq!(effects, vec![...])` the compiler and the test run point you at.

Prefer adding the expected `Effect::Save` over loosening to `contains`: the point of an exact vector is that it says everything a keystroke costs, and a write is part of what it costs.

- [ ] **Step 6: Coalesce and write**

In `crates/wwt/src/core.rs`:

```rust
/// A held `j` produces a scroll and an extraction per frame, and every one of
/// them changes the scroll offset a restart would come back to. Writing each
/// would be a syscall per frame for a file nobody reads until the next launch.
const SAVE_DEBOUNCE: Duration = Duration::from_secs(1);
```

`Core` gains two fields, set by `Startup` in Task 13 and initialized to `None` by this task's `Core::new`. `core.rs` needs `use std::path::PathBuf;` and `use crate::store::Snapshot;`:

```rust
    /// Where the session file goes, or `None` when this instance does not own
    /// it. A private session, on a profile another instance holds, writes
    /// nothing.
    session_file: Option<PathBuf>,
    /// The most recent snapshot not yet written.
    pending: Option<Snapshot>,
```

A timer arm beside the resize one, and the same shape:

```rust
                () = async { sleep_until(save_at.expect("guarded")).await },
                    if save_at.is_some() =>
                {
                    save_at = None;
                    self.flush_save();
                    None
                }
```

The arm produces no event on purpose: a write changes nothing about what is on screen, and composing again would build the same frame and diff it against itself.

```rust
    /// Write the pending snapshot, if there is one and it is ours to write.
    ///
    /// `spawn_blocking` rather than the loop's own thread: it is a small file
    /// and a rename, but the loop's promise is that nothing in it waits on a
    /// syscall.
    fn flush_save(&mut self) {
        let (Some(path), Some(snapshot)) = (self.session_file.clone(), self.pending.take()) else {
            return;
        };
        let tx = self.jobs_tx.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(error) = crate::store::save(&path, &snapshot) {
                let _ = tx.send(Finished::Job(Job::Noted(error)));
            }
        });
    }
```

The effect arm arms the timer rather than writing:

```rust
                Effect::Save(snapshot) => {
                    self.pending = Some(snapshot);
                    save_at = Some(Instant::now() + SAVE_DEBOUNCE);
                }
```

`save_at` lives beside `resize_at` as a local in `run`, so `apply` needs it passed in as `&mut Option<Instant>`; add it as a parameter rather than making it a field, which is what `resize_at` would have done if `apply` had needed it.

And on the way out, `Effect::Quit` flushes before returning, because the last second of browsing is exactly the part you would notice missing:

```rust
                Effect::Quit => {
                    self.flush_save();
                    return Ok(true);
                }
```

A write started this way is not awaited, so quitting does not wait on the disk. `spawn_blocking` runs on a thread the runtime does not drop until the task finishes, so the write completes even as `main` returns.

- [ ] **Step 7: Run the tests**

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/wwt/src
git commit -m "feat(wwt): write the open tabs down as they change

Deciding when it is worth saving is a rule and lives in Session; coalescing
the writes and making the syscalls is machinery and lives in Core. That
division is what keeps a held j at one write rather than one per frame, and
what keeps an extraction of a page that did not move from being a write at
all.

Quitting flushes first: the last second of browsing is the part you would
notice missing."
```

---

### Task 13: Coming back to where you were

The payoff. The profile makes logins durable, the snapshot makes tabs durable, and a second instance gets neither and is told so.

This is also where `main` stops opening a page for `Core` and every tab comes into being the same way, through an effect.

**Files:**
- Modify: `crates/wwt/src/main.rs`, `core.rs`, `session.rs`, `tab.rs`, `effect.rs`

**Interfaces:**
- Consumes: `store::{profile_path, session_path, load}`, `Chromium::launch(Option<&Path>)`.
- Produces: `Scroll::To(f64)`.
- Produces: `wwt::core::Startup { grid, cell, snapshot, open, session_file }` and `Core::new(client: Arc<Client>, startup: Startup) -> Core`.
- Produces: `Session::restore(grid: GridSize, cell: CellSize, snapshot: Option<Snapshot>, open: Option<String>) -> Session`.
- Produces on `Tab`: `opened: bool`, `read: bool`.

- [ ] **Step 1: Write the failing tests**

In `crates/wwt/src/session.rs`, add to `mod tests`:

```rust
    // Restore.

    fn snapshot_of(urls: &[&str], focus: usize) -> Snapshot {
        Snapshot {
            version: crate::store::VERSION,
            focus,
            tabs: urls
                .iter()
                .map(|url| SavedTab {
                    url: (*url).to_string(),
                    title: "saved".to_string(),
                    scroll_y: 120.0,
                })
                .collect(),
        }
    }

    #[test]
    fn restoring_asks_for_every_tab_that_was_open() {
        let mut session = Session::restore(
            GRID,
            CELL,
            Some(snapshot_of(&["https://one.test", "https://two.test"], 1)),
            None,
        );
        assert_eq!(session.tabs().len(), 2);
        assert_eq!(session.focused().url, "https://two.test", "you come back where you were");

        let effects = session.begin();
        assert_eq!(
            effects,
            vec![
                Effect::OpenTab { id: TabId(0), url: "https://one.test".to_string() },
                Effect::OpenTab { id: TabId(1), url: "https://two.test".to_string() },
            ]
        );
    }

    #[test]
    fn a_url_on_the_command_line_is_a_new_tab_beside_the_restored_ones() {
        // Nothing you had is lost by typing `wwt example.com` out of habit,
        // which is the failure mode that actually costs something.
        let session = Session::restore(
            GRID,
            CELL,
            Some(snapshot_of(&["https://one.test"], 0)),
            Some("https://asked.test".to_string()),
        );
        assert_eq!(session.tabs().len(), 2);
        assert_eq!(session.focused().url, "https://asked.test");
    }

    #[test]
    fn no_snapshot_and_no_url_is_one_blank_tab() {
        let session = Session::restore(GRID, CELL, None, None);
        assert_eq!(session.tabs().len(), 1);
        assert_eq!(session.focused().url, "about:blank");
    }

    #[test]
    fn a_snapshot_with_no_tabs_in_it_still_leaves_you_a_browser() {
        let empty = Snapshot { version: crate::store::VERSION, focus: 0, tabs: Vec::new() };
        let session = Session::restore(GRID, CELL, Some(empty), None);
        assert_eq!(session.tabs().len(), 1, "a browser with no page in it is not a state");
    }

    #[test]
    fn a_focus_index_past_the_end_of_the_snapshot_lands_on_a_real_tab() {
        // The file is data from disk and is not trusted.
        let session = Session::restore(
            GRID,
            CELL,
            Some(snapshot_of(&["https://one.test", "https://two.test"], 99)),
            None,
        );
        assert_eq!(session.focused().url, "https://two.test");
    }

    #[test]
    fn a_restored_tab_goes_back_to_the_offset_it_was_left_at() {
        let mut session = Session::restore(GRID, CELL, Some(snapshot_of(&["https://one.test"], 0)), None);
        session.begin();

        let effects = session.on(Event::Done(Job::Opened(tab0(), Ok(()))));
        assert!(effects.contains(&Effect::Scroll(tab0(), Scroll::To(120.0))));
    }

    #[test]
    fn every_restored_tab_is_read_once_so_the_bar_has_real_titles() {
        let mut session = Session::restore(
            GRID,
            CELL,
            Some(snapshot_of(&["https://one.test", "https://two.test"], 1)),
            None,
        );
        session.begin();

        // Tab 0 is in the background and has never been read. It is read
        // anyway, once, which is what makes the first switch to it instant.
        let effects = session.on(Event::Done(Job::Opened(tab0(), Ok(()))));
        assert!(effects.contains(&Effect::Extract(tab0())));

        // And having been read, it goes quiet.
        session.on(Event::Done(Job::Extracted(tab0(), Box::new(extraction("https://one.test")))));
        assert_eq!(session.on(Event::Dirty(tab0())), vec![]);
    }
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p wwt --lib session`
Expected: compilation fails, `no function or associated item named restore found`.

- [ ] **Step 3: Let a tab remember whether it exists and whether it has been read**

In `crates/wwt/src/tab.rs`, two fields, both false for a tab nobody has opened yet:

```rust
    /// A target exists for this tab. False between asking for one and being
    /// told it opened, which is the window in which effects naming this tab
    /// are dropped.
    pub opened: bool,
    /// This tab has been read at least once, so its title is real and its
    /// runs are worth painting. Until then it is read even in the background:
    /// that first read is what makes the first switch to it instant.
    pub read: bool,
```

In `crates/wwt/src/session.rs`, `start_extract` gains the clause that makes "idle" mean "after the first read":

```rust
        // A background tab keeps its flag and spends it when focus arrives.
        // The exception is a tab nobody has read yet: reading it once is what
        // puts a real title in the bar and makes the first switch to it a
        // repaint rather than a round trip.
        if !focused && tab.read {
            return;
        }
```

and `Job::Extracted` sets `tab.read = true` beside the other assignments.

`Job::Opened(id, Ok(()))` sets `tab.opened = true`, and emits the scroll a restored tab needs:

```rust
                let mut restore_to = None;
                if let Some(tab) = self.tab_mut(id) {
                    tab.opened = true;
                    tab.navigating = false;
                    tab.state = State::Ready;
                    tab.mark_dirty();
                    if tab.scroll_y > 0.0 {
                        restore_to = Some(tab.scroll_y);
                    }
                }
                if self.focused_id() == id {
                    effects.push(Effect::Activate(id));
                }
                if let Some(y) = restore_to {
                    effects.push(Effect::Scroll(id, Scroll::To(y)));
                }
                self.start_extract(id, effects);
```

The extraction may read the page before the scroll has landed, which costs a background tab one stale cached frame. It corrects itself: the scroll fires the page's own scroll listener, which sets the tab's dirty flag, which is spent the moment you switch to it.

- [ ] **Step 4: Add the absolute scroll**

In `crates/wwt/src/effect.rs`:

```rust
pub enum Scroll {
    /// By a distance in CSS pixels, positive being downward.
    By(f64),
    /// To an absolute offset. Restoring a saved position, and nothing else.
    To(f64),
    Top,
    End,
}
```

In `crates/wwt/src/core.rs`, the arm beside the others:

```rust
                            Scroll::To(y) => page.scroll_to(y).await,
```

- [ ] **Step 5: Build a session out of a snapshot**

In `crates/wwt/src/session.rs`:

```rust
    /// A session with one tab that already has a page. Tests and nothing
    /// else, now that every real tab comes into being through an effect.
    pub fn new(grid: GridSize, cell: CellSize) -> Self { ... }

    /// The tabs a restart should come back to, plus whatever was asked for
    /// on the command line.
    ///
    /// The snapshot is data from disk and is not trusted: an empty tab list
    /// and a focus index past the end both have to produce a browser you can
    /// use, because the alternative is a crash on launch that you cannot get
    /// past without finding the file yourself.
    pub fn restore(
        grid: GridSize,
        cell: CellSize,
        snapshot: Option<Snapshot>,
        open: Option<String>,
    ) -> Self {
        let mut session = Self {
            grid,
            cell,
            vp: page_viewport(grid, cell),
            mode: Mode::Normal,
            tabs: Vec::new(),
            focus: 0,
            next_id: 0,
        };

        let saved = snapshot.map(|s| (s.focus, s.tabs)).unwrap_or((0, Vec::new()));
        for restored in saved.1 {
            let id = session.mint();
            let mut tab = Tab::new(id, restored.url);
            tab.title = restored.title;
            tab.scroll_y = restored.scroll_y;
            tab.navigating = true;
            session.tabs.push(tab);
        }
        session.focus = saved.0.min(session.tabs.len().saturating_sub(1));

        if let Some(url) = open {
            let id = session.mint();
            let mut tab = Tab::new(id, url);
            tab.navigating = true;
            session.tabs.push(tab);
            session.focus = session.tabs.len() - 1;
        }

        if session.tabs.is_empty() {
            let id = session.mint();
            let mut tab = Tab::new(id, "about:blank".to_string());
            tab.navigating = true;
            session.tabs.push(tab);
        }
        session
    }

    /// The first thing a browser does: ask for the pages it does not have,
    /// and read the ones it does.
    pub fn begin(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        let wanted: Vec<(TabId, String, bool)> = self
            .tabs
            .iter()
            .map(|tab| (tab.id, tab.url.clone(), tab.opened))
            .collect();
        for (id, url, opened) in wanted {
            if opened {
                self.start_extract(id, &mut effects);
            } else {
                effects.push(Effect::OpenTab { id, url });
            }
        }
        effects
    }
```

`Session::new` keeps its body from Task 5 with `opened: true` on the tab it makes, so every existing test is unaffected.

- [ ] **Step 6: Rewire the binary**

In `crates/wwt/src/core.rs`:

```rust
/// What the browser starts as.
pub struct Startup {
    pub grid: GridSize,
    pub cell: CellSize,
    pub snapshot: Option<Snapshot>,
    /// A URL from the command line, opened beside whatever was restored.
    pub open: Option<String>,
    /// Where the session file goes, or `None` when this instance does not
    /// own it.
    pub session_file: Option<PathBuf>,
}

impl Core {
    pub fn new(client: Arc<Client>, startup: Startup) -> Self {
        let (jobs_tx, jobs_rx) = mpsc::unbounded_channel();
        let input = InputPump::spawn(jobs_tx.clone());
        let session = Session::restore(startup.grid, startup.cell, startup.snapshot, startup.open);

        Self {
            pages: HashMap::new(),
            opening: HashMap::new(),
            client,
            renderer: Renderer::new(),
            session,
            jobs_tx,
            jobs_rx,
            input,
            session_file: startup.session_file,
            pending: None,
        }
    }
```

In `crates/wwt/src/main.rs`:

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let (new_session, argument) = parse_args()?;
    let url = match argument {
        Some(argument) => Some(normalize_url(&argument).map_err(|m| anyhow::anyhow!(m))?),
        None => None,
    };

    let (grid, cell) = wwt_term::probe().context("measure the terminal")?;

    // Everything that can fail loudly happens before we touch the terminal,
    // so a failure leaves the user's screen exactly as it was.
    //
    // The profile is the lock. Chromium refuses a user-data-dir another
    // Chromium holds, so a second wwt needs no lock file of ours to go stale
    // after a crash: it gets a temporary profile, is told so, and writes no
    // session file. The instance holding the profile owns that file.
    let profile = wwt::store::profile_path();
    let (browser, private) = match profile.as_deref() {
        Some(path) => match Chromium::launch(Some(path)).await {
            Ok(browser) => (browser, false),
            Err(_) => (
                Chromium::launch(None).await.context("launch chromium")?,
                true,
            ),
        },
        None => (Chromium::launch(None).await.context("launch chromium")?, true),
    };

    let client = Arc::new(
        Client::connect(browser.ws_url())
            .await
            .context("connect to chromium")?,
    );
    client.auto_attach().await.context("watch for tabs the page opens")?;

    let session_file = (!private).then(wwt::store::session_path).flatten();
    let (snapshot, session_error) = match (&session_file, new_session) {
        (Some(path), false) => match wwt::store::load(path) {
            Ok(snapshot) => (snapshot, None),
            Err(message) => (None, Some(message)),
        },
        _ => (None, None),
    };

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, cursor::Hide)?;
    let mouse = execute!(stdout(), EnableMouseCapture).is_ok();

    let mut core = Core::new(
        client,
        Startup { grid, cell, snapshot, open: url, session_file },
    );
    if private {
        core.notice("private session: another wwt has the profile");
    } else if let Some(message) = session_error {
        core.notice(&format!("session file: {message}"));
    }
    if !mouse {
        core.notice("mouse unavailable");
    }
    // ... the rest is unchanged ...
}

/// `wwt`, `wwt <url>`, `wwt --new [url]`. Hand-rolled, because the whole
/// surface is one flag.
fn parse_args() -> Result<(bool, Option<String>)> {
    let mut new_session = false;
    let mut url = None;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--new" => new_session = true,
            "-h" | "--help" => bail!("usage: wwt [--new] [url]"),
            other if other.starts_with('-') => bail!("unknown option: {other}"),
            other => url = Some(other.to_string()),
        }
    }
    Ok((new_session, url))
}
```

The notices are set in that order deliberately: only the last one survives, and a private session is the one you most need to know about, so it is set first and overwritten only by something more recent. If you would rather all three were visible, that is a chrome change and belongs in its own task, not here.

`main.rs` no longer calls `Page::open`. Every tab, including the first, comes into being through `Effect::OpenTab`.

- [ ] **Step 7: Run the tests**

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 8: Try it, which is the whole point**

```bash
cargo run -p wwt -- example.com
```

Open two more tabs, scroll one of them, quit with `q`. Then:

```bash
cargo run -p wwt
```

The same three tabs, the same focus, the same scroll offset. Check the file:

```bash
cat "${XDG_DATA_HOME:-$HOME/.local/share}/wwt/session.json"
```

Then log in to something, quit, relaunch, and confirm you are still logged in. That is the milestone.

Finally, with the first still running, launch a second in another terminal: it should say `private session`, and quitting it must leave the first's session file untouched.

- [ ] **Step 9: Commit**

```bash
git add crates/wwt/src
git commit -m "feat(wwt): come back to the tabs and the logins you left

wwt restores; wwt <url> restores and opens the url beside what was there, so
nothing is lost by typing wwt example.com out of habit; wwt --new starts one
clean.

Every restored tab loads, scrolls back to its offset and is read once, so the
bar has real titles and the first switch to any tab is a repaint. Idling
means after that first read.

The profile is the lock and the instance holding it owns the session file. A
second wwt gets a temporary profile, says so, and writes nothing.

The snapshot is data from disk and is not trusted: an empty tab list, a focus
index past the end and a malformed file all have to produce a browser you can
use, because the alternative is a crash you cannot get past without finding
the file yourself."
```

---

### Task 14: The measurement, the glossary, and the manual pass

M4's latency claim is that a tab switch is a repaint and not a round trip. Like extraction and hints before it, that claim belongs in a test rather than in anybody's head. The vocabulary gained four words and the working notes gained three rules; both files say so.

**Files:**
- Modify: `crates/wwt/src/session.rs` (the measurement)
- Modify: `CONTEXT.md`
- Modify: `CLAUDE.md`
- Modify: `README.md`

- [ ] **Step 1: Write the measurement**

In `crates/wwt/src/session.rs`, in `mod tests`. It needs no browser, which is the point: the whole claim is that nothing leaves the process.

```rust
    /// What a tab switch costs. Run with:
    ///
    ///     cargo test -p wwt --lib measure_switch -- --nocapture
    ///
    /// The claim in spec section 3 is that a switch is a repaint and no round
    /// trip, so this asserts the absence of an extraction as well as printing
    /// the time. A page's worth of runs is composed and the frame is built,
    /// which is everything between pressing `J` and having the text.
    #[test]
    fn measure_switch() {
        let mut session = ready();
        open_two_more(&mut session);
        for tab in 0..3 {
            session.tabs[tab].runs = (0..300).map(|i| run(&format!("line {i}"))).collect();
            session.tabs[tab].read = true;
            session.tabs[tab].dirty = false;
        }

        let mut worst = std::time::Duration::ZERO;
        for _ in 0..200 {
            let start = std::time::Instant::now();
            let effects = session.on(key('J'));
            let frame = session.compose();
            worst = worst.max(start.elapsed());

            assert!(
                !effects.iter().any(|e| matches!(e, Effect::Extract(_))),
                "a clean tab must not be re-read: a switch is a repaint"
            );
            std::hint::black_box(frame);
        }
        eprintln!("switch, worst of 200: {worst:?}");
        assert!(worst < std::time::Duration::from_millis(5), "switch took {worst:?}");
    }
```

Run it and record the number in the commit message. Composing 300 runs into a grid is ~40µs, so a switch should land far under a millisecond; the assertion is loose on purpose, because it runs on whatever machine CI has.

- [ ] **Step 2: Update the glossary**

In `CONTEXT.md`, add to "What the browser is doing", after **Mode**:

```markdown
**Tab** — one page, and everything true of it rather than of the browser:
its URL, title, runs, caret, scroll offset, and what we have asked it for.
Identified by a **tab id**, a counter that never reuses a value, because a
page operation outlives the state that asked for it and an index would let a
closed tab's answer land on the tab that took its place. `wwt::tab::Tab`.

**Focus** — which tab you are looking at. The only tab that receives keys,
clicks and hint queries, and the only one painted. Switching **activates**
the target as well, because `Input.dispatchMouseEvent` is answered by
whichever target the browser has in front.

**Idle** — what a background tab is, precisely: it is read once when it
opens, so its title is real and the first switch to it is a repaint, and
after that it re-extracts only while focused. A dirty signal for a background
tab sets a flag that is spent when focus arrives.

**Snapshot** — the open tabs on their way to or from disk: a URL, a title and
a scroll offset each, plus which one was in front. Not called a session,
because `Session` already names the state machine and `wwt-cdp` already calls
an attached target a session id. `wwt::store::Snapshot`.
```

And in "The browser we drive":

```markdown
**Profile** — Chromium's `--user-data-dir`, persistent at
`$XDG_DATA_HOME/wwt/profile`. The cookie jar that makes logins durable, and
the lock: Chromium refuses one another Chromium holds, so a second wwt gets a
temporary profile and writes no session file. The instance holding the
profile owns that file.

**Adoption** — taking over a target a page opened for itself. Auto-attach
holds it before its first document runs, which is the only moment the
bootstrap can be installed into the document it actually loads.
```

- [ ] **Step 3: Update the working notes**

In `CLAUDE.md`, change the milestone line to M4 and add a section after "Input":

```markdown
## Tabs and sessions

`Session` holds `Vec<Tab>` and a focus index; `Tab` (`wwt/src/tab.rs`) holds
everything true of one page rather than of the browser. `Core` holds
`HashMap<TabId, Arc<Page>>`. Four rules carry M4:

- **A `TabId` is a counter and never a position.** Effects name a tab and
  jobs name it back, and a job whose tab is gone is dropped. Close a tab
  while its extraction is in flight and every later tab shifts down one; an
  index would let the answer land on a page that never asked.
- **A background tab keeps its runs.** A switch paints from the cache and
  only then re-extracts, so it is a repaint rather than a round trip.
  `measure_switch` holds that down. It also keeps its dirty flag rather than
  spending it: a tab is read once when it opens and thereafter only while
  focused, so an idle background tab costs what an idle foreground tab costs.
- **Switching activates.** `Input.dispatchMouseEvent` is answered by
  whichever target the browser has in front. With one target that was a
  test-harness quirk; with several it is a correctness rule, and M5's
  screencast will want the same guarantee.
- **A `Page` never reaches `Session`.** The loop's result channel carries a
  `Finished`, which is either a `Job` on its way through or a target that
  finished opening; the page is filed in `Core` and the session hears only
  that the tab opened.

The chrome is two rows, the tab bar on top and the statusline at the bottom,
both unconditional so opening a tab never reflows a page. The page therefore
does not start at frame row 0, and that shift lives in `Viewport` as an
origin row rather than as a `+1` in `paint_run`, `Caret::cell` and
`page_cell`. `to_cell(to_css(c)) == c` now holds at every origin too.

**The profile is the lock.** Chromium refuses a `--user-data-dir` another
Chromium holds, so a second `wwt` falls back to a temporary profile, says
`private session`, and writes no session file. The instance holding the
profile owns that file: one rule for both resources and no lock file of ours
to go stale after a crash.

**Deciding to save is a rule, writing is machinery.** `Session` emits
`Effect::Save` when the tab set, the focus, or a page's URL, title or scroll
offset changes; `Core` coalesces on a timer and writes temp-then-rename. An
extraction of a page that did not move is not a write.

Eviction of background targets past a limit, and the lazy restore that shares
its machinery, are deferred to M7. They introduce the one state this design
does not have, a tab that exists without a target.
```

Also add to the Commands block:

```
    cargo test -p wwt --lib measure_switch -- --nocapture      # tab switch latency
```

- [ ] **Step 4: Update the README**

Add the four new keys to whatever key table `README.md` carries, and the two paths under a short "Files" note:

```markdown
| `J` / `K` | next / previous tab |
| `t` | open a tab |
| `x` | close this tab |

wwt keeps a Chromium profile at `$XDG_DATA_HOME/wwt/profile` and the tabs you
had open at `$XDG_DATA_HOME/wwt/session.json`. A second wwt cannot have the
profile, so it runs private: not logged in, and it writes no session file.
```

- [ ] **Step 5: The manual pass**

Not automatable, and the milestone is not done without it. Work through it on a real terminal and fix what it finds:

1. `wwt` with no session file: one blank tab, no error.
2. `wwt example.com`: one tab, readable, the bar shows it.
3. `t`, a URL, Enter: a second tab, focused, and the bar marks it.
4. `J` and `K` around three tabs: instant, and each keeps its scroll offset.
5. Scroll a background tab's page from the page itself (a `setInterval` fixture, or a page that lazy-loads) and switch to it: it repaints with what was cached, then updates.
6. `f` on one tab, `J` before the labels arrive: no labels appear over the other tab.
7. A `target=_blank` link: a new tab, focused, with the page in it.
8. `x` down to the last tab, then `x` again: it quits.
9. Resize the terminal with three tabs open, then switch: every tab is laid out for the terminal you have now.
10. Quit and relaunch: same tabs, same focus, same scroll offsets.
11. Log in to something, quit, relaunch: still logged in.
12. Two terminals: the second says `private session`, and quitting it leaves the first's file alone.
13. Corrupt `session.json` by hand and launch: a notice and a usable browser, never a crash.
14. `wwt --new`: one blank tab, and the old session still on disk until this one quits.

- [ ] **Step 6: Run everything one last time**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p wwt --lib measure_switch -- --nocapture
cargo test -p wwt-page --test extraction measure_extraction -- --nocapture
cargo test -p wwt-page --test interaction measure_scroll_latency -- --nocapture
```

The last two are M2's and M3's numbers. M4 should not have moved them; if either has, find out why before calling the milestone done.

- [ ] **Step 7: Commit**

```bash
git add CONTEXT.md CLAUDE.md README.md crates/wwt/src/session.rs
git commit -m "docs: write down what a tab is and what a switch costs

The measurement is in the tests rather than in anybody's head, like
extraction's and hints' before it: a switch is a repaint and no round trip,
and measure_switch asserts the absence of the extraction as well as printing
the time."
```

---

## Done when

- `cargo test --workspace` passes and `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- The fourteen manual checks in Task 14 all pass on a real terminal.
- `measure_switch` prints a number well under a millisecond, and M2's and M3's measurements are unmoved.
- The spec's section 10 amendments are in `2026-08-19-wwt-design.md`, which Task 0 of this plan does not do because the spec commit already did it. If any implementation forces a deviation from `2026-08-21-wwt-m4-design.md`, amend that spec in the same commit as the code, per `CLAUDE.md`.
