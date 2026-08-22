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

/// Send the image data and place it, in one action.
///
/// `a=T` rather than a transmission followed by `a=p`. Transmitting to an id
/// that already has a virtual placement destroys it, so as two sequences
/// there is a window in which the cells on screen address nothing and show
/// the terminal's background through the picture. At a scroll's frame rate
/// that window is visible as flicker, and occasionally as the whole image
/// blinking out. One action has no window.
///
/// `p=1` names the placement, so re-issuing it replaces that placement
/// rather than adding another to the pile.
pub fn transmit_and_place(payload: &str, area: CellRect, out: &mut impl Write) -> io::Result<()> {
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
            // f=100 is PNG, t=d means the payload is in the escape itself,
            // U=1 makes the placement the virtual kind unicode placeholders
            // refer to. The action lands when the last chunk arrives.
            write!(
                out,
                "\x1b_Gq=2,a=T,U=1,f=100,t=d,i={IMAGE_ID},p=1,c={},r={},m={more};",
                area.cols, area.rows
            )?;
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

/// Forget the image and every placement of it.
pub fn delete(out: &mut impl Write) -> io::Result<()> {
    write!(out, "\x1b_Gq=2,a=d,d=i,i={IMAGE_ID}\x1b\\")
}

/// Fill `area` with placeholder cells addressing the placement.
///
/// The image id rides in the foreground colour, and every cell carries its
/// own row and column as combining diacritics.
///
/// Addressing only the first cell of each row and letting the rest continue
/// from it would be smaller, and it is wrong: a cell with no diacritics
/// continues from the cell before it, so a hint label painted into the
/// middle of a row orphans every placeholder after it and the picture tears
/// from the label to the right edge. Overlays are the whole reason this
/// design uses placeholders rather than placing the image directly, so they
/// have to survive one. The cost is paid when placeholders are written,
/// which is on entering pixel mode, on a resize and on a switch, and never
/// on a frame.
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
        // A terminal bigger than the table can address gets no placeholders
        // for the part past the end rather than wrong ones.
        let Some(row_mark) = diacritics::for_index(row) else {
            break;
        };
        write!(out, "\x1b[{};{}H", area.row + row + 1, area.col + 1)?;
        for col in 0..area.cols {
            let Some(col_mark) = diacritics::for_index(col) else {
                break;
            };
            out.write_all(PLACEHOLDER.encode_utf8(&mut buf).as_bytes())?;
            out.write_all(row_mark.encode_utf8(&mut buf).as_bytes())?;
            out.write_all(col_mark.encode_utf8(&mut buf).as_bytes())?;
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

    fn area() -> CellRect {
        CellRect { col: 0, row: 1, cols: 4, rows: 2 }
    }

    #[test]
    fn a_payload_shorter_than_one_chunk_is_one_terminated_sequence() {
        let sent = bytes(|out| transmit_and_place("AAAA", area(), out));
        assert_eq!(
            sent,
            "\x1b_Gq=2,a=T,U=1,f=100,t=d,i=7829364,p=1,c=4,r=2,m=0;AAAA\x1b\\"
        );
    }

    #[test]
    fn transmitting_and_placing_are_one_action() {
        // As two sequences there is a window between them in which the cells
        // address a placement that no longer exists, and that window is the
        // flicker.
        let sent = bytes(|out| transmit_and_place("AAAA", area(), out));
        assert_eq!(sent.matches("\x1b_G").count(), 1, "one sequence, not two");
        assert!(sent.contains("a=T"), "transmit and display");
        assert!(sent.contains("U=1"), "as a virtual placement");
        assert!(sent.contains("p=1"), "with a name, so it replaces itself");
    }

    #[test]
    fn a_payload_longer_than_a_chunk_is_split_and_only_the_last_says_stop() {
        let payload = "x".repeat(CHUNK + 10);
        let sent = bytes(|out| transmit_and_place(&payload, area(), out));
        assert_eq!(sent.matches("\x1b_G").count(), 2, "two sequences");
        assert!(sent.contains("m=1;"), "the first says more follows");
        assert!(sent.contains("\x1b_Gq=2,m=0;"), "the last says it is done");
        // The continuation carries no format, id or geometry: the terminal
        // already knows what transmission this is.
        let continuation = sent.find("\x1b_Gq=2,m=").expect("a continuation");
        assert!(!sent[continuation..].contains("f=100"));
    }

    #[test]
    fn a_payload_of_exactly_one_chunk_does_not_send_an_empty_second() {
        let payload = "y".repeat(CHUNK);
        let sent = bytes(|out| transmit_and_place(&payload, area(), out));
        assert_eq!(sent.matches("\x1b_G").count(), 1);
        assert!(sent.contains("m=0;"));
    }

    #[test]
    fn an_empty_payload_sends_nothing_at_all() {
        // Not a picture. A sequence with m=1 and no data would leave the
        // terminal holding a transmission that never ends.
        assert_eq!(bytes(|out| transmit_and_place("", area(), out)), "");
    }

    #[test]
    fn a_placement_says_how_many_cells_it_covers() {
        let sent = bytes(|out| transmit_and_place("AAAA", area(), out));
        assert!(sent.contains("c=4,r=2"), "{sent:?}");
    }

    #[test]
    fn deleting_names_the_image_rather_than_the_screen() {
        assert_eq!(bytes(delete), "\x1b_Gq=2,a=d,d=i,i=7829364\x1b\\");
    }

    #[test]
    fn every_placeholder_cell_carries_its_own_row_and_column() {
        // A cell with no diacritics continues from the cell before it, so a
        // label painted into the middle of a row would orphan every cell
        // after it. Overlays are why this design uses placeholders at all.
        let sent = bytes(|out| placeholders(area(), out));
        let cells = sent.matches(PLACEHOLDER).count();
        assert_eq!(cells, 8, "one per cell");

        let marks: usize = diacritics::CODES
            .iter()
            .map(|mark| sent.matches(*mark).count())
            .sum();
        assert_eq!(marks, cells * 2, "a row and a column for every one of them");
    }

    #[test]
    fn placeholders_carry_the_image_id_in_the_foreground_bytes() {
        let one = CellRect { col: 0, row: 0, cols: 1, rows: 1 };
        let sent = bytes(|out| placeholders(one, out));
        assert!(sent.starts_with("\x1b[38;2;119;119;116m"), "{sent:?}");
    }

    #[test]
    fn a_grid_taller_than_the_table_stops_rather_than_addressing_wrongly() {
        let tall = CellRect { col: 0, row: 0, cols: 1, rows: 400 };
        let sent = bytes(|out| placeholders(tall, out));
        assert_eq!(sent.matches(PLACEHOLDER).count(), 297);
    }

    #[test]
    fn a_grid_wider_than_the_table_stops_at_the_edge_of_what_it_can_address() {
        let wide = CellRect { col: 0, row: 0, cols: 400, rows: 1 };
        let sent = bytes(|out| placeholders(wide, out));
        assert_eq!(sent.matches(PLACEHOLDER).count(), 297);
    }

    #[test]
    fn placeholders_are_addressed_from_the_areas_own_origin() {
        // The page does not start at the top of the screen: the tab bar is
        // row 0, and terminal addressing is 1-based on top of that.
        let one = CellRect { col: 0, row: 1, cols: 1, rows: 1 };
        assert!(bytes(|out| placeholders(one, out)).contains("\x1b[2;1H"));
    }
}
