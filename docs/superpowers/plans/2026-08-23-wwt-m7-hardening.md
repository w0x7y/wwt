# wwt M7 — Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make wwt survive a dead Chromium, a wedged page and a session of thirty tabs, by giving a tab the ability to exist without a target.

**Architecture:** Eviction, lazy restore and the restart path all want one new state: a tab that is real to the person using it and has no Chromium target behind it. Task 1 widens `Tab::opened` into a three-state `Presence`; task 4 makes `Detached` reachable; tasks 5, 6 and 7 are three pointings of that one mechanism. Deadlines and the config file are independent and land early because later tasks read them.

**Tech Stack:** Rust 2024, tokio, `toml` (added by this milestone), the hand-rolled `wwt-cdp` client, no other new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-23-wwt-m7-design.md`. The parent design is `docs/superpowers/specs/2026-08-19-wwt-design.md`; where the two disagree the parent wins, except for the amendments in M7 section 10, which change the parent and are applied in task 8.

## Global Constraints

- **Only one new dependency, and it is `toml`.** Everything else in `[workspace.dependencies]` is fixed. If a task seems to need another crate, stop and ask.
- **`wwt-frame` has no I/O and no dependencies.** Non-negotiable. M7 does not touch it except for the measurement in task 8.
- **`wwt-ui` depends on `wwt-frame` only.** No pages, no CDP, no terminal, no file reading. The search template is passed in as a parameter for this reason.
- **Unit tests in `src/` must run without Chromium.** Anything needing a browser goes in `tests/`.
- **`cargo clippy --workspace --all-targets -- -D warnings` must be clean per task**, not per plan.
- **Never blank the frame you are looking at** (parent spec §8). Every failure path here degrades to stale-but-labeled.
- **Nothing blocks the loop.** Page and browser operations spawn and report back as a `Job` or a `Finished`.
- **Nothing in a `select!` arm touches `self`.** An arm produces an `Incoming` and nothing else.
- **No em-dashes** in prose, comments or commit messages.
- Comments explain *why*, in prose, where the reason is not obvious.
- Test names are sentences describing the property.
- Commits are conventional with a crate scope: `feat(wwt):`, `refactor(cdp):`, `test(page):`.
- Commit messages end with:
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  ```

## File structure

| File | Responsibility after M7 |
|---|---|
| `crates/wwt/src/tab.rs` | `Presence`, and `Tab::detach()` as the one place that says what a tab keeps and loses |
| `crates/wwt/src/config.rs` | **New.** Parse `config.toml` into a `Config`; defaults and validation live here |
| `crates/wwt/src/store.rs` | Gains `config_dir`/`config_path` beside the existing data paths |
| `crates/wwt/src/effect.rs` | Gains `Effect::Detach`, `Effect::Relaunch` |
| `crates/wwt/src/event.rs` | Gains `Failure`, `Event::BrowserLost`, `Event::BrowserBack`, `Job::Relaunched` |
| `crates/wwt/src/session.rs` | The rules: detach, evict, lazy restore, the stalled rule, the relaunch rule |
| `crates/wwt/src/core.rs` | Owns the `Chromium`, the guarded CDP arm, the relaunch task, `Finished::Relaunched` |
| `crates/wwt-cdp/src/client.rs` | Typed `Timeout`, `call_with`/`call_on_with`, two deadline constants |
| `crates/wwt-cdp/src/launch.rs` | `find_chromium` and `launch` take a configured binary path |
| `crates/wwt-ui/src/command.rs` | `parse` and `normalize_url` take the search template |
| `crates/wwt-page/src/extract.rs` | Navigation calls name the long deadline |

---

### Task 1: `Presence` replaces `opened`

A pure widening with no behaviour change. `Detached` is defined here and produced by nothing until task 4, which is what keeps that task small.

**Files:**
- Modify: `crates/wwt/src/tab.rs`
- Modify: `crates/wwt/src/session.rs` (lines 119, 359-362, 633, 1078 today)
- Test: `crates/wwt/src/tab.rs` (unit tests at the bottom)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub enum Presence { Opening, Attached, Detached }` (derives `Debug, Clone, Copy, PartialEq, Eq`); `Tab::presence: Presence`; `Tab::attached(&self) -> bool`.

- [x] **Step 1: Write the failing test**

Add to the `mod tests` block at the bottom of `crates/wwt/src/tab.rs`:

```rust
    #[test]
    fn a_tab_that_has_only_been_asked_for_has_no_target_yet() {
        let tab = Tab::new(TabId(0), "https://example.com".to_string());
        assert_eq!(tab.presence, Presence::Opening);
        assert!(
            !tab.attached(),
            "an effect naming this tab would be dropped, so none may be emitted for it"
        );
    }

    #[test]
    fn only_an_attached_tab_can_be_asked_for_anything() {
        // The two states without a target are not interchangeable: one has
        // an answer coming and one is waiting to be focused. Both answer no
        // to the only question `Core` asks.
        let mut tab = Tab::new(TabId(0), String::new());
        tab.presence = Presence::Attached;
        assert!(tab.attached());
        tab.presence = Presence::Detached;
        assert!(!tab.attached());
    }
```

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p wwt --lib tab::`
Expected: FAIL, `cannot find type Presence in this scope`.

- [x] **Step 3: Add the enum and the field**

In `crates/wwt/src/tab.rs`, above `pub struct Tab`:

```rust
/// Whether a target exists for this tab, and if not, whether one is coming.
///
/// `Core` drops every effect naming a tab it holds no page for, so this is
/// the question to ask before emitting one or setting an in-flight flag
/// beside it. It used to be a bool called `opened`, and a bool cannot carry
/// the difference that matters: `Opening` has an answer in flight and
/// `Detached` has nothing, so focusing the first should wait and focusing
/// the second should ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// A target was asked for and `Job::Opened` is coming.
    Opening,
    /// A target exists. The only state in which an effect naming this tab
    /// is not dropped.
    Attached,
    /// No target, and none is coming until this tab is focused. Evicted,
    /// restored but not yet reached, or left behind by a dead browser.
    Detached,
}
```

Replace the `opened` field and its doc comment with:

```rust
    /// Whether this tab has a target behind it. See `Presence`.
    pub presence: Presence,
```

In `Tab::new`, replace `opened: false,` with `presence: Presence::Opening,`.

Add to `impl Tab`, beside `mark_dirty`:

```rust
    /// Whether an effect naming this tab would reach a page.
    pub fn attached(&self) -> bool {
        self.presence == Presence::Attached
    }
```

- [x] **Step 4: Fix every site that read the old field**

In `crates/wwt/src/session.rs`:

- Line ~119, in `Session::new`: `tab.opened = true;` becomes `tab.presence = Presence::Attached;`
- Line ~359, in `begin`: the tuple becomes `(TabId, String, f64, bool)` built with `tab.attached()` instead of `tab.opened`, and the loop binding is renamed `attached`:
  ```rust
        let wanted: Vec<(TabId, String, f64, bool)> = self
            .tabs
            .iter()
            .map(|tab| (tab.id, tab.url.clone(), tab.scroll_y, tab.attached()))
            .collect();
        for (id, url, scroll_y, attached) in wanted {
            if attached {
                self.start_extract(id, &mut effects);
            } else {
                effects.push(Effect::OpenTab { id, url, scroll_y });
            }
        }
  ```
- Line ~633, the `f` guard: `self.focused().opened` becomes `self.focused().attached()`.
- Line ~1078, in `Job::Opened(_, Ok(()))`: `tab.opened = true;` becomes `tab.presence = Presence::Attached;`

Add `Presence` to the `use crate::tab::...` import at the top of `session.rs`.

- [x] **Step 5: Run the workspace to verify nothing moved**

Run: `cargo test -p wwt --lib`
Expected: PASS. This task changes no behaviour, so every existing test must still pass untouched. If one needed editing, the widening was not faithful.

- [x] **Step 6: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/wwt/src/tab.rs crates/wwt/src/session.rs
git commit -m "$(cat <<'EOF'
refactor(wwt): a tab's target is three states, not a bool

`opened` answered one question, whether an effect naming this tab would
reach a page. M7 needs a second: whether one is coming. A tab waiting on
`Job::Opened` should be left alone and a tab with no target at all should
be asked for one, and a bool cannot tell them apart.

`Detached` is defined here and produced by nothing yet, so this commit
changes no behaviour and edits no existing test.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `config.toml`

Three keys, a parser asserted on with data, and the three call sites that read them. Independent of every other task, and first because tasks 5 and 7 read `max_tabs`.

**Files:**
- Modify: `Cargo.toml` (workspace deps), `crates/wwt/Cargo.toml`
- Create: `crates/wwt/src/config.rs`
- Modify: `crates/wwt/src/lib.rs` (declare the module)
- Modify: `crates/wwt/src/store.rs` (config path resolution)
- Modify: `crates/wwt-ui/src/command.rs` (`parse` and `normalize_url` take the template)
- Modify: `crates/wwt-cdp/src/launch.rs` (`find_chromium` and `launch` take a configured path)
- Modify: `crates/wwt/src/session.rs` (hold the template, pass it to `parse`)
- Modify: `crates/wwt/src/core.rs` (`Startup` carries the config), `crates/wwt/src/main.rs`

**Interfaces:**
- Consumes: nothing from task 1.
- Produces:
  - `wwt::config::Config { max_tabs: usize, search: String, chromium: Option<PathBuf> }`
  - `wwt::config::Config::default() -> Config` (`max_tabs: 8`, `search: "https://duckduckgo.com/?q={}"`, `chromium: None`)
  - `wwt::config::parse(text: &str) -> (Config, Vec<String>)`, the config and every complaint about it
  - `wwt::config::load(path: Option<&Path>) -> (Config, Vec<String>)`
  - `wwt::store::config_path() -> Option<PathBuf>`
  - `wwt_ui::command::normalize_url(raw: &str, search: &str) -> Result<String, String>`
  - `wwt_ui::command::parse(line: &str, search: &str) -> Result<Command, String>`
  - `wwt_cdp::find_chromium(configured: Option<&Path>) -> Result<PathBuf>`
  - `wwt_cdp::Chromium::launch(profile: Option<&Path>, binary: Option<&Path>) -> Result<Chromium>`
  - `Session::configure(&mut self, config: &Config)`

- [x] **Step 1: Add the dependency**

In the workspace `Cargo.toml`, under `[workspace.dependencies]`, after `serde_json`:

```toml
toml = "0.9"
```

In `crates/wwt/Cargo.toml`, under `[dependencies]`:

```toml
toml = { workspace = true }
```

