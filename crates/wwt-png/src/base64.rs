//! base64 to bytes, and nothing else.
//!
//! wwt went five milestones without needing this: a screencast frame
//! arrives base64 and the graphics protocol wants it base64, so pixel mode
//! forwards the string it was given. Half-block has to look inside one.

use crate::Error;

/// The standard alphabet, as a reverse lookup. 64 marks a character that is
/// not in it, which is how the url-safe alphabet is rejected rather than
/// silently accepted.
const INVALID: u8 = 64;

fn value(byte: u8) -> u8 {
    match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => INVALID,
    }
}

pub fn decode(text: &str) -> Result<Vec<u8>, Error> {
    // Three bytes out of every four characters in, so this is exact for
    // unpadded input and one or two long for padded, which is one
    // allocation for a payload that can be hundreds of kilobytes.
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut accumulator = 0u32;
    let mut bits = 0u32;

    for byte in text.bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let value = value(byte);
        if value == INVALID {
            return Err(Error::Base64);
        }
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }

    // Whole groups leave nothing; a group of two or three characters leaves
    // four or two bits of padding, which are zero. Anything else is a
    // truncated group and a caller who thinks they have a whole image.
    if bits >= 6 {
        return Err(Error::Base64);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_string_decodes_to_nothing() {
        assert_eq!(decode("").expect("empty"), Vec::<u8>::new());
    }

    #[test]
    fn each_padding_length_decodes_to_its_own_bytes() {
        // The three shapes a final group can take, which is where every
        // base64 decoder that is wrong is wrong.
        assert_eq!(decode("TWFu").expect("no padding"), b"Man");
        assert_eq!(decode("TWE=").expect("one pad"), b"Ma");
        assert_eq!(decode("TQ==").expect("two pads"), b"M");
    }

    #[test]
    fn every_byte_value_survives_a_round_trip() {
        // Encoded by hand rather than by a crate, since there is none: this
        // is the first 12 bytes 0..12 in base64.
        assert_eq!(decode("AAECAwQFBgcICQoL").expect("bytes"), (0u8..12).collect::<Vec<_>>());
    }

    #[test]
    fn the_last_two_alphabet_characters_are_plus_and_slash() {
        // Chromium sends standard base64, not the url-safe alphabet, and a
        // decoder that quietly accepts both would hide the day that changes.
        assert_eq!(decode("++//").expect("alphabet"), vec![0xfb, 0xef, 0xff]);
        assert!(decode("--__").is_err(), "url-safe base64 is not what CDP sends");
    }

    #[test]
    fn whitespace_between_groups_is_ignored() {
        // Nothing in CDP wraps its base64, but a fixture read from a file
        // arrives with a trailing newline.
        assert_eq!(decode("TWFu\n").expect("trailing newline"), b"Man");
    }

    #[test]
    fn a_truncated_group_is_an_error_rather_than_a_short_read() {
        assert!(decode("TWFuA").is_err(), "five characters is not a whole group");
    }
}
