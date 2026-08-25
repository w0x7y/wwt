use crate::{Block, BlockKind, Document, LinkId};
use wwt_frame::{CellPos, Frame, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourcePos {
    pub block: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkRange {
    pub link: LinkId,
    pub row: usize,
    pub start: u16,
    pub end: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Fragment {
    col: u16,
    text: String,
    style: Style,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    text: String,
    source: SourcePos,
    fragments: Vec<Fragment>,
}

#[derive(Debug, Clone, Copy)]
struct SourceChar {
    ch: char,
    link: Option<LinkId>,
    offset: usize,
}

#[derive(Debug, Clone, Copy)]
struct OrdinaryLine<'a> {
    chars: &'a [SourceChar],
    source_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    rows: Vec<Row>,
    link_ranges: Vec<LinkRange>,
}

impl Layout {
    pub fn new(document: &Document, cols: u16) -> Self {
        let width = usize::from(cols.max(1));
        let mut layout = Self {
            rows: Vec::new(),
            link_ranges: Vec::new(),
        };
        for (block_index, block) in document.blocks.iter().enumerate() {
            layout.retarget_trailing_blank(SourcePos {
                block: block_index,
                offset: 0,
            });
            layout.push_block(block, block_index, width);
        }
        layout
    }

    pub fn rows(&self) -> usize {
        self.rows.len()
    }

    pub fn source_at(&self, row: usize) -> Option<SourcePos> {
        let last = self.rows.len().checked_sub(1)?;
        Some(self.rows[row.min(last)].source)
    }

    pub fn top_for(&self, source: SourcePos) -> usize {
        if let Some(exact) = self.rows.iter().position(|row| row.source == source) {
            return exact;
        }
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.source < source)
            .max_by_key(|(_, row)| row.source)
            .map_or(0, |(index, _)| index)
    }

    pub fn link_ranges(&self) -> &[LinkRange] {
        &self.link_ranges
    }

    pub fn visible_links(
        &self,
        top_row: usize,
        origin_row: u16,
        page_rows: u16,
    ) -> Vec<(LinkId, CellPos)> {
        let bottom = top_row.saturating_add(usize::from(page_rows));
        let mut visible = Vec::new();
        for range in self
            .link_ranges
            .iter()
            .filter(|range| range.row >= top_row && range.row < bottom)
        {
            if visible.iter().any(|(link, _)| *link == range.link) {
                continue;
            }
            let Ok(relative_row) = u16::try_from(range.row - top_row) else {
                continue;
            };
            let Some(row) = origin_row.checked_add(relative_row) else {
                continue;
            };
            visible.push((
                range.link,
                CellPos {
                    col: range.start,
                    row,
                },
            ));
        }
        visible
    }

    pub fn link_at(
        &self,
        cell: CellPos,
        top_row: usize,
        origin_row: u16,
        page_rows: u16,
    ) -> Option<LinkId> {
        let relative_row = cell.row.checked_sub(origin_row)?;
        if relative_row >= page_rows {
            return None;
        }
        let row = top_row.checked_add(usize::from(relative_row))?;
        self.link_ranges
            .iter()
            .find(|range| range.row == row && range.start <= cell.col && cell.col < range.end)
            .map(|range| range.link)
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
            for fragment in &row.fragments {
                frame.paint_text(
                    CellPos {
                        col: fragment.col,
                        row: frame_row,
                    },
                    &fragment.text,
                    fragment.style,
                );
            }
        }
    }

    fn push_block(&mut self, block: &Block, block_index: usize, width: usize) {
        let chars = source_chars(block);
        match block.kind {
            BlockKind::Paragraph => {
                self.push_ordinary(&chars, block_index, width, "", "", false);
                self.push_blank(SourcePos {
                    block: block_index,
                    offset: chars.len(),
                });
            }
            BlockKind::Heading { level } => {
                self.push_blank(SourcePos {
                    block: block_index,
                    offset: 0,
                });
                let lengths = self.push_ordinary(&chars, block_index, width, "", "", true);
                if level <= 2
                    && let Some(length) = lengths.into_iter().max()
                    && length > 0
                {
                    let rule = if level == 1 { '=' } else { '-' };
                    self.push_generated(
                        std::iter::repeat_n(rule, length).collect(),
                        SourcePos {
                            block: block_index,
                            offset: 0,
                        },
                        true,
                    );
                }
                self.push_blank(SourcePos {
                    block: block_index,
                    offset: chars.len(),
                });
            }
            BlockKind::UnorderedListItem { depth } => {
                let prefix = format!("{}• ", "  ".repeat(depth));
                self.push_with_prefix(&chars, block_index, width, &prefix, false);
            }
            BlockKind::OrderedListItem { depth, ordinal } => {
                let prefix = format!("{}{ordinal}. ", "  ".repeat(depth));
                self.push_with_prefix(&chars, block_index, width, &prefix, false);
            }
            BlockKind::Quote { depth } => {
                let prefix = "> ".repeat(depth);
                let prefix = capped_prefix(&prefix, width);
                self.push_ordinary(&chars, block_index, width, &prefix, &prefix, false);
            }
            BlockKind::Preformatted => self.push_preformatted(&chars, block_index, width),
        }
    }

    fn push_with_prefix(
        &mut self,
        chars: &[SourceChar],
        block: usize,
        width: usize,
        prefix: &str,
        bold: bool,
    ) {
        let prefix = capped_prefix(prefix, width);
        let continuation = " ".repeat(prefix.chars().count());
        self.push_ordinary(chars, block, width, &prefix, &continuation, bold);
    }

    fn push_ordinary(
        &mut self,
        chars: &[SourceChar],
        block: usize,
        width: usize,
        first_prefix: &str,
        continuation_prefix: &str,
        bold: bool,
    ) -> Vec<usize> {
        let prefix_width = first_prefix.chars().count();
        let content_width = width.saturating_sub(prefix_width).max(1);
        let lines = ordinary_lines(chars, content_width);
        let lengths = lines.iter().map(|line| line.chars.len()).collect();
        for (index, line) in lines.into_iter().enumerate() {
            let prefix = if index == 0 {
                first_prefix
            } else {
                continuation_prefix
            };
            self.push_content(
                prefix,
                line.chars,
                SourcePos {
                    block,
                    offset: line.source_offset,
                },
                bold,
            );
        }
        lengths
    }

    fn push_preformatted(&mut self, chars: &[SourceChar], block: usize, width: usize) {
        let mut line_start = 0;
        for line_end in chars
            .iter()
            .enumerate()
            .filter_map(|(index, ch)| (ch.ch == '\n').then_some(index))
            .chain(std::iter::once(chars.len()))
        {
            let line = &chars[line_start..line_end];
            if line.is_empty() {
                self.push_content(
                    "",
                    line,
                    SourcePos {
                        block,
                        offset: line_start,
                    },
                    false,
                );
            } else {
                for chunk in line.chunks(width) {
                    self.push_content("", chunk, SourcePos { block, offset: 0 }, false);
                }
            }
            line_start = line_end.saturating_add(1);
        }
    }

    fn push_content(
        &mut self,
        prefix: &str,
        chars: &[SourceChar],
        fallback_source: SourcePos,
        bold: bool,
    ) {
        let row_index = self.rows.len();
        let prefix_width = prefix.chars().count();
        let source = chars.first().map_or(fallback_source, |ch| SourcePos {
            block: fallback_source.block,
            offset: ch.offset,
        });
        let mut text = prefix.to_string();
        text.extend(chars.iter().map(|ch| ch.ch));
        let mut fragments = Vec::new();
        if !prefix.is_empty() {
            fragments.push(Fragment {
                col: 0,
                text: prefix.to_string(),
                style: Style::default(),
            });
        }

        let mut start = 0;
        while start < chars.len() {
            let link = chars[start].link;
            let style = Style {
                bold: bold || link.is_some(),
                ..Style::default()
            };
            let mut end = start + 1;
            while end < chars.len() && chars[end].link == link {
                end += 1;
            }
            let col = u16::try_from(prefix_width + start).expect("a row fits the terminal width");
            let end_col = u16::try_from(prefix_width + end).expect("a row fits the terminal width");
            fragments.push(Fragment {
                col,
                text: chars[start..end].iter().map(|ch| ch.ch).collect(),
                style,
            });
            if let Some(link) = link {
                self.link_ranges.push(LinkRange {
                    link,
                    row: row_index,
                    start: col,
                    end: end_col,
                });
            }
            start = end;
        }
        self.rows.push(Row {
            text,
            source,
            fragments,
        });
    }

    fn push_generated(&mut self, text: String, source: SourcePos, bold: bool) {
        let style = Style {
            bold,
            ..Style::default()
        };
        self.rows.push(Row {
            fragments: vec![Fragment {
                col: 0,
                text: text.clone(),
                style,
            }],
            text,
            source,
        });
    }

    fn push_blank(&mut self, source: SourcePos) {
        if self.rows.last().is_some_and(|row| row.text.is_empty()) {
            return;
        }
        self.rows.push(Row {
            text: String::new(),
            source,
            fragments: Vec::new(),
        });
    }

    fn retarget_trailing_blank(&mut self, source: SourcePos) {
        if let Some(row) = self.rows.last_mut()
            && row.text.is_empty()
        {
            row.source = source;
        }
    }
}