Run `cargo check -p wwt`. If `0.9` does not resolve, use the highest `0.x` that does and say so in the commit body.

- [x] **Step 2: Write the failing parser tests**

Create `crates/wwt/src/config.rs` containing only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_the_defaults_and_no_complaints() {
        let (config, complaints) = parse("");
        assert_eq!(config, Config::default());
        assert!(complaints.is_empty());
    }

    #[test]
    fn every_key_is_read() {
        let (config, complaints) = parse(
            r#"
            max_tabs = 3
            search = "https://example.com/find?q={}"
            chromium = "/opt/chromium/chrome"
            "#,
        );
        assert!(complaints.is_empty(), "{complaints:?}");
        assert_eq!(config.max_tabs, 3);
        assert_eq!(config.search, "https://example.com/find?q={}");
        assert_eq!(config.chromium.as_deref(), Some(Path::new("/opt/chromium/chrome")));
    }

    #[test]
    fn a_file_that_is_not_toml_keeps_the_defaults_and_says_so() {
        // A browser that will not start because of a typo in a config file
        // is worse than one that starts and tells you.
        let (config, complaints) = parse("max_tabs = = 3");
        assert_eq!(config, Config::default());
        assert_eq!(complaints.len(), 1);
    }

    #[test]
    fn an_unknown_key_is_a_complaint_and_not_a_refusal() {
        let (config, complaints) = parse("colour_scheme = \"dark\"\nmax_tabs = 2");
        assert_eq!(config.max_tabs, 2, "the keys we do know still apply");
        assert_eq!(complaints.len(), 1);
        assert!(complaints[0].contains("colour_scheme"));
    }

    #[test]
    fn a_search_without_a_placeholder_cannot_take_a_query() {
        // Appending would be the friendly guess and it is the wrong one: a
        // template with the query in the middle is exactly why this is a
        // template rather than a prefix.
        let (config, complaints) = parse("search = \"https://example.com/find\"");
        assert_eq!(config.search, Config::default().search);
        assert_eq!(complaints.len(), 1);
        assert!(complaints[0].contains("{}"));
    }

    #[test]
    fn no_tabs_at_all_is_not_a_limit_anybody_can_use() {
        let (config, complaints) = parse("max_tabs = 0");
        assert_eq!(config.max_tabs, Config::default().max_tabs);
        assert_eq!(complaints.len(), 1);
    }

    #[test]
    fn a_wrongly_typed_value_leaves_the_rest_of_the_file_working() {
        let (config, complaints) = parse("max_tabs = \"eight\"\nsearch = \"https://e.com/?q={}\"");
        assert_eq!(config.max_tabs, Config::default().max_tabs);
        assert_eq!(config.search, "https://e.com/?q={}");
        assert_eq!(complaints.len(), 1);
    }

    #[test]
    fn a_missing_file_is_a_first_run_and_not_a_problem() {
        let (config, complaints) = load(Some(Path::new("/nonexistent/wwt/config.toml")));
        assert_eq!(config, Config::default());
        assert!(complaints.is_empty());
    }
}
```

- [x] **Step 3: Run to verify it fails**

Add `pub mod config;` to `crates/wwt/src/lib.rs`.
Run: `cargo test -p wwt --lib config::`
Expected: FAIL, `cannot find function parse in this scope`.

- [x] **Step 4: Implement the parser**

Above the test module in `crates/wwt/src/config.rs`:

```rust
//! What wwt reads out of `config.toml`, and what it does when it cannot.
//!
//! Three keys, and no more until something needs a fourth: a configuration
//! file fills up with settings nobody asked for unless it is defended.
//!
//! Nothing here can fail. A file that is not TOML, a key we do not know, a
//! value of the wrong type and a value out of range all produce the default
//! and a complaint, because a browser that will not start because of a typo
//! is worse than one that starts and tells you. The complaints become a
//! statusline notice, which is what the session file already does.

use std::path::{Path, PathBuf};

use toml::Value;

/// Where anything that is not a URL goes. DuckDuckGo because its html and
/// lite endpoints are the whole page in the markup and it wants no account,
/// which is what a browser like this one needs.
const DEFAULT_SEARCH: &str = "https://duckduckgo.com/?q=";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// How many live targets to hold, the focused one included. Targets and
    /// not tabs: a tab is cheap and a target is a browser process's worth
    /// of memory, and the tab bar goes on showing all of them.
    pub max_tabs: usize,
    /// A URL with `{}` where the percent-encoded query goes.
    pub search: String,
    /// Which browser to launch. `WWT_CHROMIUM` still wins over it, because
    /// a variable is set for one run and a file is written for all of them.
    pub chromium: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_tabs: 8,
            search: format!("{DEFAULT_SEARCH}{{}}"),
            chromium: None,
        }
    }
}

/// Read the file, or the defaults when there is not one.
///
/// A missing file is the normal case and says nothing. Anything else it
/// cannot make sense of, it says out loud and carries on without.
pub fn load(path: Option<&Path>) -> (Config, Vec<String>) {
    let Some(path) = path else {
        return (Config::default(), Vec::new());
    };
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (Config::default(), Vec::new())
        }
        Err(error) => (
            Config::default(),
            vec![format!("{}: {error}", path.display())],
        ),
    }
}

/// The arithmetic of `load`, with the file passed in, so every case is a
/// test with no filesystem in it.
pub fn parse(text: &str) -> (Config, Vec<String>) {
    let mut config = Config::default();
    let mut complaints = Vec::new();

    let table = match text.parse::<toml::Table>() {
        Ok(table) => table,
        Err(error) => {
            // One complaint for the whole file: a parse error names one
            // position and everything after it is unread.
            return (config, vec![format!("config.toml: {error}")]);
        }
    };

    for (key, value) in table {
        match key.as_str() {
            "max_tabs" => match value.as_integer() {
                // At least one, or the focused tab is over the limit and
                // eviction has nothing it is allowed to take.
                Some(n) if n >= 1 => config.max_tabs = n as usize,
                Some(n) => complaints.push(format!("max_tabs must be at least 1, not {n}")),
                None => complaints.push("max_tabs must be a number".to_string()),
            },
            "search" => match value.as_str() {
                Some(template) if template.contains("{}") => config.search = template.to_string(),
                Some(_) => complaints
                    .push("search needs {} where the query goes".to_string()),
                None => complaints.push("search must be a string".to_string()),
            },
            "chromium" => match value.as_str() {
                Some(path) => config.chromium = Some(PathBuf::from(path)),
                None => complaints.push("chromium must be a path".to_string()),
            },
            other => complaints.push(format!("unknown setting: {other}")),
        }
    }

    (config, complaints)
}
```

Add `use std::path::Path;` to the test module's imports if the compiler asks.

- [x] **Step 5: Run to verify it passes**

Run: `cargo test -p wwt --lib config::`
Expected: PASS, 8 tests.

- [x] **Step 6: Add the config path to the store**

In `crates/wwt/src/store.rs`, beside `data_dir` and `data_dir_from`:

```rust
/// Our directory under the user's config home, or `None` when there is no
/// home to put it in.
pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// The arithmetic of `config_dir`, with the environment passed in. The
/// same shape as `data_dir_from` and for the same reason: environment
/// variables are process global and tests run in threads.
fn config_dir_from(xdg: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(xdg) = xdg.filter(|value| !value.is_empty()) {
        return Some(Path::new(xdg).join("wwt"));
    }
    let home = home.filter(|value| !value.is_empty())?;
    Some(Path::new(home).join(".config/wwt"))
}

/// The settings file, which is the user's to write and ours only to read.
pub fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("config.toml"))
}
```

And a test beside the existing `data_dir_from` tests:

```rust
    #[test]
    fn the_config_lives_under_the_config_home_and_not_the_data_home() {
        assert_eq!(
            config_dir_from(Some(OsStr::new("/x/config")), None),
            Some(PathBuf::from("/x/config/wwt"))
        );
        assert_eq!(
            config_dir_from(None, Some(OsStr::new("/home/a"))),
            Some(PathBuf::from("/home/a/.config/wwt"))
        );
        // An empty variable is unset, per the XDG basedir spec.
        assert_eq!(
            config_dir_from(Some(OsStr::new("")), Some(OsStr::new("/home/a"))),
            Some(PathBuf::from("/home/a/.config/wwt"))
        );
    }
