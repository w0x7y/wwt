//! Semantic reader extraction for one page.

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use wwt_reader::{BlockKind, Document, DocumentBuilder, Link, LinkId, Span};

use crate::extract::RawStatus;
use crate::{Page, Status};

const READER_JS: &str = include_str!("../assets/reader.js");

#[derive(Debug, Clone, PartialEq)]
pub struct ReaderExtraction {
    pub document: Document,
    pub status: Status,
}

#[derive(Debug, Deserialize)]
struct RawReaderExtraction {
    blocks: Vec<RawBlock>,
    links: Vec<RawLink>,
    #[serde(flatten)]
    status: RawStatus,
}

#[derive(Debug, Deserialize)]
struct RawBlock {
    kind: String,
    level: Option<u8>,
    depth: Option<usize>,
    ordinal: Option<usize>,
    spans: Vec<RawSpan>,
}

#[derive(Debug, Deserialize)]
struct RawSpan {
    text: String,
    link: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct RawLink {
    url: String,
    #[serde(rename = "newTab")]
    new_tab: bool,
}

impl RawBlock {
    fn block_kind(&self) -> Result<BlockKind> {
        match self.kind.as_str() {
            "heading" => {
                let level = self.level.context("reader heading has no level")?;
                if !(1..=6).contains(&level) {
                    bail!("reader heading has invalid level {level}");
                }
                Ok(BlockKind::Heading { level })
            }
            "paragraph" => Ok(BlockKind::Paragraph),
            "ordered-list-item" => Ok(BlockKind::OrderedListItem {
                depth: self
                    .depth
                    .context("reader ordered list item has no depth")?,
                ordinal: self
                    .ordinal
                    .context("reader ordered list item has no ordinal")?,
            }),
            "unordered-list-item" => Ok(BlockKind::UnorderedListItem {
                depth: self
                    .depth
                    .context("reader unordered list item has no depth")?,
            }),
            "quote" => Ok(BlockKind::Quote {
                depth: self.depth.context("reader quote has no depth")?,
            }),
            "preformatted" => Ok(BlockKind::Preformatted),
            other => bail!("reader returned unknown block kind {other:?}"),
        }
    }
}

impl RawReaderExtraction {
    fn into_reader(self) -> Result<ReaderExtraction> {
        if self.blocks.is_empty() {
            bail!("no readable content");
        }

        let links = self
            .links
            .into_iter()
            .map(|link| Link {
                url: link.url,
                new_tab: link.new_tab,
            })
            .collect();
        let mut document = DocumentBuilder::new(links);
        for block in self.blocks {
            let kind = block.block_kind()?;
            let spans = block
                .spans
                .into_iter()
                .map(|span| Span {
                    text: span.text,
                    link: span.link.map(LinkId),
                })
                .collect();
            document.push_block(kind, spans);
        }
        let document = document
            .finish()
            .map_err(|link| anyhow!("reader span names missing link {}", link.0))?;

        Ok(ReaderExtraction {
            document,
            status: self.status.into_status(),
        })
    }
}

impl Page {
    /// Read the dominant semantic content without asking Chromium for layout.
    pub async fn reader(&self) -> Result<ReaderExtraction> {
        let value = self
            .js(READER_JS)
            .await
            .context("run the reader extraction script")?;
        let raw: RawReaderExtraction = serde_json::from_value(value)
            .context("the reader extraction script returned an unexpected shape")?;
        raw.into_reader()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::RawReaderExtraction;

    fn extraction(blocks: Value, links: Value) -> RawReaderExtraction {
        serde_json::from_value(json!({
            "blocks": blocks,
            "links": links,
            "title": "title",
            "url": "https://example.com/",
            "scrollY": 0,
            "scrollHeight": 100,
            "innerHeight": 50,
        }))
        .expect("valid raw extraction")
    }

    #[test]
    fn an_empty_document_has_a_stable_error() {
        let error = extraction(json!([]), json!([]))
            .into_reader()
            .expect_err("empty reader content must fail");

        assert_eq!(error.to_string(), "no readable content");
    }

    #[test]
    fn an_unknown_block_kind_is_rejected() {
        let error = extraction(
            json!([{ "kind": "widget", "spans": [{ "text": "x", "link": null }] }]),
            json!([]),
        )
        .into_reader()
        .expect_err("unknown reader kind must fail");

        assert_eq!(
            error.to_string(),
            "reader returned unknown block kind \"widget\""
        );
    }

    #[test]
    fn a_heading_level_outside_html_is_rejected() {
        let error = extraction(
            json!([{
                "kind": "heading",
                "level": 7,
                "spans": [{ "text": "x", "link": null }],
            }]),
            json!([]),
        )
        .into_reader()
        .expect_err("invalid heading level must fail");

        assert_eq!(error.to_string(), "reader heading has invalid level 7");
    }

    #[test]
    fn a_span_cannot_refer_to_a_missing_link() {
        let error = extraction(
            json!([{
                "kind": "paragraph",
                "spans": [{ "text": "x", "link": 0 }],
            }]),
            json!([]),
        )
        .into_reader()
        .expect_err("missing reader link must fail");

        assert_eq!(error.to_string(), "reader span names missing link 0");
    }
}
