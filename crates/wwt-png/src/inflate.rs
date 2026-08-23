//! Deflate, because a PNG's pixels are behind one and adding a crate to
//! read them is the one thing this repo will not do.
//!
//! The structure is zlib's own `puff.c`: decode a canonical Huffman code
//! by walking code lengths shortest first, so the tables are two small
//! arrays rather than a lookup structure. It is not the fastest way to
//! inflate. The picture is a few thousand pixels and arrives every 33ms,
//! so the fastest way is not what this needs to be.

use crate::Error;

/// Bits, least significant first, which is the order deflate uses for
/// everything except the Huffman codes themselves.
struct Bits<'a> {
    data: &'a [u8],
    /// Index of the next byte to draw from.
    byte: usize,
    /// Bits drawn from the data and not yet handed out.
    accumulator: u32,
    held: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, byte: 0, accumulator: 0, held: 0 }
    }

    /// `count` bits, LSB first. Deflate never asks for more than 16.
    fn take(&mut self, count: u32) -> Result<u32, Error> {
        while self.held < count {
            let next = *self.data.get(self.byte).ok_or(Error::Deflate)?;
            self.byte += 1;
            self.accumulator |= u32::from(next) << self.held;
            self.held += 8;
        }
        let value = self.accumulator & ((1u32 << count) - 1);
        self.accumulator >>= count;
        self.held -= count;
        Ok(value)
    }

    /// Drop the rest of the current byte. A stored block starts on a
    /// boundary, and it is the only thing in the format that does.
    fn align(&mut self) {
        let partial = self.held % 8;
        self.accumulator >>= partial;
        self.held -= partial;
    }

    /// The next whole byte, once aligned.
    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(8)? as u8)
    }
}