```

- [x] **Step 7: Take the search template as a parameter in `wwt-ui`**

In `crates/wwt-ui/src/command.rs`:

- Delete the `SEARCH` constant and its doc comment.
- Change `pub fn normalize_url(raw: &str) -> Result<String, String>` to
  `pub fn normalize_url(raw: &str, search: &str) -> Result<String, String>`, and its last line to:
  ```rust
      Ok(search.replacen("{}", &as_query(raw), 1))
  ```
  Add to its doc comment:
  ```rust
  /// The search template is passed in rather than read: `wwt-ui` depends on
  /// `wwt-frame` alone, and that rule is not being spent on a string.
  ```
- Change `pub fn parse(line: &str) -> Result<Command, String>` to take `search: &str` and pass it to both `normalize_url` calls.
- Update every existing test in that file to pass `"https://duckduckgo.com/?q={}"`, and add:
  ```rust
      #[test]
      fn a_search_goes_wherever_the_template_says() {
          assert_eq!(
              normalize_url("two words", "https://example.com/find?q={}&ie=utf8"),
              Ok("https://example.com/find?q=two+words&ie=utf8".to_string()),
              "the query goes where the placeholder is, not on the end"
          );
      }
  ```

- [x] **Step 8: Take the binary path as a parameter in `wwt-cdp`**

In `crates/wwt-cdp/src/launch.rs`:

- `pub fn find_chromium(configured: Option<&std::path::Path>) -> Result<PathBuf>`. After the `WWT_CHROMIUM` block and before the `PATH` search:
  ```rust
      // The environment wins over the file because it is the more specific
      // thing: a variable is set for one run and a file is written for all
      // of them.
      if let Some(path) = configured {
          if !path.is_file() {
              bail!("config.toml names {}, which is not a file", path.display());
          }
          return Ok(path.to_path_buf());
      }
  ```
- `pub async fn launch(profile: Option<&std::path::Path>, binary: Option<&std::path::Path>)`, whose first line becomes `let binary = find_chromium(binary)?;`.
- Update the four call sites in `crates/wwt-cdp/tests/browser.rs`, and the ones in `crates/wwt-page/tests/common/mod.rs`, `crates/wwt/tests/smoke.rs` and `crates/wwt/tests/input.rs`, to pass `None` as the second argument.

- [x] **Step 9: Wire it through the session and main**

In `crates/wwt/src/session.rs`:
- Add a field `search: String` to `Session`, initialised in `empty` with `Config::default().search`.
- Add beside `set_graphics`:
  ```rust
      /// Take what the config file said. Called once at startup, like
      /// `set_graphics`, and for the same reason: it is not a thing that
      /// changes while the browser is running.
      pub fn configure(&mut self, config: &crate::config::Config) {
          self.search = config.search.clone();
          self.max_tabs = config.max_tabs;
      }
  ```
  Add `max_tabs: usize` to `Session` too, defaulted from `Config::default().max_tabs`; task 5 is what reads it.
- At line ~679, `command::parse(&line)` becomes `command::parse(&line, &self.search)`.

In `crates/wwt/src/core.rs`, add `pub config: crate::config::Config` to `Startup`, and in `Core::new` call `session.configure(&startup.config)` beside `session.set_graphics(...)`.

In `crates/wwt/src/main.rs`:
- After `parse_args`, before the terminal probe:
  ```rust
      let (config, complaints) = wwt::config::load(wwt::store::config_path().as_deref());
  ```
- The command-line URL becomes `normalize_url(&argument, &config.search)`.
- Both `Chromium::launch(Some(path))` calls become `Chromium::launch(Some(path), config.chromium.as_deref())`, and both `Chromium::launch(None)` calls become `Chromium::launch(None, config.chromium.as_deref())`.
- Pass `config: config.clone()` in `Startup`.
- After the existing notices, and before them in priority since a config problem is the least urgent of the three:
  ```rust
      // First, so anything more urgent overwrites it: the statusline holds
      // one notice and a config typo matters less than a session you cannot
      // save.
      if let Some(complaint) = complaints.first() {
          core.notice(&format!("config.toml: {complaint}"));
      }
  ```
  Place this block *above* the `if !mouse` block.

- [x] **Step 10: Run everything**

Run: `cargo test --workspace`
Expected: PASS. The browser tests launch Chromium; they must still pass with the new `launch` signature.

- [x] **Step 11: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add -A
git commit -m "$(cat <<'EOF'
feat(wwt): three settings, in a file the user owns

M7 needs a tab limit that is not a constant, and two things that have
been constants for want of anywhere to put them: where a search goes,
and which browser to launch. `normalize_url`'s own doc comment said
"making this a setting is a configuration question, and there is still
no configuration". Now there is.

The template is passed into `wwt-ui` rather than read there, because
that crate depends on `wwt-frame` alone and the rule is not being spent
on a string. `WWT_CHROMIUM` still wins over the file: a variable is set
for one run and a file is written for all of them.

Nothing in here can fail. A file that is not TOML, an unknown key, a
wrong type and a value out of range each produce the default and a
complaint in the statusline, which is the treatment the session file
already gets.

Adds the `toml` crate, against the standing rule in CLAUDE.md, asked
and answered.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Deadlines, a typed timeout, and `Stalled`

**Files:**
- Modify: `crates/wwt-cdp/src/client.rs`, `crates/wwt-cdp/src/lib.rs`
- Modify: `crates/wwt-page/src/extract.rs` (navigation calls)
- Modify: `crates/wwt/src/event.rs`, `crates/wwt/src/core.rs`, `crates/wwt/src/session.rs`
- Test: `crates/wwt-cdp/src/client.rs` (unit), `crates/wwt/src/session.rs` (unit)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `wwt_cdp::TimedOut` (a unit struct implementing `std::error::Error` via `thiserror`)
  - `wwt_cdp::DEADLINE`, `wwt_cdp::NAVIGATION_DEADLINE` (`Duration`)
  - `Client::call_with(&self, method: &str, params: Value, deadline: Duration)`
  - `Client::call_on_with(&self, session_id: &str, method: &str, params: Value, deadline: Duration)`
  - `wwt::event::Failure { TimedOut, Failed(String) }`, with `Failure::from_error(&anyhow::Error) -> Failure`
  - `Job::Extracted(TabId, Source, Result<Box<Extraction>, Failure>)`, `Job::Status(TabId, Result<Status, Failure>)`, `Job::Hints(TabId, Result<Vec<HintTarget>, Failure>)`, `Job::Failed(TabId, Failure)`

- [x] **Step 1: Write the failing timeout test**

In the `mod tests` block of `crates/wwt-cdp/src/client.rs`:

```rust
    #[tokio::test(start_paused = true)]
    async fn a_call_that_is_never_answered_produces_a_timeout_and_not_a_string() {
        // The whole of the stalled rule rests on this being tellable apart
        // from a page whose script threw.
        let (pending, subscribers) = parts();
        let (tx, _rx) = mpsc::unbounded_channel();
        let client = Client {
            next_id: AtomicU64::new(1),
            outgoing: tx,
            pending,
            subscribers,
            user_agent: OnceLock::new(),
        };

        let error = client
            .call_with("Runtime.evaluate", json!({}), Duration::from_secs(5))
            .await
            .expect_err("nothing ever answers this");
        assert!(
            error.downcast_ref::<TimedOut>().is_some(),
            "a deadline is a kind of failure, not a message about one"
        );
    }
```

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p wwt-cdp --lib`
Expected: FAIL, `cannot find type TimedOut`.

- [x] **Step 3: Implement the deadlines**

In `crates/wwt-cdp/src/client.rs`, replace the `CALL_TIMEOUT` constant and its comment with:

```rust
/// What a command gets when it is our own script being asked a question.
///
/// An extraction measures ~4ms, a status read under 1ms, and the worst
/// `DOMSnapshot` of `heavy.html` ~26ms, so this is two hundred times the
/// slowest thing ever measured here. It was a flat thirty seconds, which
/// meant a wedged page swallowed a keystroke for half a minute before
/// anything on screen said so.
pub const DEADLINE: Duration = Duration::from_secs(5);

/// What a command gets when the answer is somebody else's network.
///
/// A real page on a bad connection legitimately takes this long, and the
/// thing being waited for is not our main thread.
pub const NAVIGATION_DEADLINE: Duration = Duration::from_secs(30);

/// A command that was never answered, as a type rather than a message.
///
/// `Session` treats a deadline differently from a script that threw: one
/// means our extractor cannot read this page and the other means the page
/// is not running. The difference has to survive the trip through
/// `anyhow`, so it is carried by a type that can be downcast to.
#[derive(Debug, thiserror::Error)]
#[error("{method} was not answered within {deadline:?}")]
pub struct TimedOut {
    pub method: String,
    pub deadline: Duration,
}
```

Change `send` to take a deadline, and add the two public wrappers:

```rust
    /// Send a command to the browser target.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.send(method, params, None, DEADLINE).await
    }

    /// The same, with a deadline of its own. See `NAVIGATION_DEADLINE`.
    pub async fn call_with(&self, method: &str, params: Value, deadline: Duration) -> Result<Value> {
        self.send(method, params, None, deadline).await
    }

    /// Send a command to an attached session (a page).
    pub async fn call_on(&self, session_id: &str, method: &str, params: Value) -> Result<Value> {
        self.send(method, params, Some(session_id), DEADLINE).await
    }

    /// The same, with a deadline of its own.
    pub async fn call_on_with(
        &self,
        session_id: &str,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value> {
        self.send(method, params, Some(session_id), deadline).await
    }

    async fn send(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
        deadline: Duration,
    ) -> Result<Value> {
```

and inside it, the timeout arm becomes:

```rust
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(TimedOut { method: method.to_string(), deadline }.into());
            }
```

with `timeout(CALL_TIMEOUT, rx)` becoming `timeout(deadline, rx)`.

Export from `crates/wwt-cdp/src/lib.rs`:

```rust
pub use client::{Client, DEADLINE, Event, NAVIGATION_DEADLINE, TimedOut};
```

(keeping whatever else that line already exports).

- [x] **Step 4: Give navigation the long deadline**

In `crates/wwt-page/src/extract.rs`, the three calls that start a navigation take `NAVIGATION_DEADLINE`:

- in `navigate`: `Page.navigate`
- in `back` and `forward`: `Page.navigateToHistoryEntry`
- in `reload`: `Page.reload`

Each becomes `self.client.call_on_with(&self.session_id, "...", json!({...}), wwt_cdp::NAVIGATION_DEADLINE)`.

Leave `LOAD_TIMEOUT` exactly as it is: it is the wait for `Page.loadEventFired` and already has its own thirty seconds.

- [x] **Step 5: Run to verify the timeout test passes**

Run: `cargo test -p wwt-cdp --lib`
Expected: PASS.

- [x] **Step 6: Write the failing session tests**

In `crates/wwt/src/session.rs` tests:

```rust
    #[test]
    fn a_read_that_timed_out_stalls_the_tab_and_does_not_degrade_it() {
        // A script that threw is a page our extractor cannot read, and the
        // snapshot is a different extractor that might. A page that did not
        // answer in five seconds has no main thread running, and the
        // snapshot needs the same one: asking would cost a second deadline
        // to learn the same thing, and would mark the tab degraded for the
        // rest of its life over a wedge that may last a second.
        let mut session = ready_session();
        let id = session.focused_id();
        let effects = session.on(Event::Done(Job::Extracted(
            id,
            Source::Script,
            Err(Failure::TimedOut),
        )));

        assert_eq!(*session.state(), State::Stalled);
        assert!(
            !session.focused().degraded,
            "a deadline is not a broken script"
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::Extract(_, Source::Snapshot))),
            "there is nothing to ask a page that is not running"
        );
        assert!(!session.focused().reading, "the read is over either way");
    }

    #[test]
    fn a_script_that_threw_still_reaches_for_the_snapshot() {
        // M6's rule, unchanged. This is the test that proves the exemption
        // above is an exemption and not a replacement.
        let mut session = ready_session();
        let id = session.focused_id();
        let effects = session.on(Event::Done(Job::Extracted(
            id,
            Source::Script,
            Err(Failure::Failed("__wwt is not defined".to_string())),
        )));

        assert!(session.focused().degraded);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Extract(_, Source::Snapshot)))
        );
    }

    #[test]
    fn a_page_that_comes_back_clears_the_stall_by_itself() {
        // Nothing schedules a retry: a page wedged in a loop cannot run its
        // own MutationObserver, so it sends no dirty signal and nothing
        // re-asks. A page that recovers sends one and is read normally.
        let mut session = ready_session();
        let id = session.focused_id();
        session.on(Event::Done(Job::Extracted(id, Source::Script, Err(Failure::TimedOut))));
        assert_eq!(*session.state(), State::Stalled);

        let effects = session.on(Event::Dirty(id));
        assert!(
            effects.iter().any(|e| matches!(e, Effect::Extract(_, Source::Script))),
            "the fast path, because a timeout never degraded it"
        );
        session.on(read(id, Source::Script, "https://example.com"));
        assert_eq!(*session.state(), State::Ready);
    }
```

