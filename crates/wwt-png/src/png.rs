//! The container around the pixels, and the per-row filters that make them
//! compress.
//!
//! It reads what Chromium's screencast produces and refuses the rest. See
//! the crate docs: a decoder that accepts a format it will never be given
//! is untested code, and guessing wrong is worse than failing.

use crate::{Error, base64, inflate};

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// A decoded picture, always RGBA whatever the file said, so that the one
/// consumer downstream has one shape to handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Png {
    pub width: usize,
    pub height: usize,
    /// Row major, four bytes per pixel.
    pub pixels: Vec<u8>,
}

pub fn decode_base64(text: &str) -> Result<Png, Error> {
    decode(&base64::decode(text)?)
}

pub fn decode(bytes: &[u8]) -> Result<Png, Error> {
    if bytes.len() < SIGNATURE.len() || bytes[..SIGNATURE.len()] != SIGNATURE {
        return Err(Error::Png);
    }

    let mut at = SIGNATURE.len();
    let mut header: Option<(usize, usize, u8)> = None;
    let mut compressed = Vec::new();

    // Chunk layout: 4 bytes of length, 4 of type, the data, 4 of CRC. The
    // CRC is not checked. A frame reaches us over a websocket that framed
    // it and a CDP message that parsed as JSON, so a corrupt one is not a
    // failure mode that survives to here, and checking costs a pass over
    // every byte of every frame.
    while at + 8 <= bytes.len() {
        let length =
            u32::from_be_bytes(bytes[at..at + 4].try_into().map_err(|_| Error::Png)?) as usize;
        let kind = &bytes[at + 4..at + 8];
        let start = at + 8;
        let end = start.checked_add(length).ok_or(Error::Png)?;
        if end + 4 > bytes.len() {
            return Err(Error::Png);
        }
        let data = &bytes[start..end];

        match kind {
            b"IHDR" => {
                if data.len() < 13 {
                    return Err(Error::Png);
                }
                let width =
                    u32::from_be_bytes(data[0..4].try_into().map_err(|_| Error::Png)?) as usize;
                let height =
                    u32::from_be_bytes(data[4..8].try_into().map_err(|_| Error::Png)?) as usize;
                let depth = data[8];
                let colour = data[9];
                let compression = data[10];
                let filter = data[11];
                let interlace = data[12];
                if width == 0 || height == 0 {
                    return Err(Error::Png);
                }
                if compression != 0 || filter != 0 {
                    return Err(Error::Png);
                }
                // Everything this refuses, it refuses on purpose.
                if depth != 8 || interlace != 0 || !matches!(colour, 2 | 6) {
                    return Err(Error::Unsupported);
                }
                header = Some((width, height, colour));
            }
            b"IDAT" => compressed.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        at = end + 4;
    }

    let (width, height, colour) = header.ok_or(Error::Png)?;
    let channels = if colour == 6 { 4 } else { 3 };
    let raw = inflate::zlib(&compressed)?;
    unfilter(&raw, width, height, channels).map(|pixels| Png { width, height, pixels })
}

/// Undo the per-row filters and widen to RGBA.
///
/// Each row arrives with a filter byte in front of it, and the predictors
/// address the already-reconstructed bytes rather than the filtered ones,
/// which is why this walks bytes in place rather than rows in parallel.
fn unfilter(raw: &[u8], width: usize, height: usize, channels: usize) -> Result<Vec<u8>, Error> {
    let stride = width.checked_mul(channels).ok_or(Error::Png)?;
    if raw.len() != height.checked_mul(stride + 1).ok_or(Error::Png)? {
        return Err(Error::Png);
    }

    let mut current = vec![0u8; stride];
    let mut previous = vec![0u8; stride];
    let mut out = Vec::with_capacity(width * height * 4);

    for row in 0..height {
        let start = row * (stride + 1);
        let filter = raw[start];
        current.copy_from_slice(&raw[start + 1..start + 1 + stride]);

        for index in 0..stride {
            // The pixel to the left is `channels` bytes back, and does not
            // exist for the first pixel of a row, where the format says
            // zero rather than wrapping to the previous row.
            let left = if index >= channels { current[index - channels] } else { 0 };
            let above = previous[index];
            let above_left = if index >= channels { previous[index - channels] } else { 0 };
            current[index] = match filter {
                0 => current[index],
                1 => current[index].wrapping_add(left),
                2 => current[index].wrapping_add(above),
                3 => {
                    let average = ((u16::from(left) + u16::from(above)) / 2) as u8;
                    current[index].wrapping_add(average)
                }
                4 => current[index].wrapping_add(paeth(left, above, above_left)),
                _ => return Err(Error::Png),
            };
        }

        for pixel in current.chunks_exact(channels) {
            out.extend_from_slice(&pixel[..3]);
            out.push(if channels == 4 { pixel[3] } else { 255 });
        }
        std::mem::swap(&mut previous, &mut current);
    }

    Ok(out)
}

/// The PNG predictor: whichever of left, above and above-left is closest to
/// their linear combination.
fn paeth(left: u8, above: u8, above_left: u8) -> u8 {
    let estimate = i16::from(left) + i16::from(above) - i16::from(above_left);
    let d_left = (estimate - i16::from(left)).abs();
    let d_above = (estimate - i16::from(above)).abs();
    let d_above_left = (estimate - i16::from(above_left)).abs();
    if d_left <= d_above && d_left <= d_above_left {
        left
    } else if d_above <= d_above_left {
        above
    } else {
        above_left
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A whole PNG, built here so that a test can be read without opening a
    /// hex editor. `chunk` writes length, type, data and CRC.
    fn png(width: u32, height: u32, colour: u8, raw: &[u8]) -> Vec<u8> {
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, colour, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &store(raw));
        chunk(&mut out, b"IEND", &[]);
        out
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        // The CRC is not checked by the decoder, so it need not be right
        // here. That it is not checked is asserted below.
        out.extend_from_slice(&[0, 0, 0, 0]);
    }

    /// A zlib stream of stored deflate blocks, so a test fixture needs no
    /// compressor.
    fn store(raw: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        out.push(0x01);
        out.extend_from_slice(&(raw.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
        out.extend_from_slice(raw);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out
    }

    #[test]
    fn an_unfiltered_rgba_row_decodes_to_its_pixels() {
        // One row, two pixels, filter type 0.
        let raw = [0, 255, 0, 0, 255, 0, 255, 0, 255];
        let image = decode(&png(2, 1, 6, &raw)).expect("decode");
        assert_eq!(image.width, 2);
        assert_eq!(image.height, 1);
        assert_eq!(image.pixels, vec![255, 0, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn an_rgb_png_gains_an_opaque_alpha_channel() {
        // Colour type 2, which is what Chromium's screencast actually
        // sends. The consumer only ever wants RGBA, so widening here is
        // what keeps one path downstream instead of two.
        let raw = [0, 1, 2, 3, 4, 5, 6];
        let image = decode(&png(2, 1, 2, &raw)).expect("decode");
        assert_eq!(image.pixels, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn the_sub_filter_adds_the_pixel_to_its_left() {
        // Filter 1. Second pixel is stored as a delta of 10 on each channel.
        let raw = [1, 5, 5, 5, 255, 10, 10, 10, 0];
        let image = decode(&png(2, 1, 6, &raw)).expect("decode");
        assert_eq!(image.pixels, vec![5, 5, 5, 255, 15, 15, 15, 255]);
    }

    #[test]
    fn the_up_filter_adds_the_row_above() {
        // Filter 2 on the second row.
        let raw = [0, 5, 5, 5, 255, 2, 1, 1, 1, 0];
        let image = decode(&png(1, 2, 6, &raw)).expect("decode");
        assert_eq!(image.pixels, vec![5, 5, 5, 255, 6, 6, 6, 255]);
    }

    #[test]
    fn the_average_and_paeth_filters_decode() {
        // Filter 3 then filter 4, one pixel per row, so the predictors are
        // exercised without arithmetic nobody can check by eye. The alpha
        // deltas are chosen to reconstruct to 255 as well: a filter applies
        // to every channel, not to the three anyone thinks about.
        let raw = [0, 8, 8, 8, 255, 3, 2, 2, 2, 128, 4, 1, 1, 1, 0];
        let image = decode(&png(1, 3, 6, &raw)).expect("decode");
        assert_eq!(&image.pixels[0..4], &[8, 8, 8, 255]);
        // Average of left (0, no pixel) and above (8) is 4, plus 2.
        assert_eq!(&image.pixels[4..8], &[6, 6, 6, 255]);
        // Paeth with no left picks above, 6, plus 1.
        assert_eq!(&image.pixels[8..12], &[7, 7, 7, 255]);
    }

    #[test]
    fn an_unknown_filter_type_is_an_error() {
        let raw = [9, 0, 0, 0, 0];
        assert_eq!(decode(&png(1, 1, 6, &raw)), Err(Error::Png));
    }

    #[test]
    fn something_that_is_not_a_png_is_refused() {
        assert_eq!(decode(b"not a png at all"), Err(Error::Png));
        assert_eq!(decode(&[]), Err(Error::Png));
    }

    #[test]
    fn a_palettised_or_interlaced_or_16_bit_png_is_refused_rather_than_guessed() {
        // Chromium's screencast sends none of these, and a decoder that
        // half-handles one puts a plausible wrong picture on screen.
        let raw = [0, 0];
        assert_eq!(decode(&png(1, 1, 3, &raw)), Err(Error::Unsupported));

        let mut interlaced = png(1, 1, 6, &[0, 0, 0, 0, 0]);
        interlaced[28] = 1; // the interlace byte of IHDR
        assert_eq!(decode(&interlaced), Err(Error::Unsupported));

        let mut deep = png(1, 1, 6, &[0, 0, 0, 0, 0]);
        deep[24] = 16; // the bit depth byte
        assert_eq!(decode(&deep), Err(Error::Unsupported));
    }

    #[test]
    fn a_truncated_image_is_an_error_rather_than_a_short_picture() {
        // Two rows promised, one delivered.
        let raw = [0, 1, 2, 3, 4];
        assert_eq!(decode(&png(1, 2, 6, &raw)), Err(Error::Png));
    }

    #[test]
    fn idat_split_across_chunks_is_one_stream() {
        // A real PNG of any size arrives in several IDATs, and each one is
        // a slice of a single deflate stream rather than a stream of its
        // own. Splitting the fixture's stream in two proves they are
        // concatenated before inflating and not inflated one at a time.
        let raw = [0, 7, 7, 7, 255];
        let stream = store(&raw);
        let (first, second) = stream.split_at(4);

        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", first);
        chunk(&mut out, b"IDAT", second);
        chunk(&mut out, b"IEND", &[]);

        assert_eq!(decode(&out).expect("split idat").pixels, vec![7, 7, 7, 255]);
    }
}
