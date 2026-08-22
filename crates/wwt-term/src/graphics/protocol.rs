//! The three sequences and the cells that refer to them.
//!
//! Every sequence carries `q=2`, which suppresses the terminal's success and
//! error replies. A reply would arrive on stdin, where a keystroke lives, and
//! the input pump would have to know to throw away something that is not a
//! key.

use std::io::{self, Write};

use wwt_frame::CellRect;

use super::diacritics;

/// The two image ids wwt alternates between.
///
/// Two rather than one, because transmitting to an id tears down its
/// placement for as long as the transmission lasts, and a full-page PNG is
/// chunked into dozens of sequences. Sent to the id that is on screen, that
/// window is the whole transmission and it is visible as flicker. Sent to
/// the other one, the picture on screen is untouched until the new one is
/// complete and the cells are pointed at it.
///
/// A small image transmitted in a single sequence shows none of this, which
/// is why it took a page the size of a real one to find.
pub const IMAGE_IDS: [u32; 2] = [0x77_77_74, 0x77_77_75];

/// The placeholder character every cell showing image carries.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// Base64 payload per escape sequence. The protocol's limit is 4096.
const CHUNK: usize = 4096;

/// Send the image data to `id`, without placing it.
///
/// Deliberately not `a=T`. The placement is created separately once the
/// bytes are all here, so that nothing about what is on screen changes while
/// a chunked payload is still arriving.
pub fn transmit(payload: &str, id: u32, out: &mut impl Write) -> io::Result<()> {
    // An empty payload is not a picture. Sending the sequence anyway would
    // leave a transmission open with `m=1` and no terminator.
    if payload.is_empty() {
        return Ok(());
    }

    let mut chunks = payload.as_bytes().chunks(CHUNK).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = u8::from(chunks.peek().is_some());
        if first {
            // f=100 is PNG, t=d means the payload is in the escape itself.
            write!(out, "\x1b_Gq=2,a=t,f=100,t=d,i={id},m={more};")?;
            first = false;
        } else {
            // Continuations carry only the chunk marker: the terminal is
            // already holding the transmission this belongs to.
            write!(out, "\x1b_Gq=2,m={more};")?;
        }
        out.write_all(chunk)?;
        out.write_all(b"\x1b\\")?;
    }
    Ok(())
}

/// Create the virtual placement unicode placeholders refer to.
///
/// `p=1` names it, so re-issuing replaces that placement rather than adding
/// another to the pile.
pub fn place(area: CellRect, id: u32, out: &mut impl Write) -> io::Result<()> {
    write!(
        out,
        "\x1b_Gq=2,a=p,U=1,i={id},p=1,c={},r={}\x1b\\",
        area.cols, area.rows
    )
}

/// Forget an image and every placement of it.
pub fn delete(id: u32, out: &mut impl Write) -> io::Result<()> {
    write!(out, "\x1b_Gq=2,a=d,d=i,i={id}\x1b\\")
}

/// The foreground colour a placeholder cell must carry: the image id, in
/// three bytes.
pub fn image_fg(id: u32, out: &mut impl Write) -> io::Result<()> {
    write!(
        out,
        "\x1b[38;2;{};{};{}m",
        (id >> 16) & 0xff,
        (id >> 8) & 0xff,
        id & 0xff
    )
}