Use whichever existing helper the file already has for a session with one attached, read tab; the tests near `fn read(...)` at line ~2177 show the established shape. If no `ready_session` helper exists, use the one those tests use and rename these accordingly.

- [x] **Step 7: Run to verify they fail**

Run: `cargo test -p wwt --lib session::tests::a_read_that_timed_out`
Expected: FAIL, `cannot find type Failure`.

- [x] **Step 8: Implement `Failure` and the rule**

In `crates/wwt/src/event.rs`:

```rust
/// Why something did not work, in the only two kinds `Session` treats
/// differently.
///
/// `Core` reports what happened and the session decides what it means,
/// which is the seam M6 drew when the effect started naming its source
/// rather than the page carrying a flag. A string cannot carry the
/// distinction, and every rule about degrading depends on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The command was never answered. The page is not running.
    TimedOut,
    /// It was answered, with a refusal.
    Failed(String),
}

impl Failure {
    /// What an error from a page operation was.
    pub fn from_error(error: &anyhow::Error) -> Self {
        if error.downcast_ref::<wwt_cdp::TimedOut>().is_some() {
            return Failure::TimedOut;
        }
        Failure::Failed(error.to_string())
    }

    /// What to put in the statusline. A timeout says `[stalled]` instead,
    /// so this is only ever reached by the other kind.
    pub fn message(&self) -> String {
        match self {
            Failure::TimedOut => "the page did not answer".to_string(),
            Failure::Failed(message) => message.clone(),
        }
    }
}
```

Change the four `Job` variants to carry `Failure` instead of `String`: `Extracted`, `Status`, `Hints`, `Failed`.

In `crates/wwt/src/core.rs`, every `.map_err(|error| error.to_string())` feeding one of those four becomes `.map_err(|error| Failure::from_error(&error))`, and the `Effect::Scroll` and `Effect::Navigate` arms' `Job::Failed(id, e.to_string())` become `Job::Failed(id, Failure::from_error(&e))`. `Job::Noted`, `Job::Opened` and `Job::Unsaved` keep their strings: none of them is a read, and none decides whether to degrade.

In `crates/wwt/src/session.rs`, `Job::Extracted`'s error arm:

```rust
                    Err(failure) => {
                        let tab = self.tab_mut(id).expect("resolved above");
                        tab.reading = false;
                        match (source, &failure) {
                            // A deadline is not a broken script. The page
                            // is not running, and `DOMSnapshot` needs the
                            // same main thread our script does, so asking
                            // it costs a second deadline to learn the same
                            // thing and leaves the tab degraded for good
                            // over a wedge that may last a second.
                            (_, Failure::TimedOut) => tab.state = State::Stalled,
                            // The script broke. Read it the other way,
                            // once, and go on reading it that way until it
                            // navigates.
                            (Source::Script, _) => {
                                tab.degraded = true;
                                tab.dirty = true;
                                self.start_extract(id, effects);
                            }
                            // There is no third source. The frame you are
                            // looking at stands and only the statusline
                            // changes.
                            (Source::Snapshot, _) => tab.state = State::Error(failure.message()),
                        }
                        return;
                    }
```

`Job::Status`'s error arm takes the same shape: `Failure::TimedOut` sets `State::Stalled` and returns; anything else degrades as it does today.

`Job::Hints`'s error arm uses `failure.message()`. `Job::Failed`'s arm sets `State::Stalled` on `Failure::TimedOut` and `State::Error(message)` otherwise.

- [x] **Step 9: Run to verify everything passes**

Run: `cargo test --workspace`
Expected: PASS.

- [x] **Step 10: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add -A
git commit -m "$(cat <<'EOF'
feat(cdp): a deadline is a kind of failure, not a message about one

Commands had a flat thirty second deadline, so a page wedged in a
JavaScript loop swallowed a keystroke for half a minute with nothing on
screen to say why. Two classes now: thirty seconds for a navigation,
because the thing being waited for is somebody else's network, and five
for everything else, which is two hundred times the slowest thing ever
measured here.

The timeout is a type so it survives the trip through anyhow, and
`Failure` carries the distinction to the session, which is what makes
the rule below expressible at all: a timed-out read sets `Stalled` and
does not degrade. A script that threw is a page our extractor cannot
read and the snapshot is a different extractor that might; a page that
did not answer has no main thread, and the snapshot needs the same one.

`State::Stalled` has been in the statusline since M1 with nothing that
set it. This is what sets it.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Detach, and reattach on focus

The spine. Nothing here evicts anything yet; this task builds the mechanism and one way to reach it.

**Files:**
- Modify: `crates/wwt/src/tab.rs` (`Tab::detach`)
- Modify: `crates/wwt/src/effect.rs` (`Effect::Detach`)
- Modify: `crates/wwt/src/session.rs` (`detach`, and `focus_tab` reattaching)
- Modify: `crates/wwt/src/core.rs` (the `Detach` arm)
- Test: `crates/wwt/src/tab.rs`, `crates/wwt/src/session.rs`

**Interfaces:**
- Consumes: `Presence` from task 1.
- Produces: `Tab::detach(&mut self)`; `Effect::Detach(TabId)`; `Session::detach(&mut self, id: TabId, effects: &mut Vec<Effect>)`.

- [x] **Step 1: Write the failing tab test**

In `crates/wwt/src/tab.rs` tests:

```rust
    #[test]
    fn a_detached_tab_keeps_what_it_looked_like_and_loses_what_it_was_waiting_for() {
        let mut tab = Tab::new(TabId(0), "https://example.com".to_string());
        tab.presence = Presence::Attached;
        tab.title = "Example".to_string();
        tab.scroll_y = 400.0;
        tab.runs = vec![TextRun::default()];
        tab.read = true;
        tab.degraded = true;
        tab.reading = true;
        tab.navigating = true;
        tab.hinting = true;
        tab.hints = Some(Vec::new());

        tab.detach();

        assert_eq!(tab.presence, Presence::Detached);
        // What it looked like, which is what makes switching back a repaint.
        assert_eq!(tab.title, "Example");
        assert_eq!(tab.scroll_y, 400.0);
        assert_eq!(tab.runs.len(), 1);
        // Answers that are never coming. `Core` drops every effect naming a
        // tab with no page, so a flag left set here is a flag nothing will
        // ever clear.
        assert!(!tab.reading);
        assert!(!tab.navigating);
        assert!(!tab.hinting);
        // Geometry belonging to a document that is about to stop existing.
        assert_eq!(tab.hints, None);
        // A new document reinstalls bootstrap.js, so the tab has earned
        // another attempt at the fast path. The same reason navigation
        // clears it.
        assert!(!tab.degraded);
        assert!(tab.dirty, "nothing about the old document is authoritative");
    }
```

If `TextRun` has no `Default`, build one the way the existing session tests build theirs.

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p wwt --lib tab::`
Expected: FAIL, `no method named detach`.

- [x] **Step 3: Implement `Tab::detach`**

In `crates/wwt/src/tab.rs`, beside `mark_dirty`:

```rust
    /// Let go of this tab's target, and keep the tab.
    ///
    /// The one place that says what a tab is without a browser behind it.
    /// Eviction detaches one tab, a dead Chromium detaches all of them, and
    /// a restored tab starts this way, so getting the list wrong here is
    /// three bugs rather than one.
    pub fn detach(&mut self) {
        self.presence = Presence::Detached;
        // Every answer in flight is an answer that will not arrive: `Core`
        // holds no page for this tab any more. A flag left set is a flag
        // nothing can clear, which is `f` dead for the rest of the run.
        self.reading = false;
        self.navigating = false;
        self.hinting = false;
        // Geometry, belonging to a document that is about to stop existing.
        self.hints = None;
        // A reattached page is a new document with our script freshly in
        // it, so it has earned the fast path back. The same reason a
        // navigation clears this.
        self.degraded = false;
        // The runs stay, and are what a switch back paints first. They are
        // also no longer authoritative, which is what the flag says.
        self.dirty = true;
    }
```

- [x] **Step 4: Write the failing session tests**

```rust
    #[test]
    fn a_detached_tab_is_asked_for_again_when_you_switch_to_it() {
        let mut session = two_ready_tabs();
        let away = session.tabs[0].id;
        let mut effects = Vec::new();
        session.detach(away, &mut effects);
        assert!(effects.contains(&Effect::Detach(away)));

        session.tabs[0].scroll_y = 900.0;
        let effects = session.on(key(KeyCode::Char('1'), KeyModifiers::ALT));

        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::OpenTab { id, scroll_y, .. } if *id == away && *scroll_y == 900.0
            )),
            "a reattach is an open, and an open carries the offset: as two \
             effects they are two tasks, and an extraction that wins that \
             race reads offset zero and saves it"
        );
        assert_eq!(session.focused().presence, Presence::Opening);
    }

    #[test]
    fn nothing_is_asked_of_a_detached_tab_while_it_is_away() {
        // `Core` would drop the effect and the flag would never be cleared.
        // This is the rule `Tab::opened` named in M4, under its new name.
        let mut session = two_ready_tabs();
        let away = session.tabs[0].id;
        let mut effects = Vec::new();
        session.detach(away, &mut effects);

        let effects = session.on(Event::Dirty(away));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Extract(id, _) if *id == away)),
            "a detached tab has no page to read"
        );
        assert!(session.tabs[0].dirty, "the flag is kept and spent on reattach");
        assert!(!session.tabs[0].reading);
    }

    #[test]
    fn switching_to_a_detached_tab_paints_what_it_looked_like_first() {
        // M4's repaint guarantee survives eviction: the runs are still
        // here, so the switch is a repaint and the round trip happens
        // behind it.
        let mut session = two_ready_tabs();
        let away = session.tabs[0].id;
        let runs = session.tabs[0].runs.len();
        assert!(runs > 0, "the fixture must have read this tab");
        let mut effects = Vec::new();
        session.detach(away, &mut effects);

        session.on(key(KeyCode::Char('1'), KeyModifiers::ALT));
        assert_eq!(session.focused().runs.len(), runs);
        assert!(!session.compose().is_empty_page());
    }
