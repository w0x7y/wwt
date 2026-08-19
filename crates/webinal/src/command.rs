//! The `:` command line.

/// Schemes we pass through untouched. Anything else is treated as a bare
/// host and given `https://`.
const SCHEMES: &[&str] = &["http://", "https://", "file://", "about:", "data:"];

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Open(String),
    Back,
    Forward,
    Reload,
    Quit,
}

/// Parse a command line. The caller has already stripped the leading `:`.
pub fn parse(line: &str) -> Result<Command, String> {
    let line = line.trim();
    let (name, rest) = match line.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (line, ""),
    };

    match name {
        "" => Err("empty command".to_string()),
        "open" | "o" => {
            if rest.is_empty() {
                return Err("open needs a URL".to_string());
            }
            Ok(Command::Open(normalize_url(rest)?))
        }
        "back" | "b" => Ok(Command::Back),
        "forward" | "f" => Ok(Command::Forward),
        "reload" => Ok(Command::Reload),
        "quit" | "q" => Ok(Command::Quit),
        other => Err(format!("unknown command: {other}")),
    }
}

/// Turn what the user typed into a URL, or explain why it is not one.
///
/// There is deliberately no search-engine fallback: choosing a default
/// engine is a configuration question, and there is no configuration yet.
pub fn normalize_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("empty URL".to_string());
    }
    if SCHEMES.iter().any(|scheme| raw.starts_with(scheme)) {
        return Ok(raw.to_string());
    }
    if raw.split_whitespace().count() > 1 {
        return Err(format!("not a URL: {raw}"));
    }
    // A bare host needs at least one dot to be distinguishable from a typo.
    let host = raw.split(['/', '?', '#']).next().unwrap_or(raw);
    if !host.contains('.') {
        return Err(format!("not a URL: {raw}"));
    }
    Ok(format!("https://{raw}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_takes_a_url() {
        assert_eq!(
            parse("open https://example.com"),
            Ok(Command::Open("https://example.com".to_string()))
        );
    }

    #[test]
    fn o_is_short_for_open() {
        assert_eq!(
            parse("o example.com"),
            Ok(Command::Open("https://example.com".to_string()))
        );
    }

    #[test]
    fn a_bare_host_gains_https() {
        assert_eq!(normalize_url("example.com"), Ok("https://example.com".to_string()));
    }

    #[test]
    fn an_explicit_scheme_is_left_alone() {
        assert_eq!(normalize_url("http://example.com"), Ok("http://example.com".to_string()));
        assert_eq!(normalize_url("file:///tmp/a.html"), Ok("file:///tmp/a.html".to_string()));
        assert_eq!(normalize_url("about:blank"), Ok("about:blank".to_string()));
    }

    #[test]
    fn something_that_is_not_a_url_is_an_error_not_a_search() {
        assert!(normalize_url("how tall is everest").is_err());
        assert!(normalize_url("notahost").is_err());
    }

    #[test]
    fn the_short_forms_parse() {
        assert_eq!(parse("q"), Ok(Command::Quit));
        assert_eq!(parse("quit"), Ok(Command::Quit));
        assert_eq!(parse("back"), Ok(Command::Back));
        assert_eq!(parse("forward"), Ok(Command::Forward));
        assert_eq!(parse("reload"), Ok(Command::Reload));
    }

    #[test]
    fn surrounding_whitespace_does_not_matter() {
        assert_eq!(parse("  quit  "), Ok(Command::Quit));
    }

    #[test]
    fn an_unknown_command_names_itself() {
        assert_eq!(parse("frobnicate"), Err("unknown command: frobnicate".to_string()));
    }

    #[test]
    fn open_without_an_argument_is_an_error() {
        assert!(parse("open").is_err());
    }

    #[test]
    fn an_empty_line_is_an_error() {
        assert!(parse("   ").is_err());
    }
}
