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
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// Write a snapshot, atomically.
///
/// Temp file in the same directory, then rename: a rename within one
/// filesystem is atomic, so a crash mid-write leaves the previous snapshot
/// wholly intact rather than a truncated one that reads as corrupt.
pub fn save(path: &Path, snapshot: &Snapshot) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;

    let text = serde_json::to_string_pretty(snapshot).map_err(|error| error.to_string())?;
    let temp = path.with_extension("json.new");
    std::fs::write(&temp, text).map_err(|error| format!("{}: {error}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|error| format!("{}: {error}", path.display()))
}

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

        assert!(
            load(&path).is_err(),
            "a corrupt file must be a notice, not silence"
        );
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
        assert!(
            leftovers.is_empty(),
            "temporary files were left behind: {leftovers:?}"
        );
    }
}
