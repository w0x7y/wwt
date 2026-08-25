mod common;

use common::{harness, open, runtime};
use wwt_reader::{Block, BlockKind, LinkId};

fn text(block: &Block) -> String {
    block.spans.iter().map(|span| span.text.as_str()).collect()
}

#[test]
fn an_article_beats_a_main_that_only_surrounds_it() {
    let h = harness();
    runtime().block_on(async {
        let document = open(&h, "reader.html")
            .await
            .reader()
            .await
            .expect("read the article")
            .document;
        let text: String = document.blocks.iter().map(text).collect();

        assert!(text.contains("Article title"), "document was {text:?}");
        assert!(!text.contains("Main introduction"), "document was {text:?}");
        assert!(!text.contains("Main conclusion"), "document was {text:?}");
    });
}

#[test]
fn a_main_can_win_on_substantial_text_it_owns() {
    let h = harness();
    runtime().block_on(async {
        let document = open(&h, "reader-competing.html")
            .await
            .reader()
            .await
            .expect("read the main content")
            .document;
        let text: String = document.blocks.iter().map(text).collect();

        assert!(
            text.contains("substantial introduction"),
            "document was {text:?}"
        );
        assert!(text.contains("Small release note"), "document was {text:?}");
        assert!(
            text.contains("substantial conclusion"),
            "document was {text:?}"
        );
    });
}

#[test]
fn no_landmark_falls_back_to_the_body_without_site_furniture() {
    let h = harness();
    runtime().block_on(async {
        let document = open(&h, "reader-body.html")
            .await
            .reader()
            .await
            .expect("read the body")
            .document;
        let text: String = document.blocks.iter().map(text).collect();

        assert!(text.contains("Body heading"), "document was {text:?}");
        assert!(
            text.contains("Body fallback content"),
            "document was {text:?}"
        );
        assert!(!text.contains("furniture"), "document was {text:?}");
    });
}

#[test]
fn semantic_blocks_retain_structure_and_document_order() {
    let h = harness();
    runtime().block_on(async {
        let document = open(&h, "reader.html")
            .await
            .reader()
            .await
            .expect("read the article")
            .document;
        let blocks: Vec<(BlockKind, String)> = document
            .blocks
            .iter()
            .map(|block| (block.kind.clone(), text(block)))
            .collect();

        assert_eq!(
            blocks,
            vec![
                (BlockKind::Heading { level: 1 }, "Article title".into()),
                (BlockKind::Paragraph, "Article byline stays.".into()),
                (
                    BlockKind::Paragraph,
                    "Alpha linked words omega.\nNext line.".into()
                ),
                (BlockKind::Heading { level: 2 }, "Details".into()),
                (
                    BlockKind::UnorderedListItem { depth: 0 },
                    "First item".into()
                ),
                (
                    BlockKind::UnorderedListItem { depth: 1 },
                    "Nested item".into()
                ),
                (
                    BlockKind::OrderedListItem {
                        depth: 0,
                        ordinal: 3,
                    },
                    "Third item".into(),
                ),
                (BlockKind::Quote { depth: 1 }, "Quoted words.".into()),
                (BlockKind::Preformatted, "  let x = 1;\n    next();".into()),
                (BlockKind::Paragraph, "Name | Value".into()),
                (BlockKind::Paragraph, "Alpha | One".into()),
                (BlockKind::Paragraph, "[diagram] follows the image.".into()),
                (
                    BlockKind::Paragraph,
                    "Empty destination and script destination.".into(),
                ),
                (BlockKind::Paragraph, "Open separately.".into()),
                (BlockKind::Paragraph, "Article footnote stays.".into()),
            ]
        );
    });
}

#[test]
fn hidden_content_disappears_but_an_article_header_and_footer_stay() {
    let h = harness();
    runtime().block_on(async {
        let document = open(&h, "reader.html")
            .await
            .reader()
            .await
            .expect("read the article")
            .document;
        let text: String = document.blocks.iter().map(text).collect();

        assert!(
            text.contains("Article byline stays"),
            "document was {text:?}"
        );
        assert!(
            text.contains("Article footnote stays"),
            "document was {text:?}"
        );
        for excluded in [
            "Site masthead",
            "Site navigation",
            "Sidebar",
            "Site footer",
            "Hidden attribute",
            "Aria hidden",
            "Display none",
            "Visibility hidden",
            "Transparent text",
        ] {
            assert!(
                !text.contains(excluded),
                "{excluded:?} remained in {text:?}"
            );
        }
    });
}

#[test]
fn links_are_absolute_and_retain_target_behavior() {
    let h = harness();
    runtime().block_on(async {
        let document = open(&h, "reader.html")
            .await
            .reader()
            .await
            .expect("read the article")
            .document;

        assert_eq!(document.links.len(), 2, "links were {:?}", document.links);
        assert!(document.links[0].url.ends_with("/destination.html"));
        assert!(!document.links[0].new_tab);
        assert!(document.links[1].url.ends_with("/new-tab.html"));
        assert!(document.links[1].new_tab);

        let linked = document
            .blocks
            .iter()
            .flat_map(|block| &block.spans)
            .find(|span| span.text == "linked words")
            .expect("the nested inline link is one span");
        assert_eq!(linked.link, Some(LinkId(0)));
    });
}

#[test]
fn empty_and_script_destinations_are_text_but_not_targets() {
    let h = harness();
    runtime().block_on(async {
        let document = open(&h, "reader.html")
            .await
            .reader()
            .await
            .expect("read the article")
            .document;
        let invalid = document
            .blocks
            .iter()
            .find(|block| text(block).contains("Empty destination"))
            .expect("invalid-link paragraph");

        assert!(invalid.spans.iter().all(|span| span.link.is_none()));
    });
}

#[test]
fn reader_and_ordinary_extraction_report_the_same_status() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "reader.html").await;

        let reader = page.reader().await.expect("read the article");
        let ordinary = page.extract().await.expect("extract the page");

        assert_eq!(reader.status, ordinary.status);
    });
}

#[test]
fn a_page_without_readable_blocks_reports_a_stable_error() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "reader-body.html").await;
        page.eval("document.body.replaceChildren()")
            .await
            .expect("empty the page");

        let error = page
            .reader()
            .await
            .expect_err("empty reader content must fail");

        assert_eq!(error.to_string(), "no readable content");
    });
}