fn source_chars(block: &Block) -> Vec<SourceChar> {
    let mut offset = 0;
    let mut chars = Vec::new();
    for span in &block.spans {
        for ch in span.text.chars() {
            chars.push(SourceChar {
                ch,
                link: span.link,
                offset,
            });
            offset += 1;
        }
    }
    chars
}

fn ordinary_lines(chars: &[SourceChar], width: usize) -> Vec<OrdinaryLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        while start < chars.len() && chars[start].ch.is_whitespace() && chars[start].ch != '\n' {
            start += 1;
        }
        if start == chars.len() {
            break;
        }

        if chars[start].ch == '\n' {
            lines.push(OrdinaryLine {
                chars: &chars[start..start],
                source_offset: chars[start].offset,
            });
            start += 1;
            continue;
        }

        let hard_end = chars[start..]
            .iter()
            .position(|ch| ch.ch == '\n')
            .map_or(chars.len(), |offset| start + offset);
        let remaining = hard_end - start;
        let end = if remaining <= width {
            hard_end
        } else {
            let limit = start + width;
            (start + 1..=limit)
                .rev()
                .find(|&index| chars[index].ch.is_whitespace())
                .unwrap_or(limit)
        };
        lines.push(OrdinaryLine {
            chars: &chars[start..end],
            source_offset: chars[start].offset,
        });
        start = end;
        if start == hard_end && hard_end < chars.len() {
            start += 1;
        }
    }
    lines
}

