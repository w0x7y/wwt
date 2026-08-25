use crate::{BlockKind, Document};
use wwt_frame::{CellPos, Frame, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePos {
    pub block: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    text: String,
    source: SourcePos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    rows: Vec<Row>,
}

impl Layout {
    pub fn new(document: &Document, cols: u16) -> Self {
        let width = usize::from(cols.max(1));
        let mut rows = Vec::new();
        for (block_index, block) in document.blocks.iter().enumerate() {
            let text: Vec<char> = block
                .spans
                .iter()
                .flat_map(|span| span.text.chars())
                .collect();
            if block.kind == BlockKind::Preformatted {
                wrap_preformatted(&mut rows, &text, width, block_index);
            } else {
                wrap_ordinary(&mut rows, &text, width, block_index);
            }
            if block.kind == BlockKind::Paragraph {
                rows.push(Row {
                    text: String::new(),
                    source: SourcePos {
                        block: block_index,
                        offset: text.len(),
                    },
                });
            }
        }
        Self { rows }
    }

    pub fn rows(&self) -> usize {
        self.rows.len()
    }

    pub fn paint(&self, frame: &mut Frame, top_row: usize, origin_row: u16, page_rows: u16) {
        for (offset, row) in self
            .rows
            .iter()
            .skip(top_row)
            .take(usize::from(page_rows))
            .enumerate()
        {
            let Ok(offset) = u16::try_from(offset) else {
                break;
            };
            let Some(frame_row) = origin_row.checked_add(offset) else {
                break;
            };
            frame.paint_text(
                CellPos {
                    col: 0,
                    row: frame_row,
                },
                &row.text,
                Style::default(),
            );
        }
    }
}

fn wrap_ordinary(rows: &mut Vec<Row>, text: &[char], width: usize, block: usize) {
    let mut start = 0;
    while start < text.len() {
        while start < text.len() && text[start].is_whitespace() {
            start += 1;
        }
        if start == text.len() {
            break;
        }

        let remaining = text.len() - start;
        let end = if remaining <= width {
            text.len()
        } else {
            let limit = start + width;
            (start + 1..=limit)
                .rev()
                .find(|&index| text[index].is_whitespace())
                .unwrap_or(limit)
        };
        rows.push(Row {
            text: text[start..end].iter().collect(),
            source: SourcePos {
                block,
                offset: start,
            },
        });
        start = end;
    }
}

fn wrap_preformatted(rows: &mut Vec<Row>, text: &[char], width: usize, block: usize) {
    let mut line_start = 0;
    for line_end in text
        .iter()
        .enumerate()
        .filter_map(|(index, &ch)| (ch == '\n').then_some(index))
        .chain(std::iter::once(text.len()))
    {
        push_preformatted_line(rows, &text[line_start..line_end], width, block, line_start);
        line_start = line_end.saturating_add(1);
    }
}

fn push_preformatted_line(
    rows: &mut Vec<Row>,
    line: &[char],
    width: usize,
    block: usize,
    line_start: usize,
) {
    if line.is_empty() {
        rows.push(Row {
            text: String::new(),
            source: SourcePos {
                block,
                offset: line_start,
            },
        });
        return;
    }
    for (chunk, chars) in line.chunks(width).enumerate() {
        rows.push(Row {
            text: chars.iter().collect(),
            source: SourcePos {
                block,
                offset: line_start + chunk * width,
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, BlockKind, Document, Span};
    use wwt_frame::{Frame, GridSize};

    fn block(kind: BlockKind, text: &str) -> Block {
        Block {
            kind,
            spans: vec![Span {
                text: text.to_string(),
                link: None,
            }],
        }
    }

    fn document(blocks: Vec<Block>) -> Document {
        Document {
            blocks,
            links: Vec::new(),
        }
    }

    fn texts(layout: &Layout) -> Vec<&str> {
        layout.rows.iter().map(|row| row.text.as_str()).collect()
    }

    #[test]
    fn a_paragraph_wraps_at_the_last_space_that_fits() {
        let document = document(vec![block(BlockKind::Paragraph, "alpha beta gamma")]);

        let layout = Layout::new(&document, 10);

        assert_eq!(texts(&layout), vec!["alpha beta", "gamma", ""]);
    }

    #[test]
    fn a_word_wider_than_the_terminal_is_split_without_elision() {
        let document = document(vec![block(BlockKind::Paragraph, "abcdefghij")]);

        let layout = Layout::new(&document, 4);

        assert_eq!(texts(&layout), vec!["abcd", "efgh", "ij", ""]);
    }

    #[test]
    fn paragraphs_have_exactly_one_blank_row_between_them() {
        let document = document(vec![
            block(BlockKind::Paragraph, "first"),
            block(BlockKind::Paragraph, "second"),
        ]);

        let layout = Layout::new(&document, 20);

        assert_eq!(texts(&layout), vec!["first", "", "second", ""]);
    }

    #[test]
    fn preformatted_text_keeps_spaces_and_hard_breaks() {
        let document = document(vec![block(BlockKind::Preformatted, "  first\n second")]);

        let layout = Layout::new(&document, 20);

        assert_eq!(texts(&layout), vec!["  first", " second"]);
    }

    #[test]
    fn a_preformatted_line_wider_than_the_terminal_is_hard_wrapped() {
        let document = document(vec![block(BlockKind::Preformatted, "  abcde")]);

        let layout = Layout::new(&document, 4);

        assert_eq!(texts(&layout), vec!["  ab", "cde"]);
    }

    #[test]
    fn a_one_column_terminal_always_makes_progress() {
        let document = document(vec![block(BlockKind::Paragraph, "ab cd")]);

        let layout = Layout::new(&document, 1);

        assert_eq!(texts(&layout), vec!["a", "b", "c", "d", ""]);
    }

    #[test]
    fn each_row_names_its_first_source_character() {
        let document = document(vec![block(BlockKind::Paragraph, "alpha beta")]);

        let layout = Layout::new(&document, 5);

        assert_eq!(
            layout.rows[0].source,
            SourcePos {
                block: 0,
                offset: 0
            }
        );
        assert_eq!(
            layout.rows[1].source,
            SourcePos {
                block: 0,
                offset: 6
            }
        );
    }

    #[test]
    fn painting_starts_at_the_requested_layout_row() {
        let document = document(vec![block(BlockKind::Paragraph, "zero one two")]);
        let layout = Layout::new(&document, 5);
        let mut frame = Frame::new(GridSize { cols: 5, rows: 4 });

        layout.paint(&mut frame, 1, 1, 2);

        assert_eq!(frame.row_text(0), "");
        assert_eq!(frame.row_text(1), "one");
        assert_eq!(frame.row_text(2), "two");
    }

    #[test]
    fn painting_never_touches_a_row_below_the_page_area() {
        let document = document(vec![block(BlockKind::Paragraph, "zero one two")]);
        let layout = Layout::new(&document, 5);
        let mut frame = Frame::new(GridSize { cols: 5, rows: 4 });

        layout.paint(&mut frame, 0, 1, 1);

        assert_eq!(frame.row_text(1), "zero");
        assert_eq!(frame.row_text(2), "");
    }
}
