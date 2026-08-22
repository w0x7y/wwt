//! The three sequences and the cells that refer to them.
//!
//! Every sequence carries `q=2`, which suppresses the terminal's success and
//! error replies. A reply would arrive on stdin, where a keystroke lives, and
//! the input pump would have to know to throw away something that is not a
//! key.

use std::io::{self, Write};

use wwt_frame::CellRect;

use super::diacritics;

/// The one image id wwt uses.
///
/// Fixed rather than rotating, because a new frame transmitted to a live id
/// re-renders the placeholders already on screen, which is what keeps a
/// screencast frame from costing a repaint.
pub const IMAGE_ID: u32 = 0x77_77_74;

/// The placeholder character every cell showing image carries.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// Base64 payload per escape sequence. The protocol's limit is 4096.
const CHUNK: usize = 4096;

/// Send the image data, without placing it.
pub fn transmit(payload: &str, out: &mut impl Write) -> io::Result<()> {
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
            write!(out, "\x1b_Gq=2,a=t,f=100,t=d,i={IMAGE_ID},m={more};")?;
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
pub fn place(area: CellRect, out: &mut impl Write) -> io::Result<()> {
    write!(
        out,
        "\x1b_Gq=2,a=p,U=1,i={IMAGE_ID},c={},r={}\x1b\\",
        area.cols, area.rows
    )
}

/// Forget the image and every placement of it.
pub fn delete(out: &mut impl Write) -> io::Result<()> {
    write!(out, "\x1b_Gq=2,a=d,d=i,i={IMAGE_ID}\x1b\\")
}

/// Fill `area` with placeholder cells addressing the placement.
///
/// The image id rides in the foreground colour, and the row and column in
/// combining diacritics. Only the first cell of a row spends them: a cell
/// with none continues from the one before it, so a full-width row costs two
/// diacritics rather than two per cell.
pub fn placeholders(area: CellRect, out: &mut impl Write) -> io::Result<()> {
    let id = IMAGE_ID;
    write!(
        out,
        "\x1b[38;2;{};{};{}m",
        (id >> 16) & 0xff,
        (id >> 8) & 0xff,
        id & 0xff
    )?;

    let mut buf = [0u8; 4];
    for row in 0..area.rows {
        // A terminal with more rows than the table can address gets no
        // placeholders for them rather than wrong ones.
        let Some(row_mark) = diacritics::for_index(row) else {
            break;
        };
        let Some(col_mark) = diacritics::for_index(0) else {
            break;
        };

        write!(out, "\x1b[{};{}H", area.row + row + 1, area.col + 1)?;
        out.write_all(PLACEHOLDER.encode_utf8(&mut buf).as_bytes())?;
        out.write_all(row_mark.encode_utf8(&mut buf).as_bytes())?;
        out.write_all(col_mark.encode_utf8(&mut buf).as_bytes())?;
        for _ in 1..area.cols {
            out.write_all(PLACEHOLDER.encode_utf8(&mut buf).as_bytes())?;
        }
    }
    write!(out, "\x1b[0m")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(f: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        f(&mut out).expect("writing to a Vec cannot fail");
        String::from_utf8(out).expect("the protocol is ascii and utf-8 payloads")
    }

    #[test]
    fn a_payload_shorter_than_one_chunk_is_one_terminated_sequence() {
        let sent = bytes(|out| transmit("AAAA", out));
        assert_eq!(sent, "\x1b_Gq=2,a=t,f=100,t=d,i=7829364,m=0;AAAA\x1b\\");
    }

    #[test]
    fn a_payload_longer_than_a_chunk_is_split_and_only_the_last_says_stop() {
        let payload = "x".repeat(CHUNK + 10);
        let sent = bytes(|out| transmit(&payload, out));
        assert_eq!(sent.matches("\x1b_G").count(), 2, "two sequences");
        assert!(sent.contains("m=1;"), "the first says more follows");
        assert!(sent.contains("\x1b_Gq=2,m=0;"), "the last says it is done");
        // The continuation carries no format or id: the terminal already
        // knows what transmission this is.
        let continuation = sent.find("\x1b_Gq=2,m=").expect("a continuation");
        assert!(!sent[continuation..].contains("f=100"));
    }

    #[test]
    fn a_payload_of_exactly_one_chunk_does_not_send_an_empty_second() {
        let payload = "y".repeat(CHUNK);
        let sent = bytes(|out| transmit(&payload, out));
        assert_eq!(sent.matches("\x1b_G").count(), 1);
        assert!(sent.contains("m=0;"));
    }

    #[test]
    fn an_empty_payload_sends_nothing_at_all() {
        // Not a picture. A sequence with m=1 and no data would leave the
        // terminal holding a transmission that never ends.
        assert_eq!(bytes(|out| transmit("", out)), "");
    }

    #[test]
    fn a_placement_says_how_many_cells_it_covers() {
        let area = CellRect { col: 0, row: 1, cols: 80, rows: 22 };
        assert_eq!(
            bytes(|out| place(area, out)),
            "\x1b_Gq=2,a=p,U=1,i=7829364,c=80,r=22\x1b\\"
        );
    }

    #[test]
    fn deleting_names_the_image_rather_than_the_screen() {
        assert_eq!(bytes(delete), "\x1b_Gq=2,a=d,d=i,i=7829364\x1b\\");
    }

    #[test]
    fn placeholders_spend_diacritics_only_on_the_first_cell_of_a_row() {
        let area = CellRect { col: 0, row: 1, cols: 4, rows: 2 };
        let sent = bytes(|out| placeholders(area, out));
        assert_eq!(sent.matches(PLACEHOLDER).count(), 8, "one per cell");
        assert_eq!(
            sent.matches(diacritics::CODES[0]).count(),
            3,
            "row 0 and column 0 on the first row, column 0 on the second"
        );
    }

    #[test]
    fn placeholders_carry_the_image_id_in_the_foreground_bytes() {
        let area = CellRect { col: 0, row: 0, cols: 1, rows: 1 };
        let sent = bytes(|out| placeholders(area, out));
        assert!(sent.starts_with("\x1b[38;2;119;119;116m"), "{sent:?}");
    }

    #[test]
    fn a_grid_taller_than_the_table_stops_rather_than_addressing_wrongly() {
        let area = CellRect { col: 0, row: 0, cols: 1, rows: 400 };
        let sent = bytes(|out| placeholders(area, out));
        assert_eq!(sent.matches(PLACEHOLDER).count(), 297);
    }

    #[test]
    fn placeholders_are_addressed_from_the_areas_own_origin() {
        // The page does not start at the top of the screen: the tab bar is
        // row 0, and terminal addressing is 1-based on top of that.
        let area = CellRect { col: 0, row: 1, cols: 1, rows: 1 };
        assert!(bytes(|out| placeholders(area, out)).contains("\x1b[2;1H"));
    }
}