```

Use the helpers the existing tab tests use (`two_ready_tabs` is the shape at line ~1718; reuse the real name). If `Frame` has no `is_empty_page`, assert on the composed frame the way the neighbouring M4 tests do.

- [x] **Step 5: Run to verify they fail**

Run: `cargo test -p wwt --lib session::tests::a_detached_tab`
Expected: FAIL, `no method named detach on Session`.

- [x] **Step 6: Implement the session half**

Add `Detach(TabId)` to `Effect` in `crates/wwt/src/effect.rs`:

```rust
    /// Let go of a tab's target and keep the tab. `CloseTab` without the
    /// tab going away: the URL, the title, the scroll offset and the runs
    /// are all still true, and the page is opened again when you come back.
    Detach(TabId),
```

In `crates/wwt/src/session.rs`:

```rust
    /// Give up a tab's target while keeping the tab.
    ///
    /// The one entry point, so eviction, a dead browser and a session
    /// restored from disk all leave a tab in the same state.
    fn detach(&mut self, id: TabId, effects: &mut Vec<Effect>) {
        let Some(tab) = self.tab_mut(id) else { return };
        if !tab.attached() {
            // Nothing to give up, and `Opening` must not be overwritten: its
            // `Job::Opened` is still coming and would arrive as a surprise.
            return;
        }
        tab.detach();
        effects.push(Effect::Detach(id));
        // Deliberately no save. The URL, the title and the offset are
        // exactly what they were, and section 7 of the parent spec says a
        // write happens when one of those changed.
    }

    /// Ask for the target a detached tab does not have.
    ///
    /// Reuses `Effect::OpenTab` rather than adding a reattach of its own: it
    /// already carries the scroll offset, and its `Job::Opened` already
    /// activates the tab, restarts the screencast and triggers the first
    /// read. A reattach is an open, and inherits every rule that holds for
    /// one.
    fn reattach(&mut self, id: TabId, effects: &mut Vec<Effect>) {
        let Some(tab) = self.tab_mut(id) else { return };
        if tab.presence != Presence::Detached {
            return;
        }
        tab.presence = Presence::Opening;
        tab.navigating = true;
        tab.state = State::Loading;
        let (url, scroll_y) = (tab.url.clone(), tab.scroll_y);
        effects.push(Effect::OpenTab { id, url, scroll_y });
    }
```

In `focus_tab`, after `self.focus = index;` and before `self.follow_focus(...)`:

```rust
        // A tab that was evicted, or left behind by a browser that died,
        // asks for its target back on the way in. Its runs are painted
        // first, so this is a round trip behind a repaint rather than
        // instead of one.
        self.reattach(id, effects);
```

and guard the two effects that need a page. `Effect::Activate(id)` and `start_extract` are both no-ops for a tab with no page, and `Core` would drop them, but emitting them is what teaches the next reader that it is fine. Wrap them:

```rust
        if self.focused().attached() {
            // The browser's foreground and ours have to be the same target,
            // or input lands on the page you just left.
            effects.push(Effect::Activate(id));
            // Spends the dirty flag this tab has been accumulating in the
            // background, and does nothing if it has none.
            self.start_extract(id, effects);
        }
```

In `start_extract`, after the `let Some(tab) = self.tab_mut(id) else { return };` line:

```rust
        // A tab with no target has no page to read. The dirty flag is kept
        // rather than spent, and the reattach is what spends it.
        if !tab.attached() {
            return;
        }
```

- [x] **Step 7: Implement the core half**

In `crates/wwt/src/core.rs`, beside the `Effect::CloseTab` arm:

```rust
                // The same as closing, minus the tab going away. Taken out
                // of the map first: whatever happens to the target, nothing
                // may still be sent to a page the session has let go of.
                Effect::Detach(id) => self.drop_page(id),

                Effect::CloseTab(id) => self.drop_page(id),
```

and the helper, beside `resize_page`:

```rust
    /// Let go of a page and close its target.
    ///
    /// Shared by closing a tab and detaching one, because to `Core` they are
    /// the same act: the difference between them is entirely the session's,
    /// which is where the tab lives.
    fn drop_page(&mut self, id: TabId) {
        let Some(page) = self.pages.remove(&id) else {
            return;
        };
        let tx = self.jobs_tx.clone();
        tokio::spawn(async move {
            if let Err(error) = page.close().await {
                let _ = tx.send(Finished::Job(Job::Noted(id, error.to_string())));
            }
        });
    }
```

- [x] **Step 8: Run to verify it passes**

Run: `cargo test -p wwt --lib`
Expected: PASS.

- [x] **Step 9: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add -A
git commit -m "$(cat <<'EOF'
feat(wwt): a tab can give up its target and keep being a tab

The state M7 is built on. Eviction, a dead browser and a session
restored from disk all want it, so it is written once, in `Tab::detach`,
where getting the list of what survives wrong is one bug rather than
three.

What survives is what the tab looked like: the URL, the title, the
offset and the runs, which is what makes switching back a repaint and
keeps M4's guarantee intact. What does not is every answer in flight,
because `Core` holds no page for this tab any more and a flag left set
is a flag nothing can clear.

The reattach reuses `Effect::OpenTab` rather than inventing one. It
already carries the scroll offset, for the reason M4 recorded, and its
`Job::Opened` already activates the tab, restarts the screencast and
triggers the first read. A reattach is an open.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Eviction

**Files:**
- Modify: `crates/wwt/src/tab.rs` (`focused_at`)
- Modify: `crates/wwt/src/session.rs` (the counter, `evict`)
- Test: `crates/wwt/src/session.rs`

**Interfaces:**
- Consumes: `Session::detach` (task 4), `Config::max_tabs` and `Session::max_tabs` (task 2).
- Produces: `Tab::focused_at: u64`; `Session::evict(&mut self, effects: &mut Vec<Effect>)`.

- [x] **Step 1: Write the failing tests**

```rust
    #[test]
    fn the_tab_you_looked_at_longest_ago_is_the_one_that_goes() {
        let mut session = four_ready_tabs();
        session.max_tabs = 3;
        let oldest = session.tabs[0].id;

        // Visit 1, 2, 3 in order, leaving tab 0 the least recently seen.
        session.on(key(KeyCode::Char('2'), KeyModifiers::ALT));
        session.on(key(KeyCode::Char('3'), KeyModifiers::ALT));
        let effects = session.on(key(KeyCode::Char('4'), KeyModifiers::ALT));

        assert!(effects.contains(&Effect::Detach(oldest)));
        assert_eq!(session.tabs[0].presence, Presence::Detached);
        assert_eq!(session.tabs.len(), 4, "an evicted tab is still a tab");
    }

    #[test]
    fn the_tab_you_are_looking_at_is_never_the_one_that_goes() {
        let mut session = four_ready_tabs();
        session.max_tabs = 1;
        let effects = session.on(key(KeyCode::Char('2'), KeyModifiers::ALT));
        let focused = session.focused_id();
        assert!(!effects.contains(&Effect::Detach(focused)));
        assert!(session.focused().attached() || session.focused().presence == Presence::Opening);
    }

    #[test]
    fn a_tab_with_an_answer_coming_is_left_alone() {
        // Its url still names where it is leaving, so reattaching later
        // would take you back to the page it navigated away from.
        let mut session = four_ready_tabs();
        session.max_tabs = 2;
        let busy = session.tabs[0].id;
        session.tab_mut(busy).expect("fixture").navigating = true;

        let effects = session.on(key(KeyCode::Char('4'), KeyModifiers::ALT));
        assert!(!effects.contains(&Effect::Detach(busy)));
    }

    #[test]
    fn nothing_eligible_means_nothing_evicted() {
        // The limit is a target and not a guarantee. The alternative is
        // racing an answer that is already on its way in order to honour a
        // number that exists to bound memory.
        let mut session = four_ready_tabs();
        session.max_tabs = 1;
        for tab in &mut session.tabs {
            tab.reading = true;
        }
        let effects = session.on(key(KeyCode::Char('2'), KeyModifiers::ALT));
        assert!(!effects.iter().any(|e| matches!(e, Effect::Detach(_))));
    }

    #[test]
    fn a_tab_already_away_does_not_count_against_the_limit() {
        // The limit counts live targets, which is what costs memory, and
        // not tabs, which are cheap and all of which the bar goes on
        // showing.
        let mut session = four_ready_tabs();
        session.max_tabs = 3;
        let mut effects = Vec::new();
        session.detach(session.tabs[0].id, &mut effects);

        let effects = session.on(key(KeyCode::Char('3'), KeyModifiers::ALT));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Detach(_))),
            "three attached tabs is not over a limit of three"
        );
    }
```

Add a `four_ready_tabs()` helper beside the existing fixtures, built the same way `two_ready_tabs` is, with four tabs all `Attached` and `read`.

- [x] **Step 2: Run to verify they fail**

Run: `cargo test -p wwt --lib session::tests::the_tab_you_looked_at_longest_ago`
Expected: FAIL, `no field focused_at`.

- [x] **Step 3: Implement**

In `crates/wwt/src/tab.rs`, add to `Tab`:

```rust
    /// When this tab was last focused, as a count and never a clock.
    ///
    /// A counter, so the recency rule is asserted with data and its tests
    /// need neither a browser nor time.
    pub focused_at: u64,
```

initialised to `0` in `Tab::new`.

In `crates/wwt/src/session.rs`, add `focus_counter: u64` to `Session` (0 in `empty`), and in `focus_tab`, immediately after `self.focus = index;`:

```rust
        self.focus_counter += 1;
        let counter = self.focus_counter;
        self.focused_mut().focused_at = counter;
```

Then, at the end of `focus_tab` after `self.save(effects)`:

```rust
        self.evict(effects);
```

And the rule itself:

```rust
    /// Hold no more live targets than the limit, by letting go of the tab
    /// you looked at longest ago.
    ///
    /// Eligible means attached, not focused, and with nothing in flight. A
    /// background tab mid-navigation has a url that still names where it is
    /// leaving, so detaching it and reattaching later would take you back
    /// to the page you navigated away from.
    ///
    /// If nothing is eligible, nothing is evicted: the limit is a target
    /// and not a guarantee. The alternative is racing an answer that is
    /// already on its way in order to honour a number whose whole purpose
    /// is to bound memory.
    fn evict(&mut self, effects: &mut Vec<Effect>) {
        let focused = self.focused_id();
        loop {
            let attached = self.tabs.iter().filter(|tab| tab.attached()).count();
            if attached <= self.max_tabs {
                return;
            }
            let oldest = self
                .tabs
                .iter()
                .filter(|tab| {
                    tab.attached()
                        && tab.id != focused
                        && !tab.reading
                        && !tab.navigating
                        && !tab.hinting
                })
                .min_by_key(|tab| tab.focused_at)
                .map(|tab| tab.id);
            let Some(id) = oldest else { return };
            self.detach(id, effects);
        }
    }
