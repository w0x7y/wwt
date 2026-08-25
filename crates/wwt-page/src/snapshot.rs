//! Reading a page without our script in it.
//!
//! `DOMSnapshot.captureSnapshot` returns the document as parallel arrays
//! indexed into a string table, which is a shape chosen for size on the
//! wire rather than for reading. All of this file is turning that into the
//! `Extraction` the script's path returns, so that nothing downstream can
//! tell which produced it.
//!
//! It shares no code with `bootstrap.js` on purpose. That is what makes it
//! a fallback rather than a second entry point to the same bug.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::json;
use wwt_frame::{CssRect, HintTarget, Style, TargetKind, TextRun, Viewport};

use crate::color::parse_css_color;
use crate::extract::{Extraction, Page, Status};

/// The fraction of the font size a baseline sits above the text box's
/// bottom. `bootstrap.js` states the same number once and calls it
/// `DESCENDER`; the two paths have to agree on it or a fallback reads the
/// page correctly and paints it a row off.
const DESCENDER: f64 = 0.21;

/// The computed styles the query asks for, in the order the answers arrive.
///
/// Two of them are the style. The other two are not: `font-size` is what
/// the baseline is computed from, and `visibility` is what culls a node the
/// browser does not show but the snapshot still reports.
const STYLES: [&str; 4] = ["color", "font-weight", "font-size", "visibility"];
const COLOUR: usize = 0;
const WEIGHT: usize = 1;
const FONT_SIZE: usize = 2;
const VISIBILITY: usize = 3;

