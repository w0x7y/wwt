//! What wwt reads out of `config.toml`, and what it does when it cannot.
//!
//! Four keys, and no more until something needs a fifth: a configuration
//! file fills up with settings nobody asked for unless it is defended.
//!
//! Nothing here can fail. A file that is not TOML, a key we do not know, a
//! value of the wrong type and a value out of range all produce the default
//! and a complaint, because a browser that will not start because of a typo
//! is worse than one that starts and tells you. The complaints become a
//! statusline notice, which is what the session file already does.

use std::path::{Path, PathBuf};

/// Where anything that is not a URL goes. DuckDuckGo because its html and
/// lite endpoints are the whole page in the markup and it wants no account,
/// which is what a browser like this one needs.
const DEFAULT_SEARCH: &str = "https://duckduckgo.com/?q={}";

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
    /// The terminal command used by the desktop launcher. The first item is
    /// the executable and the rest are its arguments before the wwt command.
    pub terminal: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_tabs: 8,
            search: DEFAULT_SEARCH.to_string(),
            chromium: None,
            terminal: vec!["kitty".to_string(), "-e".to_string()],
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
            return (config, vec![error.to_string()]);
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
                Some(_) => complaints.push("search needs {} where the query goes".to_string()),
                None => complaints.push("search must be a string".to_string()),
            },
            "chromium" => match value.as_str() {
                Some(path) => config.chromium = Some(PathBuf::from(path)),
                None => complaints.push("chromium must be a path".to_string()),
            },
            "terminal" => match value.as_array() {
                Some(parts)
                    if parts
                        .first()
                        .and_then(|part| part.as_str())
                        .is_some_and(|command| !command.is_empty())
                        && parts.iter().all(|part| part.as_str().is_some()) =>
                {
                    config.terminal = parts
                        .iter()
                        .filter_map(|part| part.as_str().map(str::to_owned))
                        .collect();
                }
                Some(_) => {
                    complaints.push("terminal must be a non-empty array of strings".to_string())
                }
                None => complaints.push("terminal must be an array of strings".to_string()),
            },
            other => complaints.push(format!("unknown setting: {other}")),
        }
    }

    (config, complaints)
}

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
            terminal = ["foot", "-e"]
            "#,
        );
        assert!(complaints.is_empty(), "{complaints:?}");
        assert_eq!(config.max_tabs, 3);
        assert_eq!(config.search, "https://example.com/find?q={}");
        assert_eq!(
            config.chromium.as_deref(),
            Some(Path::new("/opt/chromium/chrome"))
        );
        assert_eq!(config.terminal, ["foot", "-e"]);
    }

    #[test]
    fn a_terminal_command_can_be_configured() {
        let (config, complaints) = parse(r#"terminal = ["alacritty", "-e"]"#);

        assert!(complaints.is_empty(), "{complaints:?}");
        assert_eq!(config.terminal, ["alacritty", "-e"]);
    }

    #[test]
    fn an_empty_terminal_command_keeps_kitty_and_says_so() {
        let (config, complaints) = parse("terminal = []");

        assert_eq!(config.terminal, Config::default().terminal);
        assert_eq!(complaints.len(), 1);
        assert!(complaints[0].contains("non-empty"));
    }

    #[test]
    fn a_blank_terminal_executable_keeps_kitty_and_says_so() {
        let (config, complaints) = parse(r#"terminal = ["", "-e"]"#);

        assert_eq!(config.terminal, Config::default().terminal);
        assert_eq!(complaints.len(), 1);
        assert!(complaints[0].contains("non-empty"));
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