```

The loop rather than a single detach: lowering `max_tabs` between runs, or restoring a session wider than the limit, can leave several to give up at once, and one focus change should reach the limit rather than approach it.

- [x] **Step 4: Run to verify they pass**

Run: `cargo test -p wwt --lib`
Expected: PASS.

- [x] **Step 5: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add -A
git commit -m "$(cat <<'EOF'
feat(wwt): hold as many targets as the limit, not as many as you have tabs

The M4 deferral, discharged. A session of thirty tabs holds `max_tabs`
of them live and the rest detached, and the tab bar goes on showing all
thirty, because a tab is cheap and a target is not.

Least recently focused goes, by a counter and never a clock, so the rule
is asserted with data. A tab with work in flight is never taken: its url
still names where it is leaving, so reattaching later would take you
back to the page it navigated away from. If nothing is eligible nothing
is evicted, which makes the limit a target rather than a guarantee, and
that is the honest version: the alternative is racing an answer already
on its way in order to honour a number that exists to bound memory.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Lazy restore

**Files:**
- Modify: `crates/wwt/src/session.rs` (`restore`, `begin`)
- Test: `crates/wwt/src/session.rs`

**Interfaces:**
- Consumes: `Presence::Detached` (task 1), `reattach` (task 4).
- Produces: nothing new; `begin` changes what it emits.

- [x] **Step 1: Write the failing tests**

```rust
    #[test]
    fn a_restored_session_opens_the_tab_you_were_looking_at_and_no_others() {
        let snapshot = Snapshot {
            version: crate::store::VERSION,
            focus: 1,
            tabs: vec![
                saved("https://a.example", "A", 0.0),
                saved("https://b.example", "B", 250.0),
                saved("https://c.example", "C", 0.0),
            ],
        };
        let mut session = Session::restore(grid(), cell(), Some(snapshot), None);
        let effects = session.begin();

        let opens: Vec<_> = effects
            .iter()
            .filter_map(|e| match e {
                Effect::OpenTab { id, url, scroll_y } => Some((*id, url.clone(), *scroll_y)),
                _ => None,
            })
            .collect();
        assert_eq!(opens.len(), 1, "one page, however many tabs were open");
        assert_eq!(opens[0].1, "https://b.example");
        assert_eq!(opens[0].2, 250.0);
        assert_eq!(session.tabs[0].presence, Presence::Detached);
        assert_eq!(session.tabs[2].presence, Presence::Detached);
    }

    #[test]
    fn a_restored_tab_reads_as_itself_in_the_bar_before_it_has_a_page() {
        // Titles come from the file, so the bar is complete on the first
        // frame rather than a row of blanks that fills in.
        let snapshot = Snapshot {
            version: crate::store::VERSION,
            focus: 0,
            tabs: vec![saved("https://a.example", "Anemone", 0.0)],
        };
        let session = Session::restore(grid(), cell(), Some(snapshot), None);
        assert_eq!(session.tabs[0].title, "Anemone");
    }

    #[test]
    fn a_url_on_the_command_line_is_the_tab_that_opens() {
        let snapshot = Snapshot {
            version: crate::store::VERSION,
            focus: 0,
            tabs: vec![saved("https://a.example", "A", 0.0)],
        };
        let mut session = Session::restore(
            grid(),
            cell(),
            Some(snapshot),
            Some("https://new.example".to_string()),
        );
        let effects = session.begin();
        let opens: Vec<_> = effects
            .iter()
            .filter(|e| matches!(e, Effect::OpenTab { .. }))
            .collect();
        assert_eq!(opens.len(), 1);
        assert!(matches!(
            opens[0],
            Effect::OpenTab { url, .. } if url == "https://new.example"
        ));
    }
```

Add a `saved(url, title, scroll_y) -> SavedTab` helper if the file has none.

- [x] **Step 2: Run to verify they fail**

Run: `cargo test -p wwt --lib session::tests::a_restored_session_opens`
Expected: FAIL, three `OpenTab` effects where one was expected.

- [x] **Step 3: Implement**

In `Session::restore`, for each restored tab replace `tab.navigating = true;` with:

```rust
            // No target, and none asked for: the focused one is opened by
            // `begin` and the rest wait to be reached. Startup launches one
            // page rather than however many were open, and a tab you never
            // switch to costs nothing at all.
            tab.presence = Presence::Detached;
```

The tab created for a command-line URL keeps `navigating = true` and `Presence::Opening`, as does the `about:blank` fallback.

Replace the body of `begin` with:

```rust
    pub fn begin(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        // Only the tab in front. A restored tab is detached, and detached
        // tabs are opened when you reach them: the same machinery eviction
        // uses, pointed at startup. The tab bar is already complete, because
        // titles and urls came out of the session file.
        let id = self.focused_id();
        match self.focused().presence {
            Presence::Detached => self.reattach(id, &mut effects),
            // A tab the constructor already asked for: a command-line url,
            // or the `about:blank` a session with nothing in it gets.
            Presence::Opening => {
                let (url, scroll_y) = {
                    let tab = self.focused();
                    (tab.url.clone(), tab.scroll_y)
                };
                effects.push(Effect::OpenTab { id, url, scroll_y });
            }
            // `Session::new`, which tests use and nothing else does.
            Presence::Attached => self.start_extract(id, &mut effects),
        }
        effects
    }
```

- [x] **Step 4: Run to verify they pass**

Run: `cargo test --workspace`
Expected: PASS. `crates/wwt/tests/smoke.rs` exercises `begin` against a real browser; if a smoke test asserted that every restored tab opens, it is asserting the old behaviour and should be updated to assert the new one, with a comment saying why.

- [x] **Step 5: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add -A
git commit -m "$(cat <<'EOF'
feat(wwt): start one page, however many tabs you left open

Restore is lazy: the focused tab opens and the rest start detached, so a
session of thirty tabs launches one target instead of thirty. The bar is
complete on the first frame either way, because titles and urls have
been in the session file since M4.

The same machinery eviction uses, pointed at startup, which is the whole
argument for building detachment first.

A restored tab that has never been read looks like a new tab looks when
you reach it: loading, and an empty page area for one round trip. An
evicted tab paints its cached runs immediately. Both are right, and the
difference is real: one of them has been read and one has not.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: The supervisor

**Files:**
- Modify: `crates/wwt/src/main.rs` (hand the browser over)
- Modify: `crates/wwt/src/core.rs` (own it, guard the arm, relaunch)
- Modify: `crates/wwt/src/event.rs` (`Event::BrowserLost`, `Event::BrowserBack`, `Job::Relaunched`)
- Modify: `crates/wwt/src/effect.rs` (`Effect::Relaunch`)
- Modify: `crates/wwt/src/session.rs` (the rules)
- Test: `crates/wwt/src/session.rs`

**Interfaces:**
- Consumes: `Session::detach`, `Session::reattach` (task 4).
- Produces: `Effect::Relaunch`; `Event::BrowserLost`; `Event::BrowserBack`; `Job::Relaunched(Result<(), String>)`; `Startup::browser: Chromium`; `Startup::profile: Option<PathBuf>`.

- [x] **Step 1: Write the failing session tests**

```rust
    #[test]
    fn a_dead_browser_leaves_every_tab_where_it_was_and_asks_for_another() {
        let mut session = four_ready_tabs();
        session.tabs[0].scroll_y = 500.0;
        let effects = session.on(Event::BrowserLost);

        assert!(effects.contains(&Effect::Relaunch));
        assert!(
            session.tabs.iter().all(|tab| tab.presence == Presence::Detached),
            "there is no browser, so no tab has a target"
        );
        assert_eq!(session.tabs.len(), 4, "the tabs are what a restart comes back to");
        assert_eq!(session.tabs[0].scroll_y, 500.0);
        assert!(
            !session.focused().runs.is_empty(),
            "never blank the frame you are looking at"
        );
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Detach(_))),
            "there is nothing on the other end to close"
        );
    }

    #[test]
    fn a_browser_that_came_back_is_asked_for_one_page() {
        let mut session = four_ready_tabs();
        session.on(Event::BrowserLost);
        let effects = session.on(Event::BrowserBack);

        let opens: Vec<_> = effects
            .iter()
            .filter(|e| matches!(e, Effect::OpenTab { .. }))
            .collect();
        assert_eq!(opens.len(), 1, "the restart path is lazy restore");
        assert_eq!(session.focused().presence, Presence::Opening);
    }

    #[test]
    fn a_held_key_asks_for_one_relaunch_and_not_thirty() {
        let mut session = four_ready_tabs();
        session.on(Event::BrowserLost);
        session.on(Event::Done(Job::Relaunched(Err("no chromium".to_string()))));

        let first = session.on(key(KeyCode::Char('j'), KeyModifiers::NONE));
        assert!(first.contains(&Effect::Relaunch), "a keystroke is how you ask again");
        let second = session.on(key(KeyCode::Char('j'), KeyModifiers::NONE));
        assert!(
            !second.contains(&Effect::Relaunch),
            "one in flight is enough; the flag is cleared by its answer"
        );
        assert!(
            !first.iter().any(|e| matches!(e, Effect::Scroll(..))),
            "there is no page to scroll"
        );
    }

    #[test]
    fn a_relaunch_that_failed_leaves_the_frame_you_were_reading() {
        let mut session = four_ready_tabs();
        let runs = session.focused().runs.len();
        session.on(Event::BrowserLost);
        session.on(Event::Done(Job::Relaunched(Err("no chromium".to_string()))));

        assert_eq!(session.focused().runs.len(), runs);
        assert!(matches!(session.state(), State::Error(_) | State::Notice(_)));
    }
```

- [x] **Step 2: Run to verify they fail**

Run: `cargo test -p wwt --lib session::tests::a_dead_browser`
Expected: FAIL, no variant `BrowserLost`.

- [x] **Step 3: Implement the vocabulary**

`crates/wwt/src/effect.rs`:

```rust
    /// Start a browser to replace the one that died, and say how it went.
    ///
    /// `Session` decides that we try; `Core` decides how many times and how
    /// far apart, because a count and a delay are machinery.
    Relaunch,
```

`crates/wwt/src/event.rs`, in `Event`:

```rust
    /// The websocket closed. Every target died with it, so every tab has
    /// lost its page, and the frame on screen is the last true thing there
    /// is about them.
    BrowserLost,
    /// A replacement browser is connected and attached. No tab has a target
    /// yet: this is the moment to ask for the one in front.
    BrowserBack,
