#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkId(pub usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub spans: Vec<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Heading { level: u8 },
    Paragraph,
    OrderedListItem { depth: usize, ordinal: usize },
    UnorderedListItem { depth: usize },
    Quote { depth: usize },
    Preformatted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub link: Option<LinkId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    pub new_tab: bool,
}

pub struct DocumentBuilder {
    blocks: Vec<Block>,
    links: Vec<Link>,
}

impl DocumentBuilder {
    pub fn new(links: Vec<Link>) -> Self {
        Self {
            blocks: Vec::new(),
            links,
        }
    }

    pub fn push_block(&mut self, kind: BlockKind, spans: Vec<Span>) {
        let mut normalized: Vec<Span> = Vec::new();
        for span in spans {
            if span.text.is_empty() {
                continue;
            }
            if let Some(previous) = normalized.last_mut()
                && previous.link == span.link
            {
                previous.text.push_str(&span.text);
                continue;
            }
            normalized.push(span);
        }
        self.blocks.push(Block {
            kind,
            spans: normalized,
        });
    }

    pub fn finish(self) -> Result<Document, LinkId> {
        if let Some(link) = self
            .blocks
            .iter()
            .flat_map(|block| &block.spans)
            .filter_map(|span| span.link)
            .find(|link| link.0 >= self.links.len())
        {
            return Err(link);
        }
        Ok(Document {
            blocks: self.blocks,
            links: self.links,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(url: &str) -> Link {
        Link {
            url: url.to_string(),
            new_tab: false,
        }
    }

    #[test]
    fn adjacent_spans_for_the_same_link_become_one_span() {
        let mut builder = DocumentBuilder::new(vec![link("https://example.com")]);
        builder.push_block(
            BlockKind::Paragraph,
            vec![
                Span {
                    text: "read".to_string(),
                    link: Some(LinkId(0)),
                },
                Span {
                    text: " more".to_string(),
                    link: Some(LinkId(0)),
                },
            ],
        );

        let document = builder.finish().expect("the link exists");

        assert_eq!(
            document.blocks[0].spans,
            vec![Span {
                text: "read more".to_string(),
                link: Some(LinkId(0)),
            }]
        );
    }

    #[test]
    fn spans_for_different_links_keep_their_boundary() {
        let mut builder = DocumentBuilder::new(vec![
            link("https://example.com/one"),
            link("https://example.com/two"),
        ]);
        let spans = vec![
            Span {
                text: "one".to_string(),
                link: Some(LinkId(0)),
            },
            Span {
                text: "two".to_string(),
                link: Some(LinkId(1)),
            },
        ];
        builder.push_block(BlockKind::Paragraph, spans.clone());

        let document = builder.finish().expect("both links exist");

        assert_eq!(document.blocks[0].spans, spans);
    }

    #[test]
    fn empty_spans_do_not_enter_the_document() {
        let mut builder = DocumentBuilder::new(Vec::new());
        builder.push_block(
            BlockKind::Paragraph,
            vec![
                Span {
                    text: String::new(),
                    link: None,
                },
                Span {
                    text: "kept".to_string(),
                    link: None,
                },
                Span {
                    text: String::new(),
                    link: None,
                },
            ],
        );

        let document = builder.finish().expect("the block has no links");

        assert_eq!(
            document.blocks[0].spans,
            vec![Span {
                text: "kept".to_string(),
                link: None,
            }]
        );
    }

    #[test]
    fn a_span_cannot_name_a_link_outside_the_document() {
        let mut builder = DocumentBuilder::new(vec![link("https://example.com")]);
        builder.push_block(
            BlockKind::Paragraph,
            vec![Span {
                text: "missing".to_string(),
                link: Some(LinkId(1)),
            }],
        );

        assert_eq!(builder.finish(), Err(LinkId(1)));
    }
}
