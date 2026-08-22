//! The `:` command line.

/// Schemes we pass through untouched. Anything else is treated as a bare
/// host and given `https://`.
const SCHEMES: &[&str] = &["http://", "https://", "file://", "about:", "data:"];

/// Something you can turn on or off from the `:` line.
#[derive(Debug, Clone, PartialEq)]
pub enum Setting {
    /// Terminal mouse capture. Off hands text selection back to terminals
    /// that do not give it to you with shift held.
    Mouse(bool),
    /// Show the page as a picture of itself rather than as its runs.
    Pixel(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Open(String),
    TabOpen(String),
    TabClose,
    TabNext,
    TabPrev,
    Back,
    Forward,
    Reload,
    Set(Setting),
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
        "tabopen" | "t" => {
            if rest.is_empty() {
                return Err("tabopen needs a URL".to_string());
            }
            Ok(Command::TabOpen(normalize_url(rest)?))
        }
        "tabclose" => Ok(Command::TabClose),
        "tabnext" => Ok(Command::TabNext),
        "tabprev" => Ok(Command::TabPrev),
        "back" | "b" => Ok(Command::Back),
        "forward" | "f" => Ok(Command::Forward),
        "reload" => Ok(Command::Reload),
        "set" => {
            let (setting, value) = match rest.split_once(char::is_whitespace) {
                Some((setting, value)) => (setting, value.trim()),
                None => (rest, ""),
            };
            match (setting, value) {
                ("mouse", "on") => Ok(Command::Set(Setting::Mouse(true))),
                ("mouse", "off") => Ok(Command::Set(Setting::Mouse(false))),
                ("mouse", other) => Err(format!("set mouse takes on or off, not {other:?}")),
                ("pixel", "on") => Ok(Command::Set(Setting::Pixel(true))),
                ("pixel", "off") => Ok(Command::Set(Setting::Pixel(false))),
                ("pixel", other) => Err(format!("set pixel takes on or off, not {other:?}")),
                (other, _) => Err(format!("unknown setting: {other}")),
            }
        }
        "quit" | "q" => Ok(Command::Quit),
        other => Err(format!("unknown command: {other}")),
    }
}

/// Where anything that is not a URL goes.
///
/// DuckDuckGo because it is the one that answers a browser like this one:
/// its html and lite endpoints are the whole page in the markup, and it
/// wants no account to search. Making this a setting is a configuration
/// question, and there is still no configuration.
const SEARCH: &str = "https://duckduckgo.com/?q=";

/// Turn what the user typed into a URL, or into a search for it.
///
/// The only thing that cannot be either is nothing at all. A single word
/// with a dot in it is where you meant to go; everything else is what you
/// wanted to look up, which is the guess that costs least when it is wrong:
/// a search for `notahost` is a page you can read, where the error it used
/// to be was a keystroke thrown away.
pub fn normalize_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("empty URL".to_string());
    }
    if SCHEMES.iter().any(|scheme| raw.starts_with(scheme)) {
        return Ok(raw.to_string());
    }
    if raw.split_whitespace().count() == 1
        && is_host(raw.split(['/', '?', '#']).next().unwrap_or(raw))
    {
        return Ok(format!("https://{raw}"));
    }
    Ok(format!("{SEARCH}{}", as_query(raw)))
}

/// Whether one word is somewhere to go rather than something to look up.
///
/// A dot is what usually says so. The exception is a port, because
/// `localhost:3000` has no dot in it and is the address a browser that lives
/// in a terminal gets typed at more than any other. A colon alone is not
/// enough: `error: not found` is a search, which is why only a word with
/// digits after the colon counts.
fn is_host(word: &str) -> bool {
    if word.contains('.') {
        return true;
    }
    match word.split_once(':') {
        Some((name, port)) => {
            !name.is_empty() && !port.is_empty() && port.chars().all(|c| c.is_ascii_digit())
        }
        None => word == "localhost",
    }
}

/// Percent-encode a search phrase for a query string.
///
/// Hand-rolled because the dependency set is fixed, and because the whole of
/// what is needed here is one rule: keep what is unreserved, turn a space
/// into `+`, and escape every other byte. Encoding is per byte, so text in
/// any alphabet survives as its UTF-8.
fn as_query(phrase: &str) -> String {
    let mut out = String::with_capacity(phrase.len());
    for byte in phrase.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The URL a search for `query` should land in, so the tests say what
    /// they are about rather than repeating the engine.
    fn searched(query: &str) -> String {
        format!("{SEARCH}{query}")
    }

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
    fn a_word_that_is_not_a_host_is_searched_for() {
        assert_eq!(normalize_url("banana"), Ok(searched("banana")));
    }

    #[test]
    fn several_words_are_searched_for_as_one_phrase() {
        assert_eq!(
            normalize_url("how tall is everest"),
            Ok(searched("how+tall+is+everest"))
        );
    }

    #[test]
    fn a_search_that_would_change_the_url_it_lands_in_is_escaped() {
        // Anything that means something in a query string has to stop
        // meaning it, or a search for one thing fetches another.
        assert_eq!(normalize_url("rust & c++ 100%"), Ok(searched("rust+%26+c%2B%2B+100%25")));
        assert_eq!(normalize_url("a/b?c#d"), Ok(searched("a%2Fb%3Fc%23d")));
    }

    #[test]
    fn a_search_in_someone_elses_alphabet_survives_the_trip() {
        assert_eq!(normalize_url("בננה"), Ok(searched("%D7%91%D7%A0%D7%A0%D7%94")));
    }

    #[test]
    fn a_host_is_still_a_host_rather_than_something_to_search_for() {
        // The fallback must not swallow the common case: one word with a dot
        // in it is where you meant to go, not what you wanted to look up.
        assert_eq!(normalize_url("example.com"), Ok("https://example.com".to_string()));
        assert_eq!(
            normalize_url("example.com/a/b?c=d"),
            Ok("https://example.com/a/b?c=d".to_string())
        );
    }

    #[test]
    fn a_host_and_port_is_somewhere_to_go_rather_than_something_to_look_up() {
        // No dot in it, and the one address a browser built in a terminal
        // gets typed at more than any other.
        assert_eq!(normalize_url("localhost:3000"), Ok("https://localhost:3000".to_string()));
        assert_eq!(
            normalize_url("localhost:8080/health"),
            Ok("https://localhost:8080/health".to_string())
        );
        assert_eq!(normalize_url("localhost"), Ok("https://localhost".to_string()));
    }

    #[test]
    fn a_phrase_with_a_colon_in_it_is_still_a_search() {
        assert_eq!(normalize_url("error: not found"), Ok(searched("error%3A+not+found")));
    }

    #[test]
    fn there_is_still_nothing_to_do_with_nothing() {
        assert!(normalize_url("   ").is_err());
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
    #[test]
    fn set_mouse_takes_on_and_off() {
        assert_eq!(parse("set mouse on"), Ok(Command::Set(Setting::Mouse(true))));
        assert_eq!(parse("set mouse off"), Ok(Command::Set(Setting::Mouse(false))));
    }

    #[test]
    fn a_setting_that_does_not_exist_names_itself() {
        assert_eq!(parse("set zoom 2"), Err("unknown setting: zoom".to_string()));
        assert!(parse("set mouse maybe").is_err());
    }

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
}