/// A canonical Huffman code, as counts per length and symbols in order.
///
/// This is the representation that makes decoding a walk rather than a
/// table build: the codes of each length are consecutive, so knowing how
/// many there are of each length is knowing all of them.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Self, Error> {
        let mut counts = [0u16; 16];
        for &length in lengths {
            if usize::from(length) >= counts.len() {
                return Err(Error::Deflate);
            }
            counts[usize::from(length)] += 1;
        }

        // An over-subscribed code claims more codes of some length than
        // exist. `left` is how many codes of the current length are still
        // unclaimed, doubling as the length grows.
        let mut left = 1i32;
        for &count in counts.iter().skip(1) {
            left <<= 1;
            left -= i32::from(count);
            if left < 0 {
                return Err(Error::Deflate);
            }
        }

        let mut offsets = [0u16; 16];
        for length in 1..15 {
            offsets[length + 1] = offsets[length] + counts[length];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (symbol, &length) in lengths.iter().enumerate() {
            if length != 0 {
                symbols[usize::from(offsets[usize::from(length)])] = symbol as u16;
                offsets[usize::from(length)] += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    /// One symbol. Codes are packed most significant bit first, which is
    /// why this shifts the accumulating code left rather than right.
    fn decode(&self, bits: &mut Bits<'_>) -> Result<u16, Error> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for length in 1..16 {
            code |= bits.take(1)? as i32;
            let count = i32::from(self.counts[length]);
            if code - count < first {
                let at = index + (code - first);
                return self.symbols.get(at as usize).copied().ok_or(Error::Deflate);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(Error::Deflate)
    }
}

/// Length symbol 257..=285: the base length and how many extra bits follow.
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
    131, 163, 195, 227, 258,
];
const LENGTH_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
    13, 13,
];
/// The order the code-length code's own lengths arrive in. Not sorted:
/// the order puts the lengths most likely to be nonzero first, so the
/// trailing zeros can be omitted.
const CODE_LENGTH_ORDER: [usize; 19] =
    [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

fn fixed_codes() -> Result<(Huffman, Huffman), Error> {
    let mut lengths = [0u8; 288];
    for (symbol, length) in lengths.iter_mut().enumerate() {
        *length = match symbol {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let literals = Huffman::new(&lengths)?;
    let distances = Huffman::new(&[5u8; 30])?;
    Ok((literals, distances))
}

fn dynamic_codes(bits: &mut Bits<'_>) -> Result<(Huffman, Huffman), Error> {
    let literal_count = bits.take(5)? as usize + 257;
    let distance_count = bits.take(5)? as usize + 1;
    let code_length_count = bits.take(4)? as usize + 4;
    if literal_count > 286 || distance_count > 30 {
        return Err(Error::Deflate);
    }

    let mut code_lengths = [0u8; 19];
    for &position in CODE_LENGTH_ORDER.iter().take(code_length_count) {
        code_lengths[position] = bits.take(3)? as u8;
    }
    let code_length_code = Huffman::new(&code_lengths)?;

    // The literal and distance lengths are one run, which is why a repeat
    // can carry across the boundary between them.
    let mut lengths = vec![0u8; literal_count + distance_count];
    let mut written = 0usize;
    while written < lengths.len() {
        let symbol = code_length_code.decode(bits)?;
        match symbol {
            0..=15 => {
                lengths[written] = symbol as u8;
                written += 1;
            }
            16 => {
                // Repeat the previous length, so there has to be one.
                let previous = *lengths.get(written.wrapping_sub(1)).ok_or(Error::Deflate)?;
                let repeat = 3 + bits.take(2)? as usize;
                for _ in 0..repeat {
                    *lengths.get_mut(written).ok_or(Error::Deflate)? = previous;
                    written += 1;
                }
            }
            17 => {
                let repeat = 3 + bits.take(3)? as usize;
                written = written.checked_add(repeat).ok_or(Error::Deflate)?;
                if written > lengths.len() {
                    return Err(Error::Deflate);
                }
            }
            18 => {
                let repeat = 11 + bits.take(7)? as usize;
                written = written.checked_add(repeat).ok_or(Error::Deflate)?;
                if written > lengths.len() {
                    return Err(Error::Deflate);
                }
            }
            _ => return Err(Error::Deflate),
        }
    }

    let literals = Huffman::new(&lengths[..literal_count])?;
    let distances = Huffman::new(&lengths[literal_count..])?;
    Ok((literals, distances))
}

fn block(
    bits: &mut Bits<'_>,
    out: &mut Vec<u8>,
    literals: &Huffman,
    distances: &Huffman,
) -> Result<(), Error> {
    loop {
        let symbol = literals.decode(bits)?;
        match symbol {
            0..=255 => out.push(symbol as u8),
            256 => return Ok(()),
            257..=285 => {
                let index = usize::from(symbol) - 257;
                let length =
                    usize::from(LENGTH_BASE[index]) + bits.take(LENGTH_EXTRA[index])? as usize;
                let symbol = usize::from(distances.decode(bits)?);
                if symbol >= DISTANCE_BASE.len() {
                    return Err(Error::Deflate);
                }
                let distance =
                    usize::from(DISTANCE_BASE[symbol]) + bits.take(DISTANCE_EXTRA[symbol])? as usize;
                if distance > out.len() {
                    return Err(Error::Deflate);
                }
                // A byte at a time, because the source and the destination
                // overlap whenever the distance is less than the length,
                // which is how deflate says "repeat this run".
                let start = out.len() - distance;
                for offset in 0..length {
                    out.push(out[start + offset]);
                }
            }
            _ => return Err(Error::Deflate),
        }
    }
}

/// Inflate a zlib stream: a two-byte header, deflate data, and an adler32
/// this does not check. The checksum would catch a corrupt frame that the
/// PNG's own CRCs and the websocket's framing have both already passed,
/// which is a cost per frame for a case that cannot reach us.
pub fn zlib(data: &[u8]) -> Result<Vec<u8>, Error> {
    let header = data.get(..2).ok_or(Error::Deflate)?;
    if header[0] & 0x0f != 8 {
        return Err(Error::Deflate);
    }
    if header[1] & 0x20 != 0 {
        // A preset dictionary. Nothing sends one, and decoding as though
        // it were absent produces confident nonsense rather than an error.
        return Err(Error::Deflate);
    }
    if (u32::from(header[0]) * 256 + u32::from(header[1])) % 31 != 0 {
        return Err(Error::Deflate);
    }

    let mut bits = Bits::new(&data[2..]);
    let mut out = Vec::new();
    loop {
        let final_block = bits.take(1)? == 1;
        match bits.take(2)? {
            0 => {
                bits.align();
                let low = u16::from(bits.byte()?);
                let high = u16::from(bits.byte()?);
                let length = low | (high << 8);
                let low = u16::from(bits.byte()?);
                let high = u16::from(bits.byte()?);
                let complement = low | (high << 8);
                if length != !complement {
                    return Err(Error::Deflate);
                }
                for _ in 0..length {
                    out.push(bits.byte()?);
                }
            }
            1 => {
                let (literals, distances) = fixed_codes()?;
                block(&mut bits, &mut out, &literals, &distances)?;
            }
            2 => {
                let (literals, distances) = dynamic_codes(&mut bits)?;
                block(&mut bits, &mut out, &literals, &distances)?;
            }
            _ => return Err(Error::Deflate),
        }
        if final_block {
            return Ok(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zlib stream around a deflate payload. The 2-byte header is
    /// 0x78 0x01, which is what "deflate, 32K window, no preset
    /// dictionary" is; the adler32 trailer is not checked by this decoder
    /// and is written as zeros.
    fn zlib_stream(deflate: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        out.extend_from_slice(deflate);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out
    }

    #[test]
    fn a_stored_block_inflates_to_itself() {
        // BFINAL=1, BTYPE=00, pad to a byte, LEN=3, NLEN=!3, then "abc".
        let deflate = [0x01, 0x03, 0x00, 0xfc, 0xff, b'a', b'b', b'c'];
        assert_eq!(zlib(&zlib_stream(&deflate)).expect("stored"), b"abc");
    }

    #[test]
    fn a_stored_block_whose_length_is_not_its_complement_is_an_error() {
        let deflate = [0x01, 0x03, 0x00, 0x00, 0x00, b'a', b'b', b'c'];
        assert_eq!(zlib(&zlib_stream(&deflate)), Err(Error::Deflate));
    }

    #[test]
    fn a_fixed_huffman_block_inflates() {
        // "hello" under fixed Huffman, as zlib produces it.
        let stream = [
            0x78, 0x01, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00, 0x06, 0x2c, 0x02, 0x15,
        ];
        assert_eq!(zlib(&stream).expect("fixed"), b"hello");
    }

    #[test]
    fn a_back_reference_copies_from_what_was_already_written() {
        // "aaaaaaaaaa": one literal and a length-9 distance-1 copy, which
        // is the overlapping case a byte-at-a-time copy gets right and a
        // slice copy gets wrong.
        let stream = [0x78, 0xda, 0x4b, 0x4c, 0x84, 0x01, 0x00, 0x14, 0xe1, 0x03, 0xcb];
        assert_eq!(zlib(&stream).expect("copy"), b"aaaaaaaaaa".to_vec());
    }

    #[test]
    fn a_dynamic_huffman_block_inflates() {
        // A sentence sixty times over. Long and repetitive enough that
        // zlib pays for its own code tables rather than using the fixed
        // ones, which is the only way to get a block of this type at all:
        // everything shorter than a couple of kilobytes comes back fixed.
        let stream = [
            0x78, 0xda, 0xed, 0xca, 0xdb, 0x15, 0x40, 0x30, 0x14, 0x05, 0xd1, 0x56, 0x4e,
            0x05, 0x7a, 0x22, 0x82, 0x78, 0x5d, 0x22, 0xf1, 0xaa, 0x9e, 0xa5, 0x06, 0x9f,
            0xf3, 0x39, 0x6b, 0x76, 0xea, 0xbc, 0xd6, 0x1c, 0xdc, 0xa0, 0x2a, 0xda, 0x31,
            0xab, 0xb1, 0x53, 0x7d, 0x9e, 0x96, 0x4d, 0xb6, 0xfb, 0xa8, 0xf4, 0xee, 0xb1,
            0xbc, 0x2f, 0xd5, 0xd6, 0x16, 0x5f, 0x81, 0xc1, 0x60, 0x30, 0x18, 0x0c, 0x06,
            0x83, 0xc1, 0x3f, 0xe1, 0x07, 0xf9, 0x47, 0xd0, 0xd2,
        ];
        let expected = "the quick brown fox jumps over the lazy dog. ".repeat(60);
        assert_eq!(zlib(&stream).expect("dynamic"), expected.as_bytes());
    }

    #[test]
    fn a_stream_that_is_not_deflate_is_refused() {
        // Compression method 7 rather than 8.
        assert_eq!(zlib(&[0x77, 0x01, 0x00]), Err(Error::Deflate));
    }

    #[test]
    fn a_preset_dictionary_is_refused_rather_than_ignored() {
        // FDICT set. Nothing sends one, and decoding as though it were not
        // set produces confident nonsense.
        assert_eq!(zlib(&[0x78, 0x20, 0x00]), Err(Error::Deflate));
    }

    #[test]
    fn a_truncated_stream_is_an_error_rather_than_a_short_answer() {
        assert_eq!(zlib(&[0x78, 0x01, 0x4b]), Err(Error::Deflate));
    }
}