fn capped_prefix(prefix: &str, width: usize) -> String {
    prefix.chars().take(width.saturating_sub(1)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, BlockKind, Document, Link, LinkId, Span};
    use wwt_frame::{CellPos, Frame, GridSize, Style};

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
    fn a_paragraph_hard_break_starts_a_new_terminal_row() {
        let document = document(vec![block(BlockKind::Paragraph, "before\n after")]);

        let layout = Layout::new(&document, 20);

        assert_eq!(texts(&layout), vec!["before", "after", ""]);
        assert_eq!(
            layout.rows[1].source,
            SourcePos {
                block: 0,
                offset: 8
            }
        );
    }

    #[test]
    fn ordinary_text_never_paints_a_control_character_into_a_cell() {
        let document = document(vec![block(BlockKind::Paragraph, "before\n after")]);
        let layout = Layout::new(&document, 20);
        let mut frame = Frame::new(GridSize { cols: 20, rows: 3 });

        layout.paint(&mut frame, 0, 0, 3);

        assert_eq!(frame.row_text(0), "before");
        assert_eq!(frame.row_text(1), "after");
        for row in 0..3 {
            for col in 0..20 {
                let ch = frame.cell(CellPos { col, row }).expect("cell inside frame").ch;
                assert!(!ch.is_control(), "control character at ({col}, {row}): {ch:?}");
            }
        }
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

    #[test]
    fn every_block_kind_has_a_fixed_terminal_presentation() {
        let document = document(vec![
            block(BlockKind::Heading { level: 1 }, "Title"),
            block(BlockKind::Paragraph, "body"),
            block(BlockKind::Heading { level: 2 }, "Sub"),
            block(BlockKind::UnorderedListItem { depth: 0 }, "item wraps"),
            block(
                BlockKind::OrderedListItem {
                    depth: 1,
                    ordinal: 3,
                },
                "ordered text",
            ),
            block(BlockKind::Quote { depth: 2 }, "quoted words"),
            block(BlockKind::Preformatted, " x\n0123456789012"),
        ]);

        let layout = Layout::new(&document, 12);

        assert_eq!(
            texts(&layout),
            vec![
                "",
                "Title",
                "=====",
                "",
                "body",
                "",
                "Sub",
                "---",
                "",
                "• item wraps",
                "  3. ordered",
                "     text",
                "> > quoted",
                "> > words",
                " x",
                "012345678901",
                "2",
            ]
        );

        let mut frame = Frame::new(GridSize { cols: 12, rows: 17 });
        layout.paint(&mut frame, 0, 0, 17);
        assert!(frame.cell(CellPos { col: 0, row: 1 }).unwrap().style.bold);
    }

    #[test]
    fn generated_prefixes_leave_one_column_for_content() {
        let document = document(vec![
            block(BlockKind::UnorderedListItem { depth: 99 }, "ab"),
            block(BlockKind::Quote { depth: 99 }, "cd"),
        ]);

        let layout = Layout::new(&document, 1);

        assert_eq!(texts(&layout), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn a_source_position_follows_the_same_text_across_widths() {
        let document = document(vec![block(BlockKind::Paragraph, "alpha beta gamma delta")]);
        let old = Layout::new(&document, 10);
        let source = old.source_at(1).expect("the second row has a source");

        let narrow = Layout::new(&document, 6);
        let wide = Layout::new(&document, 12);

        assert_eq!(
            source,
            SourcePos {
                block: 0,
                offset: 11
            }
        );
        assert_eq!(narrow.source_at(narrow.top_for(source)), Some(source));
        assert_eq!(wide.source_at(wide.top_for(source)), Some(source));
    }

    #[test]
    fn a_source_between_rows_uses_the_row_immediately_before_it() {
        let document = document(vec![block(BlockKind::Paragraph, "alpha beta gamma")]);
        let layout = Layout::new(&document, 6);

        let top = layout.top_for(SourcePos {
            block: 0,
            offset: 8,
        });

        assert_eq!(
            layout.source_at(top),
            Some(SourcePos {
                block: 0,
                offset: 6
            })
        );
    }

    #[test]
    fn a_source_past_a_shorter_document_clamps_to_its_last_row() {
        let document = document(vec![block(BlockKind::Paragraph, "short")]);
        let layout = Layout::new(&document, 20);

        let top = layout.top_for(SourcePos {
            block: 99,
            offset: 99,
        });

        assert_eq!(top, layout.rows() - 1);
    }

    #[test]
    fn one_link_wrapped_across_three_rows_has_three_ranges_and_one_hint() {
        let document = Document {
            blocks: vec![Block {
                kind: BlockKind::Paragraph,
                spans: vec![
                    Span {
                        text: "x ".to_string(),
                        link: None,
                    },
                    Span {
                        text: "abc defghij".to_string(),
                        link: Some(LinkId(0)),
                    },
                ],
            }],
            links: vec![Link {
                url: "https://example.com".to_string(),
                new_tab: false,
            }],
        };
        let layout = Layout::new(&document, 6);

        assert_eq!(
            layout.link_ranges(),
            &[
                LinkRange {
                    link: LinkId(0),
                    row: 0,
                    start: 2,
                    end: 5,
                },
                LinkRange {
                    link: LinkId(0),
                    row: 1,
                    start: 0,
                    end: 6,
                },
                LinkRange {
                    link: LinkId(0),
                    row: 2,
                    start: 0,
                    end: 1,
                },
            ]
        );
        assert_eq!(
            layout.visible_links(0, 1, 3),
            vec![(LinkId(0), CellPos { col: 2, row: 1 })]
        );
        assert_eq!(
            layout.visible_links(1, 1, 2),
            vec![(LinkId(0), CellPos { col: 0, row: 1 })]
        );
    }

    /// Not a wall-clock budget -- a release-build measurement of the pure
    /// reflow path at widths that exercise different wrapping shapes. Run
    /// with:
    ///
    ///     cargo test -p wwt-reader measure_reader_layout --release -- --nocapture
    #[test]
    fn measure_reader_layout() {
        let paragraph = "The quick brown fox jumps over a lazy dog while nobody watches the terminal reflow this semantic document.";
        let document = document(
            (0..3_000)
                .map(|_| block(BlockKind::Paragraph, paragraph))
                .collect(),
        );

        for cols in [40, 120] {
            let start = std::time::Instant::now();
            let layout = Layout::new(&document, cols);
            let elapsed = start.elapsed();

            assert!(
                layout.rows() >= 3_000,
                "the fixture must span thousands of rows"
            );
            for row in [0, layout.rows() / 2, layout.rows() - 1] {
                let source = layout
                    .source_at(row)
                    .expect("every row has a source anchor");
                let anchored = layout.top_for(source);
                assert!(anchored < layout.rows());
                assert!(layout.source_at(anchored).is_some_and(|at| at <= source));
            }
            println!(
                "reader layout: {} blocks into {} rows at {cols} columns in {elapsed:?}",
                document.blocks.len(),
                layout.rows()
            );
        }
    }

    #[test]
    fn link_hit_testing_accepts_only_the_visible_half_open_ranges() {
        let document = Document {
            blocks: vec![Block {
                kind: BlockKind::Paragraph,
                spans: vec![Span {
                    text: "link".to_string(),
                    link: Some(LinkId(0)),
                }],
            }],
            links: vec![Link {
                url: "https://example.com".to_string(),
                new_tab: false,
            }],
        };
        let layout = Layout::new(&document, 4);

        assert_eq!(
            layout.link_at(CellPos { col: 0, row: 1 }, 0, 1, 1),
            Some(LinkId(0))
        );
        assert_eq!(
            layout.link_at(CellPos { col: 3, row: 1 }, 0, 1, 1),
            Some(LinkId(0))
        );
        assert_eq!(layout.link_at(CellPos { col: 4, row: 1 }, 0, 1, 1), None);
        assert_eq!(layout.link_at(CellPos { col: 0, row: 0 }, 0, 1, 1), None);
        assert_eq!(layout.link_at(CellPos { col: 0, row: 2 }, 0, 1, 1), None);
    }

    #[test]
    fn links_use_bold_default_terminal_colours() {
        let document = Document {
            blocks: vec![Block {
                kind: BlockKind::Paragraph,
                spans: vec![Span {
                    text: "link".to_string(),
                    link: Some(LinkId(0)),
                }],
            }],
            links: vec![Link {
                url: "https://example.com".to_string(),
                new_tab: false,
            }],
        };
        let layout = Layout::new(&document, 4);
        let mut frame = Frame::new(GridSize { cols: 4, rows: 1 });

        layout.paint(&mut frame, 0, 0, 1);

        let style = frame.cell(CellPos { col: 0, row: 0 }).unwrap().style;
        assert_eq!(
            style,
            Style {
                bold: true,
                ..Style::default()
            }
        );
    }
}