```

and in `Job`:

```rust
    /// A relaunch gave up. Only ever the failure: a browser that arrived is
    /// `Event::BrowserBack`, because `Core` has to file the browser and the
    /// client before the session can be told anything at all, exactly as
    /// `Finished::Opened` files a page before reporting `Job::Opened`.
    Relaunched(Result<(), String>),
```

Add `Job::Relaunched` to the `id` match at the top of `on_job` as a second no-tab case beside `Job::Unsaved`.

- [x] **Step 4: Implement the session rules**

Add to `Session`: `browser_lost: bool` and `relaunching: bool`, both `false` in `empty`.

In `Session::on`:

```rust
            Event::BrowserLost => self.on_browser_lost(&mut effects),
            Event::BrowserBack => self.on_browser_back(&mut effects),
```

```rust
    /// The browser died. Keep everything, ask for another.
    fn on_browser_lost(&mut self, effects: &mut Vec<Effect>) {
        self.browser_lost = true;
        // `Tab::detach` and not `Session::detach`: there is no target on the
        // other end to close, so emitting `Effect::Detach` would ask `Core`
        // to close pages whose websocket is already gone.
        for tab in &mut self.tabs {
            tab.detach();
        }
        self.focused_mut().state = State::Notice("browser gone, restarting".to_string());
        self.ask_for_a_browser(effects);
    }

    /// Ask for a browser, unless we already have.
    ///
    /// The fourth in-flight flag in this file, and it exists for the same
    /// reason as the other three: a held `j` after a failed relaunch would
    /// otherwise spawn a relaunch per repeat.
    fn ask_for_a_browser(&mut self, effects: &mut Vec<Effect>) {
        if self.relaunching {
            return;
        }
        self.relaunching = true;
        effects.push(Effect::Relaunch);
    }

    /// A browser arrived. Nothing has a target yet.
    fn on_browser_back(&mut self, effects: &mut Vec<Effect>) {
        self.browser_lost = false;
        self.relaunching = false;
        // Only the tab in front, because the restart path is lazy restore
        // arrived at from the other direction. A background tab pays for
        // its target when you reach it, which is M4's idling rule.
        let id = self.focused_id();
        self.reattach(id, effects);
    }
```

In `on_job`, the new variant:

```rust
            Job::Relaunched(result) => {
                self.relaunching = false;
                if let Err(message) = result {
                    // Stale frames and a label, never an exit. The tabs are
                    // already written down, so quitting is yours to choose.
                    self.focused_mut().state =
                        State::Error(format!("no browser: {message}. any key retries"));
                }
            }
```

In `on_key`, immediately after the mode dispatch resolves an `Action` and before it is interpreted, add the interception. Put it at the top of the function that turns an `Action` into effects:

```rust
        // With no browser there is nothing for most of these to act on, and
        // a keystroke is how you ask for one back. Deliberately not a timer:
        // an idle wwt costs ~zero CPU and that rule does not get an
        // exception for the state where there is nothing to be busy about.
        if self.browser_lost && action_touches_the_page(&action) {
            self.ask_for_a_browser(effects);
            return;
        }
```

and the predicate, beside it:

```rust
/// Whether an action would have reached a page.
///
/// Quitting, the `:` line and mode changes all still work with no browser:
/// they are ours, and taking them away would make a browser that lost its
/// Chromium unusable rather than merely empty.
fn action_touches_the_page(action: &Action) -> bool {
    matches!(
        action,
        Action::Scroll(_)
            | Action::ScrollTop
            | Action::ScrollEnd
            | Action::Back
            | Action::Forward
            | Action::Reload
            | Action::Hints
            | Action::Insert
            | Action::Send(_)
    )
}
```

- [x] **Step 5: Run to verify the session tests pass**

Run: `cargo test -p wwt --lib`
Expected: PASS.

- [x] **Step 6: Give `Core` the browser**

In `crates/wwt/src/core.rs`:

- Add to `Startup`:
  ```rust
      /// The browser itself, because the thing that restarts one has to hold
      /// it. It was `main`'s until M7.
      pub browser: Chromium,
      /// Which profile to relaunch onto, or `None` for a private session.
      pub profile: Option<PathBuf>,
  ```
- Add to `Core`: `browser: Option<Chromium>` (an `Option` so a relaunch can take it and drop it before launching a replacement), `profile: Option<PathBuf>`, and `chromium: Option<PathBuf>` set in `Core::new` from `startup.config.chromium.clone()`, so a relaunch launches the same binary the first launch did.
- Add the variant:
  ```rust
  enum Finished {
      Job(Job),
      Opened(TabId, Result<Arc<Page>, String>),
      /// A replacement browser, or the reason there is not one. It comes
      /// this way rather than as a `Job` for the reason a `Page` does: a
      /// `Chromium` and a `Client` are `Core`'s and must never reach the
      /// session.
      Relaunched(Result<(Chromium, Arc<Client>), String>),
  }
  ```
- The CDP arm becomes guarded, and reports the loss:
  ```rust
                  incoming = cdp.recv(), if !lost => match incoming {
                      // The websocket closed. A closed receiver answers
                      // `None` immediately and forever, so the arm has to be
                      // guarded off after this or the loop spins at one
                      // hundred percent CPU under a frozen page, which is a
                      // worse failure than the one being handled.
                      None => Some(Incoming::Event(Event::BrowserLost)),
                      Some(event) => match Client::opened_by_a_page(&event) {
                          ... the existing three questions, unchanged ...
                      },
                  },
  ```
  with `let mut lost = false;` declared beside `resize_at`, and set to `true` where the event is handled (see the next bullet).
- In the `match incoming` block in `run`, beside `Finished::Opened`:
  ```rust
                  Some(Incoming::Event(Event::BrowserLost)) => {
                      lost = true;
                      Some(Event::BrowserLost)
                  }
                  Some(Incoming::Finished(Finished::Relaunched(Ok((browser, client))))) => {
                      self.browser = Some(browser);
                      self.client = Arc::clone(&client);
                      // The subscription is a local, and it is the thing
                      // that has to be replaced: this is why a relaunch is
                      // handled here rather than in `apply`.
                      cdp = self.client.subscribe();
                      lost = false;
                      Some(Event::BrowserBack)
                  }
                  Some(Incoming::Finished(Finished::Relaunched(Err(error)))) => {
                      Some(Event::Done(Job::Relaunched(Err(error))))
                  }
  ```
  `cdp` must be declared `let mut cdp = ...` for this.
- The effect:
  ```rust
                  Effect::Relaunch => {
                      // Dropped before anything else happens. `Chromium` is
                      // kill_on_drop and the profile directory is the lock:
                      // Chromium refuses a user-data-dir another Chromium
                      // holds, so relaunching while our own dying browser
                      // still has it is the one failure this path would
                      // inflict on itself, and it would present as an
                      // inexplicable fall back to a private session.
                      drop(self.browser.take());
                      let profile = self.profile.clone();
                      let binary = self.chromium.clone();
                      let tx = self.jobs_tx.clone();
                      tokio::spawn(async move {
                          let _ = tx.send(Finished::Relaunched(
                              relaunch(profile.as_deref(), binary.as_deref()).await,
                          ));
                      });
                  }
  ```
  with `chromium: Option<PathBuf>` on `Core` taken from the config.
- And the free function, at the bottom of the file:
  ```rust
  /// How many times to try, and how long to wait between.
  ///
  /// `Chromium::launch` carries its own twenty second startup timeout, which
  /// dominates the worst case: a browser that starts and never announces an
  /// endpoint costs about a minute before wwt gives up. Accepted. The
  /// alternative is a second deadline over the first, and the case it would
  /// improve is a machine that is already not working.
  const RELAUNCH_BACKOFF: &[Duration] = &[
      Duration::from_millis(250),
      Duration::from_secs(1),
      Duration::from_secs(4),
  ];

  /// Start a browser to replace one that died, and connect to it.
  ///
  /// The whole of the retrying, because how many times and how far apart are
  /// machinery: the decision that we try at all is the session's.
  async fn relaunch(
      profile: Option<&std::path::Path>,
      binary: Option<&std::path::Path>,
  ) -> Result<(Chromium, Arc<Client>), String> {
      let mut last = "never attempted".to_string();
      for (attempt, wait) in RELAUNCH_BACKOFF.iter().enumerate() {
          if attempt > 0 {
              tokio::time::sleep(*wait).await;
          }
          match start(profile, binary).await {
              Ok(pair) => return Ok(pair),
              Err(error) => last = error.to_string(),
          }
      }
      Err(last)
  }

  /// One attempt: a browser, a connection, and the auto-attach that has to
  /// be on before the first target exists.
  ///
  /// Deliberately no fall back to a temporary profile. That fallback is a
  /// startup path: a relaunch that cannot have the profile is a failed
  /// attempt and backs off, because the alternative is silently continuing
  /// without the cookie jar that was the reason for holding one.
  async fn start(
      profile: Option<&std::path::Path>,
      binary: Option<&std::path::Path>,
  ) -> Result<(Chromium, Arc<Client>)> {
      let browser = Chromium::launch(profile, binary).await?;
      let client = Arc::new(Client::connect(browser.ws_url()).await?);
      client.auto_attach().await?;
      Ok((browser, client))
  }
  ```

- [x] **Step 7: Hand the browser over in `main`**

In `crates/wwt/src/main.rs`, pass `browser` and `profile` into `Startup` instead of holding `browser` as a local for the life of `main`. The `private` flag still decides the session file exactly as it does now, and `profile` passed to `Startup` is `None` when the launch fell back to a temporary one, so a relaunch of a private session gets a fresh temporary profile rather than trying to take the one another wwt holds.

- [x] **Step 8: Run the workspace**

Run: `cargo test --workspace`
Expected: PASS.

- [x] **Step 9: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add -A
git commit -m "$(cat <<'EOF'
feat(wwt): a browser that dies is replaced, and the tabs come back

Websocket close is the signal, as the spec has said since M1. Every tab
detaches, keeping its url, title, offset and runs, so the frame you were
reading stays up and only the statusline changes. The focused tab asks
for a target when the replacement arrives and the rest wait to be
reached: the restart path is lazy restore, arrived at from the other
direction.

Two details that are the whole of it working. The old `Chromium` is
dropped before the new one is launched, because kill_on_drop is what
releases the profile lock and Chromium refuses a user-data-dir another
Chromium holds. And the CDP arm is guarded off once it has answered
None, because a closed receiver answers None immediately and forever:
unguarded, the loop spins at one hundred percent under a frozen page,
which is worse than the failure being handled.

`Core` owns the browser now, because the thing that restarts one has to
hold it. Three attempts, then stale frames and a label, and the next
keystroke that would have touched the page asks again. Never a timer: an
idle wwt costs ~zero CPU, and that rule does not get an exception for
the state where there is nothing to be busy about.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: The measurements, the notes and the manual pass

**Files:**
- Modify: `crates/wwt/src/session.rs` (`measure_switch` gains a case)
- Create: `crates/wwt/tests/supervisor.rs`
- Modify: `crates/wwt-cdp/tests/browser.rs`
- Modify: `docs/superpowers/specs/2026-08-19-wwt-design.md`, `CONTEXT.md`, `CLAUDE.md`, `README.md`
- Modify: `docs/superpowers/plans/2026-08-23-wwt-m7-hardening.md` (tick the boxes)

- [x] **Step 1: Measure what a detached switch costs**

Extend `measure_switch` in `crates/wwt/src/session.rs` with a detached case, printing both numbers:

```rust
    // A switch to an evicted tab is still a repaint: the runs are cached
    // and the round trip happens behind them. The number that matters is
    // that it is the same order as an attached switch and not an
    // extraction, because that is M4's guarantee surviving M7.