/// One placeholder cell, addressing `row` and `col` of the placement.
///
/// Every cell carries its own row and column. Addressing only the first cell
/// of each row and letting the rest continue from it is smaller and is
/// wrong: a cell with no diacritics continues from the cell before it, so a
/// hint label painted into the middle of a row orphans every placeholder
/// after it and the picture tears from the label to the right edge. Overlays
/// are the whole reason this design uses placeholders rather than placing
/// the image directly, so surviving one is the requirement.
///
/// `None` when the position is past what the diacritic table can address,
/// which means the terminal is bigger than the protocol can name and those
/// cells simply show no image.
pub fn placeholder(row: u16, col: u16, out: &mut impl Write) -> io::Result<bool> {
    let (Some(row_mark), Some(col_mark)) =
        (diacritics::for_index(row), diacritics::for_index(col))
    else {
        return Ok(false);
    };

    let mut buf = [0u8; 4];
    out.write_all(PLACEHOLDER.encode_utf8(&mut buf).as_bytes())?;
    out.write_all(row_mark.encode_utf8(&mut buf).as_bytes())?;
    out.write_all(col_mark.encode_utf8(&mut buf).as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(f: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        f(&mut out).expect("writing to a Vec cannot fail");
        String::from_utf8(out).expect("the protocol is ascii and utf-8 payloads")
    }

    fn area() -> CellRect {
        CellRect { col: 0, row: 1, cols: 4, rows: 2 }
    }

    #[test]
    fn the_two_image_ids_are_different() {
        // The whole point of having two: one is on screen while the other is
        // being filled.
        assert_ne!(IMAGE_IDS[0], IMAGE_IDS[1]);
    }

    #[test]
    fn a_payload_shorter_than_one_chunk_is_one_terminated_sequence() {
        let sent = bytes(|out| transmit("AAAA", IMAGE_IDS[0], out));
        assert_eq!(sent, "\x1b_Gq=2,a=t,f=100,t=d,i=7829364,m=0;AAAA\x1b\\");
    }

    #[test]
    fn transmitting_does_not_place() {
        // a=t and not a=T. Placing is a separate sequence sent once the
        // bytes are all here, so nothing on screen changes while a chunked
        // payload is still arriving.
        let sent = bytes(|out| transmit("AAAA", IMAGE_IDS[0], out));
        assert!(sent.contains("a=t,"), "transmit only");
        assert!(!sent.contains("a=T"), "and does not display");
        assert!(!sent.contains("U=1"), "nor place");
    }

    #[test]
    fn a_payload_longer_than_a_chunk_is_split_and_only_the_last_says_stop() {
        let payload = "x".repeat(CHUNK + 10);
        let sent = bytes(|out| transmit(&payload, IMAGE_IDS[0], out));
        assert_eq!(sent.matches("\x1b_G").count(), 2, "two sequences");
        assert!(sent.contains("m=1;"), "the first says more follows");
        assert!(sent.contains("\x1b_Gq=2,m=0;"), "the last says it is done");
        let continuation = sent.find("\x1b_Gq=2,m=").expect("a continuation");
        assert!(!sent[continuation..].contains("f=100"));
    }

    #[test]
    fn a_payload_of_exactly_one_chunk_does_not_send_an_empty_second() {
        let payload = "y".repeat(CHUNK);
        let sent = bytes(|out| transmit(&payload, IMAGE_IDS[0], out));
        assert_eq!(sent.matches("\x1b_G").count(), 1);
        assert!(sent.contains("m=0;"));
    }

    #[test]
    fn an_empty_payload_sends_nothing_at_all() {
        assert_eq!(bytes(|out| transmit("", IMAGE_IDS[0], out)), "");
    }

    #[test]
    fn a_placement_names_itself_and_says_how_many_cells_it_covers() {
        assert_eq!(
            bytes(|out| place(area(), IMAGE_IDS[1], out)),
            "\x1b_Gq=2,a=p,U=1,i=7829365,p=1,c=4,r=2\x1b\\"
        );
    }

    #[test]
    fn deleting_names_the_image_rather_than_the_screen() {
        assert_eq!(
            bytes(|out| delete(IMAGE_IDS[0], out)),
            "\x1b_Gq=2,a=d,d=i,i=7829364\x1b\\"
        );
    }

    #[test]
    fn a_placeholder_cell_carries_its_own_row_and_column() {
        // A cell with no diacritics continues from the cell before it, so a
        // label painted into the middle of a row would orphan every cell
        // after it. Overlays are why this design uses placeholders at all.
        let sent = bytes(|out| placeholder(0, 0, out).map(|_| ()));
        assert_eq!(sent.chars().count(), 3, "the cell and its two marks");
        assert!(sent.starts_with(PLACEHOLDER));
    }

    #[test]
    fn a_placeholder_says_which_image_it_belongs_to_in_its_foreground() {
        assert_eq!(bytes(|out| image_fg(IMAGE_IDS[0], out)), "\x1b[38;2;119;119;116m");
        assert_eq!(bytes(|out| image_fg(IMAGE_IDS[1], out)), "\x1b[38;2;119;119;117m");
    }

    #[test]
    fn a_position_past_the_table_addresses_nothing_rather_than_addressing_wrongly() {
        let mut out = Vec::new();
        assert!(!placeholder(297, 0, &mut out).expect("write"), "no row for it");
        assert!(!placeholder(0, 297, &mut out).expect("write"), "no column for it");
        assert!(out.is_empty(), "and nothing was written");
    }

    #[test]
    fn a_position_within_the_table_is_written() {
        let mut out = Vec::new();
        assert!(placeholder(296, 296, &mut out).expect("write"));
        assert!(!out.is_empty());
    }
}
