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

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::json;
use wwt_frame::{CssRect, Style, TextRun, Viewport};

use crate::color::parse_css_color;
use crate::extract::{Extraction, Page};

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Layout {
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
        Ok(Extraction {
            runs: runs(&snapshot, document, viewport_height),
            // A caret needs character positions inside a control, which
            // needs the mirror, which is script machinery. Insert mode
            // still types; it types blind.
            caret: None,
            title: snapshot.string(document.title).to_string(),
            url: snapshot.string(document.document_url).to_string(),
            scroll_y: document.scroll_offset_y,
            scroll_height: document.content_height,
            viewport_height,
        })
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