```

Run: `cargo test -p wwt --lib measure_switch -- --nocapture` and record both numbers in the commit body.

- [x] **Step 2: Prove a deadline is typed, against a real browser**

Add to `crates/wwt-cdp/tests/browser.rs`:

```rust
/// The stalled rule rests on a timeout being tellable apart from a page
/// whose script threw, and a unit test can only prove that for a socket
/// nobody is listening to. This proves it for a real page that will not
/// answer.
#[tokio::test]
async fn a_page_that_will_not_answer_produces_a_timeout_and_not_a_refusal() {
    let browser = Chromium::launch(None, None).await.expect("launch chromium");
    let client = Client::connect(browser.ws_url()).await.expect("connect");
    // A short deadline against a script that never returns. `call_with` is
    // what makes the wait bearable in a test.
    let error = client
        .call_with(
            "Browser.getVersion",
            serde_json::json!({}),
            std::time::Duration::from_nanos(1),
        )
        .await
        .expect_err("a nanosecond is not enough for a round trip");
    assert!(error.downcast_ref::<wwt_cdp::TimedOut>().is_some());
}
```

- [x] **Step 3: Prove a killed browser is replaced by a working one**

`Core::run` cannot be driven from a test: it builds an `EventStream` over stdin and loops
until told to quit. So the browser test covers the half that needs a browser, and task 7's
session tests cover the half that is a rule. Between them every line of the path is
exercised.

Make `relaunch` `pub` in `crates/wwt/src/core.rs` (with a note on it saying it is public
so that `tests/supervisor.rs` can kill a browser and watch it come back), and create
`crates/wwt/tests/supervisor.rs`:

```rust
//! The half of the restart path that needs a real browser. The rules it
//! serves are unit tests in `session.rs`, which need none.

use wwt_cdp::Chromium;

/// A relaunch produces a browser that actually works: connected, attached,
/// and able to open a page. Asserting it returned `Ok` would pass on a
/// client whose websocket was already closed.
#[tokio::test]
async fn a_browser_killed_is_replaced_by_one_that_can_open_a_page() {
    let profile = tempfile::tempdir().expect("a profile to relaunch onto");

    let first = Chromium::launch(Some(profile.path()), None)
        .await
        .expect("launch chromium");
    // Dropped, which is what a relaunch does first: kill_on_drop is what
    // releases the profile lock, and Chromium refuses a user-data-dir
    // another Chromium holds.
    drop(first);

    let (browser, client) = wwt::core::relaunch(Some(profile.path()), None)
        .await
        .expect("a replacement browser");

    let vp = wwt::session::page_viewport(
        wwt_frame::GridSize { cols: 80, rows: 24 },
        wwt_frame::CellSize { w: 9, h: 20 },
    );
    let page = wwt_page::Page::open(client, "about:blank", vp)
        .await
        .expect("the replacement browser opens pages");
    page.extract().await.expect("and answers our script");
    drop(browser);
}

/// The profile is the lock, so a relaunch that begins before the old
/// browser is gone must fail rather than quietly land on a temporary
/// profile and lose the cookie jar.
#[tokio::test]
async fn a_relaunch_onto_a_held_profile_fails_rather_than_going_private() {
    let profile = tempfile::tempdir().expect("a profile");
    let held = Chromium::launch(Some(profile.path()), None)
        .await
        .expect("launch chromium");

    let result = wwt::core::relaunch(Some(profile.path()), None).await;
    assert!(
        result.is_err(),
        "a relaunch that cannot have the profile is a failed attempt, not a \
         private session: the cookie jar is the reason for holding one"
    );
    drop(held);
}
```

Add `tempfile = { workspace = true }` to `crates/wwt/Cargo.toml` under `[dev-dependencies]` if it is not already there.

Note the second test costs the full backoff (about five seconds) before it fails, which is
the price of asserting the behaviour rather than the shape.

- [x] **Step 4: Write the amendments into the parent spec**

In `docs/superpowers/specs/2026-08-19-wwt-design.md`, section 8:

- **"Chromium dies."** Note that the restart rebuilds from live session state rather than the session file, because the file is up to `SAVE_DEBOUNCE` behind and rebuilding from it discards up to a second of navigation at the moment least worth discarding it. The file remains what a cold start reads.
- **"Page hangs."** Two deadline classes, 30s for navigation and 5s for everything else, a typed timeout, and the rule that a timed-out read does not fall back to `DOMSnapshot`.
- **"Too many tabs."** Strike the deferral. "Configurable" is `max_tabs` in `config.toml`, and the limit is a target rather than a guarantee.
- Section 7: restore is lazy, and the sentence about a tab being read once when it opens now says something slightly different, because a lazily restored tab has not opened yet.
- Wherever the spec says the whole configuration surface is one flag and two environment variables, replace it with a pointer to `config.toml`.

- [x] **Step 5: Update the glossary**

In `CONTEXT.md`, under "What the browser is doing", add:

```markdown
**Presence** — whether a tab has a target behind it, and if not, whether one
is coming: `Opening`, `Attached`, `Detached`. `Core` drops every effect
naming a tab that is not attached, so it is the question to ask before
emitting one.

**Detached** — a tab with no target, which is still a tab: its url, title,
scroll offset and runs are all true, and the page is opened again when you
reach it. Three things produce one: eviction, a browser that died, and a
session restored from disk.

**Eviction** — letting go of the target of the tab you looked at longest
ago, once more than `max_tabs` are live. The limit counts targets and not
tabs, and is a target rather than a guarantee: a tab with work in flight is
never taken.
```

Under "The browser we drive":

```markdown
**Relaunch** — replacing a Chromium that died: drop the old one first,
because it holds the profile lock, then three attempts with backoff. What
survives is what a tab was; what does not is form contents and per-tab
history.

**Failure** — why something did not work, in the only two kinds the session
treats differently: `TimedOut`, meaning the page is not running, and
`Failed`, meaning it answered with a refusal. The first sets `Stalled`; only
the second degrades a tab.
```

Under "The screen", beside `State`, note that `Stalled` is what a timed-out read produces.

- [x] **Step 6: Update the working notes**

In `CLAUDE.md`:

- Change "Currently at **M6** (degradation)" to **M7** (hardening).
- Add `config.toml` and its three keys beside the `WWT_CHROMIUM` paragraph in **Commands**.
- Add the new test commands:
  ```
      cargo test -p wwt --lib measure_switch -- --nocapture        # attached and detached
      cargo test -p wwt --test supervisor -- --nocapture           # a browser killed and replaced
  ```
- Add a **Hardening** section after **Degradation**, saying in the house voice:
  - A tab can exist without a target, and three features are that one state pointed differently.
  - `Tab::detach` is the one place that says what survives; getting its list wrong is three bugs.
  - A reattach is `Effect::OpenTab`, which already carries the offset and already restarts the screencast, so it inherits every rule an open has. This is now the fifth place the picture follows the focus.
  - The old browser is dropped before the new one launches, because it holds the profile lock.
  - The CDP arm is guarded off after it answers `None`, or the loop spins at one hundred percent under a frozen page.
  - A timed-out read sets `Stalled` and does not degrade: the snapshot needs the same main thread the script does.
  - A stalled tab needs no retry policy, because a wedged page cannot run the observer that would ask again.
  - The limit is a target and not a guarantee.
  - Relaunching is asked for by a keystroke and never by a timer, because an idle wwt costs ~zero CPU.
  - `toml` is the one dependency added since the set was fixed, and it was asked for.
- In **Crates**, note `wwt`'s new `config.rs`.

- [x] **Step 7: Update the README**

Document `config.toml`: where it lives, the three keys, that a missing file is normal, and that a bad one is a notice rather than a refusal. Mention that a session of many tabs holds `max_tabs` live pages.

- [x] **Step 8: The manual pass**

Run `cargo run -p wwt -- example.com` in a real terminal and confirm, noting anything surprising:

1. Open five tabs with `:tabopen`, set `max_tabs = 2` in the config, restart, and switch around: the bar keeps five tabs, switching to an evicted one paints instantly and then loads.
2. `pkill -f 'remote-debugging-port'` while wwt is in front: the frame stays, the statusline says the browser is gone, and a moment later the page is back where it was.
3. Do the same with `WWT_CHROMIUM` pointed at `/bin/false`: the relaunch fails, the frame stays, a key retries.
4. Quit, restart: the tabs come back and only one page loads.
5. Load a page running `while(true){}` in a script and press `j`: `[stalled]` within five seconds, `Alt-2` still switches away, `r` recovers it.
6. Put a typo in `config.toml`: a notice, and wwt still starts.

- [x] **Step 9: Run everything one last time**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: PASS, clean.

- [x] **Step 10: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
docs: write down what a tab without a target is, and what it costs

The four amendments M7's design named, into the parent spec: the restart
rebuilds from live state and not from the session file, deadlines are two
classes with a typed timeout that does not degrade a tab, the M4
deferral on eviction is discharged, and restore is lazy.

Records the measurements rather than the claims: a switch to an evicted
tab against a switch to an attached one, which is M4's repaint guarantee
surviving M7.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

## Done when

- Killing Chromium leaves the frame you were reading up, and the page comes back where it was.
- A page wedged in a JavaScript loop is `[stalled]` within five seconds and stays switchable away from.
- A session of thirty tabs holds `max_tabs` targets, and the bar shows thirty.
- Startup launches one page.
- `config.toml` sets the limit, the search and the browser, and a bad one is a notice.
- `cargo test --workspace` passes and clippy is clean.
- The parent spec, `CONTEXT.md`, `CLAUDE.md` and the README describe what the code does.
