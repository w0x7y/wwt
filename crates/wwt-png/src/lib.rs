//! Decoding the one picture format Chromium's screencast produces.
//!
//! Its own crate for the reason `wwt-frame` is: no I/O and no
//! dependencies, so all of it is arithmetic over bytes that a test can
//! assert on with no browser, no terminal and no page. Every other crate
//! here needs one of those to be interesting.
//!
//! It decodes what Chromium sends and refuses everything else. A decoder
//! that accepts a format it will never be given is code no test covers,
//! and a wrong guess about a format is worse than an error: it puts a
//! plausible wrong picture on screen.

pub mod base64;
pub mod inflate;

/// What can be wrong with a picture.
///
/// Deliberately coarse. Nothing recovers from any of these differently:
/// the frame is dropped, the previous picture stands, and the statusline
/// says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Not base64, or a truncated group.
    Base64,
    /// Not a PNG, or a PNG whose chunks do not add up.
    Png,
    /// A PNG this decoder refuses on purpose: interlaced, palettised,
    /// 16-bit, or anything else Chromium's screencast does not produce.
    Unsupported,
    /// The deflate stream is malformed.
    Deflate,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self {
            Error::Base64 => "the picture was not valid base64",
            Error::Png => "the picture was not a valid PNG",
            Error::Unsupported => "the picture is a PNG shape wwt does not decode",
            Error::Deflate => "the picture's compressed data was malformed",
        };
        f.write_str(what)
    }
}

impl std::error::Error for Error {}