/// A field that is set for only a few nodes, sent as the indices that have
/// one and the values they have.
#[derive(Debug, Default, Deserialize)]
struct RareStrings {
    #[serde(default)]
    index: Vec<usize>,
    #[serde(default)]
    value: Vec<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct RareBools {
    #[serde(default)]
    index: Vec<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Nodes {
    #[serde(default)]
    node_name: Vec<i64>,
    #[serde(default)]
    attributes: Vec<Vec<i64>>,
    #[serde(default)]
    input_value: RareStrings,
    #[serde(default)]
    text_value: RareStrings,
    #[serde(default)]
    is_clickable: RareBools,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Layout {
    #[serde(default)]
    node_index: Vec<usize>,
    #[serde(default)]
    bounds: Vec<Vec<f64>>,
    #[serde(default)]
    styles: Vec<Vec<i64>>,
    #[serde(default)]
    text: Vec<i64>,
    #[serde(default)]
    paint_orders: Vec<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextBoxes {
    #[serde(default)]
    layout_index: Vec<usize>,
    #[serde(default)]
    bounds: Vec<Vec<f64>>,
    #[serde(default)]
    start: Vec<i64>,
    #[serde(default)]
    length: Vec<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    // Not `documentUrl`: the protocol capitalises the acronym, so the
    // camelCase rule below cannot produce this one name.
    #[serde(rename = "documentURL")]
    document_url: i64,
    title: i64,
    #[serde(default)]
    nodes: Nodes,
    layout: Layout,
    text_boxes: TextBoxes,
    #[serde(default)]
    scroll_offset_x: f64,
    #[serde(default)]
    scroll_offset_y: f64,
    #[serde(default)]
    content_height: f64,
}

#[derive(Debug, Deserialize)]
struct Snapshot {
    documents: Vec<Document>,
    strings: Vec<String>,
}

impl Snapshot {
    /// A string by index. `-1` means "no string", which is how every
    /// optional string in this protocol is spelled.
    fn string(&self, index: i64) -> &str {
        usize::try_from(index)
            .ok()
            .and_then(|index| self.strings.get(index))
            .map_or("", String::as_str)
    }
}

impl Page {
    /// Read the page without running anything of ours in it.
    ///
    /// The second source, for a tab whose injected script threw. It costs
    /// more than `extract` does and is not an alternative to it: the
    /// snapshot is the whole document, so the work is proportional to the
    /// page rather than to what is on screen. See section 11 of the M6
    /// spec, which accepts that rather than solving it.
    pub async fn snapshot(&self, vp: Viewport) -> Result<Extraction> {
        let value = self
            .client()
            .call_on(
                self.session_id(),
                "DOMSnapshot.captureSnapshot",
                json!({
                    "computedStyles": STYLES,
                    // Fills TextRun::z, which the painter's algorithm needs
                    // to resolve a cell two runs both cover.
                    "includePaintOrder": true,
                    "includeDOMRects": false,
                }),
            )
            .await
            .context("capture a DOM snapshot")?;

        let snapshot: Snapshot =
            serde_json::from_value(value).context("the DOM snapshot had an unexpected shape")?;
        let document = snapshot
            .documents
            .first()
            .ok_or_else(|| anyhow!("the DOM snapshot contained no document"))?;

        let viewport_height = f64::from(vp.css_height());
        // A control's value is not a text box, so it takes a second pass
        // over the same answer rather than a second call.
        let mut all = runs(&snapshot, document, viewport_height);
        all.extend(field_runs(&snapshot, document, viewport_height));

        Ok(Extraction {
            runs: all,
            // A caret needs character positions inside a control, which
            // needs the mirror, which is script machinery. Insert mode
            // still types; it types blind.
            caret: None,
            status: Status {
                title: snapshot.string(document.title).to_string(),
                url: snapshot.string(document.document_url).to_string(),
                scroll_y: document.scroll_offset_y,
                scroll_height: document.content_height,
                viewport_height,
            },
        })
    }
}

impl Page {
    /// The interactive boxes, without running anything of ours.
    ///
    /// `isClickable` is Chromium's own answer to the question the script
    /// asks with a tag sweep, so this is the rare place where the fallback
    /// is the simpler of the two.
    ///
    /// What it cannot do is the occlusion test: the script hit-tests a
    /// candidate before labelling it, and a snapshot has nothing to hit
    /// test with, so a link behind a modal can still get a label here. A
    /// spurious label costs a keystroke; the alternative is a round trip
    /// per candidate.
    pub async fn snapshot_hints(&self, vp: Viewport) -> Result<Vec<HintTarget>> {
        let value = self
            .client()
            .call_on(
                self.session_id(),
                "DOMSnapshot.captureSnapshot",
                // No styles and no paint order: a hint is a box and a kind,
                // and neither is a question about how the box is painted.
                json!({
                    "computedStyles": [],
                    "includePaintOrder": false,
                    "includeDOMRects": false,
                }),
            )
            .await
            .context("capture a DOM snapshot for hints")?;

        let snapshot: Snapshot =
            serde_json::from_value(value).context("the DOM snapshot had an unexpected shape")?;
        let document = snapshot
            .documents
            .first()
            .ok_or_else(|| anyhow!("the DOM snapshot contained no document"))?;

        let viewport_height = f64::from(vp.css_height());
        let layout = &document.layout;
        let layout_of = layout_index_by_node(layout);

        let editable = |node_index: usize| {
            matches!(
                snapshot.string(document.nodes.node_name.get(node_index).copied().unwrap_or(-1)),
                "INPUT" | "TEXTAREA" | "SELECT"
            )
        };

        // A control is worth hinting whether or not Chromium calls it
        // clickable, because hinting one is how insert mode is entered.
        let candidates = document
            .nodes
            .is_clickable
            .index
            .iter()
            .copied()
            .chain((0..document.nodes.node_name.len()).filter(|&index| editable(index)));

        let mut targets = Vec::new();
        let mut seen = HashSet::new();
        for node_index in candidates {
            if !seen.insert(node_index) {
                continue;
            }
            let Some(&layout_index) = layout_of.get(&node_index) else { continue };
            let Some(bounds) = layout.bounds.get(layout_index) else { continue };
            let Some(rect) = rect_of(bounds, document) else { continue };
            if rect.w <= 0.0 || rect.h <= 0.0 {
                continue;
            }
            if rect.y + rect.h <= 0.0 || rect.y >= viewport_height {
                continue;
            }
            targets.push(HintTarget {
                rect,
                kind: if editable(node_index) {
                    TargetKind::Editable
                } else {
                    TargetKind::Clickable
                },
            });
        }

        Ok(targets)
    }
}

fn runs(snapshot: &Snapshot, document: &Document, viewport_height: f64) -> Vec<TextRun> {
    let boxes = &document.text_boxes;
    let layout = &document.layout;
    let mut runs = Vec::new();

    for (index, &layout_index) in boxes.layout_index.iter().enumerate() {
        let (Some(bounds), Some(&start), Some(&length)) =
            (boxes.bounds.get(index), boxes.start.get(index), boxes.length.get(index))
        else {
            continue;
        };
        let Some(&text_index) = layout.text.get(layout_index) else { continue };
        let text = slice_utf16(snapshot.string(text_index), start, length);
        if text.trim().is_empty() {
            continue;
        }

        let styles = Styles::of(snapshot, layout, layout_index);
        // The cheap question first, as every pass over a document here
        // does: a string compare before any arithmetic.
        if !styles.visible() {
            continue;
        }

        let Some(rect) = rect_of(bounds, document) else { continue };
        // Culling is ours: the snapshot is the whole document. Half of the
        // reason the script path costs 4ms and not 18 is that it stops
        // measuring what nobody can see, and this is that rule here.
        if rect.y + rect.h <= 0.0 || rect.y >= viewport_height {
            continue;
        }

        runs.push(TextRun {
            // The script's own rule, applied to the same box. A text box is
            // not merely the tight box: it is the box `getClientRects`
            // returns, to the last fraction of a pixel, which is what lets
            // the two paths agree on a row rather than come close.
            baseline: rect.y + rect.h - styles.font_size() * DESCENDER,
            rect,
            text,
            style: styles.style(),
            z: layout.paint_orders.get(layout_index).copied().unwrap_or(0),
        });
    }

    runs
}

/// The computed styles of one layout node, still as strings.
///
/// A borrow rather than a parsed struct, so that the cheap questions
/// (`visible`) can be asked without paying for the expensive ones.
struct Styles<'a> {
    values: [&'a str; STYLES.len()],
}

impl<'a> Styles<'a> {
    fn of(snapshot: &'a Snapshot, layout: &Layout, index: usize) -> Self {
        let styles = layout.styles.get(index);
        let mut values = [""; STYLES.len()];
        for (at, value) in values.iter_mut().enumerate() {
            *value = styles.and_then(|s| s.get(at)).map_or("", |&i| snapshot.string(i));
        }
        Self { values }
    }

    fn visible(&self) -> bool {
        // Absent counts as visible: a node whose style did not come back is
        // one this cannot judge, and dropping it would lose real text.
        !matches!(self.values[VISIBILITY], "hidden" | "collapse")
    }

    fn font_size(&self) -> f64 {
        self.values[FONT_SIZE]
            .strip_suffix("px")
            .and_then(|size| size.parse().ok())
            .unwrap_or(16.0)
    }

    fn style(&self) -> Style {
        Style {
            fg: parse_css_color(self.values[COLOUR]),
            bg: None,
            // A computed font-weight is a number, whatever the stylesheet
            // said, so `bold` never reaches this comparison as a word.
            bold: self.values[WEIGHT].parse::<f64>().unwrap_or(400.0) >= 600.0,
            reverse: false,
        }
    }
}

/// A text box's rectangle, in viewport coordinates.
///
/// The snapshot's are document coordinates, and everything downstream
/// expects a client rect. Getting this wrong looks right at the top of a
/// page and drifts as you scroll.
fn rect_of(bounds: &[f64], document: &Document) -> Option<CssRect> {
    let (&x, &y, &w, &h) = (bounds.first()?, bounds.get(1)?, bounds.get(2)?, bounds.get(3)?);
    Some(CssRect { x: x - document.scroll_offset_x, y: y - document.scroll_offset_y, w, h })
}

/// The DOM counts offsets in UTF-16 code units, and Rust counts bytes.
///
/// Slicing by `chars` would be right for everything on the basic plane and
/// wrong for an emoji, which is exactly the kind of bug that only appears
/// on somebody else's page.
fn slice_utf16(text: &str, start: i64, length: i64) -> String {
    let (Ok(start), Ok(length)) = (usize::try_from(start), usize::try_from(length)) else {
        return String::new();
    };
    let units: Vec<u16> = text.encode_utf16().collect();
    let end = start.saturating_add(length).min(units.len());
    if start >= end {
        return String::new();
    }
    String::from_utf16_lossy(&units[start..end])
}

/// What a form control is showing, which no text box can say.
///
/// A control's value is element state and not DOM: `input.childNodes` is
/// empty however much you type. The script mirrors the control into a
/// hidden div to measure it; a snapshot cannot, so this paints the value
/// into the control's own box and lets `paint_run` elide it. That is the
/// difference between seeing what you typed and seeing where you typed.
fn field_runs(snapshot: &Snapshot, document: &Document, viewport_height: f64) -> Vec<TextRun> {
    let nodes = &document.nodes;
    let layout = &document.layout;
    let layout_of = layout_index_by_node(layout);

    let values = rare_strings(&nodes.input_value);
    let texts = rare_strings(&nodes.text_value);
    let mut runs = Vec::new();

    for (node_index, &name) in nodes.node_name.iter().enumerate() {
        let name = snapshot.string(name);
        if !matches!(name, "INPUT" | "TEXTAREA") {
            continue;
        }
        let Some(&layout_index) = layout_of.get(&node_index) else { continue };
        let Some(bounds) = layout.bounds.get(layout_index) else { continue };
        let Some(rect) = rect_of(bounds, document) else { continue };
        if rect.y + rect.h <= 0.0 || rect.y >= viewport_height {
            continue;
        }

        let attribute = |wanted: &str| {
            nodes
                .attributes
                .get(node_index)
                .into_iter()
                .flat_map(|pairs| pairs.as_chunks::<2>().0)
                .find(|pair| snapshot.string(pair[0]) == wanted)
                .map(|pair| snapshot.string(pair[1]).to_string())
        };

        let value = values
            .get(&node_index)
            .or_else(|| texts.get(&node_index))
            .map(|&index| snapshot.string(index).to_string())
            .unwrap_or_default();

        let text = if value.is_empty() {
            // What the browser shows, which is the placeholder.
            attribute("placeholder").unwrap_or_default()
        } else if attribute("type").as_deref() == Some("password") {
            // Never the value. The one run in this codebase that must not
            // say what it knows.
            "\u{2022}".repeat(value.chars().count())
        } else {
            value
        };
        if text.is_empty() {
            continue;
        }

        let styles = Styles::of(snapshot, layout, layout_index);
        runs.push(TextRun {
            text,
            baseline: rect.y + rect.h - styles.font_size() * DESCENDER,
            rect,
            style: styles.style(),
            // Above the page's own text: a control is drawn over whatever
            // is behind it, and its value is drawn over the control.
            z: layout.paint_orders.get(layout_index).copied().unwrap_or(0) + 1,
        });
    }

    runs
}

/// The layout tree indexes the node tree, and both passes here need the
/// opposite. Built once rather than searched per control.
fn layout_index_by_node(layout: &Layout) -> HashMap<usize, usize> {
    let mut by_node = HashMap::new();
    for (layout_index, &node_index) in layout.node_index.iter().enumerate() {
        by_node.entry(node_index).or_insert(layout_index);
    }
    by_node
}

/// A rare field as a lookup from node index to its value.
fn rare_strings(rare: &RareStrings) -> HashMap<usize, i64> {
    rare.index.iter().copied().zip(rare.value.iter().copied()).collect()
}
