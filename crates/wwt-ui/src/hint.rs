//! Hint labels: assignment, filtering, and painting.

use wwt_frame::{CellPos, Frame, Rgb, Style};

/// The home row and the keys nearest it. Fourteen characters label 14
/// targets with one keystroke and 196 with two, which covers all but the
/// densest pages.
pub const ALPHABET: &[u8] = b"sadfjklewcmpgh";

/// Labels must be findable at a glance and must never be mistaken for the
/// page underneath. Reverse video does both, and still reads on a terminal
/// that ignores the colour.
const LABEL_STYLE: Style = Style {
    fg: Rgb { r: 0xff, g: 0xd7, b: 0x00 },
    bg: None,
    bold: true,
    reverse: true,
};

/// Labels for `count` targets, all of the same length.
///
/// Uniform length is what makes the set prefix-free: no label can be a
/// prefix of another, so the moment what you have typed matches a label it
/// cannot also be the beginning of a different one. That removes both the
/// timeout and the tie-break rule a variable-length scheme needs.
pub fn labels(count: usize) -> Vec<String> {
    let base = ALPHABET.len();
    let mut width = 1usize;
    let mut capacity = base;
    while capacity < count {
        capacity = capacity.saturating_mul(base);
        width += 1;
    }

    (0..count)
        .map(|index| {
            let mut rest = index;
            let mut label = vec![0u8; width];
            for slot in (0..width).rev() {
                label[slot] = ALPHABET[rest % base];
                rest /= base;
            }
            String::from_utf8(label).expect("the alphabet is ASCII")
        })
        .collect()
}

/// What typing one more character did to the set.
#[derive(Debug, Clone, PartialEq)]
pub enum Filtered {
    /// Still narrowing, with this many targets left.
    Waiting(usize),
    /// One target left. Return its original index.
    Activate(usize),
    /// Nothing matches what was typed. The caller leaves hint mode.
    None,
}

/// One pass through hint mode: label cells, their labels, and what has been
/// typed so far.
#[derive(Debug, Clone, PartialEq)]
pub struct HintSession {
    cells: Vec<CellPos>,
    labels: Vec<String>,
    typed: String,
}

impl HintSession {
    pub fn new(cells: Vec<CellPos>) -> Self {
        let labels = labels(cells.len());
        Self { cells, labels, typed: String::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn typed(&self) -> &str {
        &self.typed
    }

    pub fn push(&mut self, c: char) -> Filtered {
        // Labels are lowercase, so a stray shift does not lose your place.
        self.typed.push(c.to_ascii_lowercase());
        self.resolve()
    }

    pub fn pop(&mut self) -> Filtered {
        self.typed.pop();
        self.resolve()
    }

    /// Paint the label of every target that still matches.
    ///
    /// Labels are painted after the page, so they cover the text underneath.
    /// That is what makes them readable, and it is undone the moment hint
    /// mode ends.
    pub fn paint(&self, frame: &mut Frame) {
        for index in self.matching() {
            frame.paint_text(self.cells[index], &self.labels[index], LABEL_STYLE);
        }
    }

    /// What the statusline says while the labels are up.
    pub fn tag(&self) -> String {
        format!("-- HINT {} ({}) -- ", self.typed, self.matching().len())
    }

    fn matching(&self) -> Vec<usize> {
        self.labels
            .iter()
            .enumerate()
            .filter(|(_, label)| label.starts_with(&self.typed))
            .map(|(index, _)| index)
            .collect()
    }

    fn resolve(&self) -> Filtered {
        let matching = self.matching();
        match matching.len() {
            0 => Filtered::None,
            1 => Filtered::Activate(matching[0]),
            n => Filtered::Waiting(n),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wwt_frame::GridSize;

    /// `count` cells stacked one row apart, so each label lands on its
    /// own row and the painted result is readable.
    fn cells(count: usize) -> Vec<CellPos> {
        (0..count)
            .map(|row| CellPos {
                col: 0,
                row: u16::try_from(row).expect("the fixture fits in a terminal"),
            })
            .collect()
    }

    #[test]
    fn labels_are_one_character_while_the_alphabet_covers_the_targets() {
        let labels = labels(ALPHABET.len());
        assert_eq!(labels.len(), ALPHABET.len());
        assert!(labels.iter().all(|l| l.chars().count() == 1), "{labels:?}");
    }

    #[test]
    fn labels_grow_to_two_characters_one_past_the_alphabet() {
        let labels = labels(ALPHABET.len() + 1);
        assert!(labels.iter().all(|l| l.chars().count() == 2), "{labels:?}");
    }

    #[test]
    fn every_label_is_distinct() {
        let labels = labels(200);
        let mut sorted = labels.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len());
    }

    #[test]
    fn no_label_is_a_prefix_of_another() {
        // This is what uniform length buys, and it is why activation needs
        // no timeout: a full match cannot also be a partial one.
        let labels = labels(200);
        for a in &labels {
            for b in &labels {
                if a != b {
                    assert!(!b.starts_with(a.as_str()), "{a} is a prefix of {b}");
                }
            }
        }
    }

    #[test]
    fn typing_narrows_the_matching_set() {
        let mut session = HintSession::new(cells(100));
        let first = ALPHABET[0] as char;
        match session.push(first) {
            Filtered::Waiting(n) => assert!(n > 0 && n < 100, "narrowed to {n} of 100"),
            other => panic!("expected to still be narrowing, got {other:?}"),
        }
        assert_eq!(session.typed(), first.to_string());
    }

    #[test]
    fn a_unique_prefix_activates_its_target() {
        let mut session = HintSession::new(cells(3));
        // Three targets get one-character labels, so the first character
        // identifies one.
        match session.push(ALPHABET[1] as char) {
            Filtered::Activate(index) => assert_eq!(index, 1),
            other => panic!("expected an activation, got {other:?}"),
        }
    }

    #[test]
    fn a_prefix_that_matches_nothing_says_so() {
        let mut session = HintSession::new(cells(3));
        assert!(matches!(session.push('z'), Filtered::None));
    }

    #[test]
    fn backspace_widens_the_set_again() {
        let mut session = HintSession::new(cells(100));
        session.push(ALPHABET[0] as char);
        match session.pop() {
            Filtered::Waiting(n) => assert_eq!(n, 100),
            other => panic!("expected the whole set back, got {other:?}"),
        }
        assert_eq!(session.typed(), "");
    }

    #[test]
    fn labels_paint_at_the_cells_the_caller_supplied() {
        let session = HintSession::new(cells(3));
        let mut frame = Frame::new(GridSize { cols: 80, rows: 24 });
        session.paint(&mut frame);
        assert_eq!(frame.row_text(0), (ALPHABET[0] as char).to_string());
        assert_eq!(frame.row_text(1), (ALPHABET[1] as char).to_string());
        assert_eq!(frame.row_text(2), (ALPHABET[2] as char).to_string());
    }

    #[test]
    fn a_filtered_out_label_stops_being_painted() {
        let mut session = HintSession::new(cells(100));
        session.push(ALPHABET[0] as char);
        let mut frame = Frame::new(GridSize { cols: 80, rows: 24 });
        session.paint(&mut frame);
        // With 100 targets the labels are two characters wide, so typing the
        // alphabet's first character keeps the first fourteen and drops the
        // rest. Row 14 held one of the dropped ones.
        assert_eq!(frame.row_text(14), "", "a filtered label was still painted");
        assert_ne!(frame.row_text(0), "", "a matching label stopped being painted");
    }
}
