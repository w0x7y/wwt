//! Parsing the colors `getComputedStyle` hands back.

use wwt_frame::{Rgb, Style};

/// Parse an `rgb()` or `rgba()` string. Anything unrecognized falls back to
/// the default foreground rather than failing the whole extraction.
pub fn parse_css_color(s: &str) -> Rgb {
    let fallback = Style::default().fg;
    let Some(open) = s.find('(') else { return fallback };
    let Some(close) = s.rfind(')') else {
        return fallback;
    };
    let body = &s[open + 1..close];
    // Handles "1, 2, 3", "1 2 3", and "1 2 3 / 0.5" in one pass.
    let body = body.split('/').next().unwrap_or(body);

    let mut parts = body
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty());

    let mut next = || -> Option<u8> {
        let raw: f64 = parts.next()?.parse().ok()?;
        Some(raw.clamp(0.0, 255.0) as u8)
    };

    match (next(), next(), next()) {
        (Some(r), Some(g), Some(b)) => Rgb { r, g, b },
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rgb() {
        assert_eq!(
            parse_css_color("rgb(255, 128, 0)"),
            Rgb { r: 255, g: 128, b: 0 }
        );
    }

    #[test]
    fn parses_rgb_without_spaces() {
        assert_eq!(parse_css_color("rgb(1,2,3)"), Rgb { r: 1, g: 2, b: 3 });
    }

    #[test]
    fn parses_rgba_and_ignores_alpha() {
        assert_eq!(
            parse_css_color("rgba(10, 20, 30, 0.5)"),
            Rgb { r: 10, g: 20, b: 30 }
        );
    }

    #[test]
    fn parses_the_modern_space_separated_form() {
        assert_eq!(
            parse_css_color("rgb(10 20 30 / 0.5)"),
            Rgb { r: 10, g: 20, b: 30 }
        );
    }

    #[test]
    fn clamps_out_of_range_components() {
        assert_eq!(parse_css_color("rgb(300, -5, 0)"), Rgb { r: 255, g: 0, b: 0 });
    }

    #[test]
    fn falls_back_to_the_default_style_color_on_junk() {
        // getComputedStyle always returns rgb()/rgba(), so anything else means
        // we misread something; a readable default beats a panic.
        assert_eq!(parse_css_color("chartreuse"), Style::default().fg);
        assert_eq!(parse_css_color(""), Style::default().fg);
    }
}
