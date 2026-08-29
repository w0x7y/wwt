//! Pixel preference, screencast reconciliation, and the retained picture.
//!
//! `Session` supplies the focused page as a small projection and translates
//! the requests returned here into effects. This module owns the lifecycle
//! policy that decides which target should screencast, accepts or discards
//! frames, and paints the last accepted picture.

use std::sync::Arc;

use wwt_frame::{CellRect, Frame, Image, Samples, Viewport};
use wwt_page::ScreencastFrame;

use crate::effect::FrameSize;
use crate::tab::TabId;

pub(crate) struct PixelPresentation {
    output: PixelOutput,
    mode: PixelMode,
    requested: Option<RequestedScreencast>,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PixelOutput {
    Graphics,
    HalfBlocks,
}

#[derive(Debug, Clone, PartialEq)]
enum PixelMode {
    Text,
    Pixel { picture: Option<Picture> },
}

#[derive(Debug, Clone, PartialEq)]
enum Picture {
    Graphics(Image),
    Blocks(Samples),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RequestedScreencast {
    tab: TabId,
    size: FrameSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FocusedPage {
    pub(crate) id: TabId,
    pub(crate) attached: bool,
    pub(crate) reader_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PresentationRequest {
    Start(TabId, FrameSize),
    Stop(TabId),
    Ack(TabId, i64),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PixelOutcome {
    pub(crate) changed: bool,
    pub(crate) refresh_live_runs: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FrameOutcome {
    pub(crate) request: Option<PresentationRequest>,
    pub(crate) notice: Option<String>,
}

impl PixelPresentation {
    pub(crate) fn new() -> Self {
        Self {
            output: PixelOutput::HalfBlocks,
            mode: PixelMode::Text,
            requested: None,
            generation: 0,
        }
    }

    pub(crate) fn set_output(&mut self, output: PixelOutput) {
        self.output = output;
    }

    pub(crate) fn set_enabled(&mut self, on: bool) -> PixelOutcome {
        if on == self.enabled() {
            return PixelOutcome::default();
        }

        self.mode = if on {
            PixelMode::Pixel { picture: None }
        } else {
            PixelMode::Text
        };
        PixelOutcome {
            changed: true,
            refresh_live_runs: !on,
        }
    }

    pub(crate) fn reconcile(
        &mut self,
        focused: FocusedPage,
        viewport: Viewport,
    ) -> Vec<PresentationRequest> {
        let desired = self.desired(focused, viewport);
        if desired == self.requested {
            return Vec::new();
        }

        let mut requests = Vec::with_capacity(2);
        if let Some(old) = self.requested {
            requests.push(PresentationRequest::Stop(old.tab));
        }
        if let Some(new) = desired {
            requests.push(PresentationRequest::Start(new.tab, new.size));
        }
        self.requested = desired;
        requests
    }

    pub(crate) fn restart(
        &mut self,
        focused: FocusedPage,
        viewport: Viewport,
    ) -> Vec<PresentationRequest> {
        if let PixelMode::Pixel {
            picture: Some(Picture::Graphics(image)),
        } = &mut self.mode
        {
            self.generation += 1;
            image.generation = self.generation;
            image.area = CellRect::of(viewport.grid(), viewport.origin_row());
        }

        let desired = self.desired(focused, viewport);
        let Some(desired) = desired else {
            return self.reconcile(focused, viewport);
        };

        let mut requests = Vec::with_capacity(2);
        if let Some(old) = self.requested {
            requests.push(PresentationRequest::Stop(old.tab));
        }
        requests.push(PresentationRequest::Start(desired.tab, desired.size));
        self.requested = Some(desired);
        requests
    }

    pub(crate) fn forget(&mut self, id: TabId) {
        if self.requested.is_some_and(|requested| requested.tab == id) {
            self.requested = None;
        }
    }

    pub(crate) fn stop(&mut self) -> Option<PresentationRequest> {
        self.requested
            .take()
            .map(|requested| PresentationRequest::Stop(requested.tab))
    }

    pub(crate) fn enabled(&self) -> bool {
        matches!(self.mode, PixelMode::Pixel { .. })
    }

    pub(crate) fn accept_frame(
        &mut self,
        source: TabId,
        source_exists: bool,
        focused: FocusedPage,
        frame: ScreencastFrame,
        viewport: Viewport,
    ) -> FrameOutcome {
        if !source_exists {
            return FrameOutcome::default();
        }

        let mut outcome = FrameOutcome {
            request: Some(PresentationRequest::Ack(source, frame.ack)),
            notice: None,
        };
        let accepts = source == focused.id
            && focused.attached
            && !focused.reader_active
            && self.enabled()
            && self
                .requested
                .is_some_and(|requested| requested.tab == source);
        if !accepts {
            return outcome;
        }

        let picture = match self.output {
            PixelOutput::Graphics => {
                self.generation += 1;
                Some(Picture::Graphics(Image {
                    generation: self.generation,
                    payload: Arc::new(frame.data),
                    area: CellRect::of(viewport.grid(), viewport.origin_row()),
                }))
            }
            PixelOutput::HalfBlocks => {
                let grid = viewport.grid();
                wwt_png::decode_base64(&frame.data).ok().and_then(|png| {
                    Samples::resampled(png.width, png.height, &png.pixels, grid.cols, grid.rows * 2)
                        .map(Picture::Blocks)
                })
            }
        };

        match picture {
            Some(picture) => {
                if let PixelMode::Pixel { picture: stored } = &mut self.mode {
                    *stored = Some(picture);
                }
            }
            None => outcome.notice = Some("that picture could not be read".to_string()),
        }
        outcome
    }

    pub(crate) fn paint(&self, frame: &mut Frame, viewport: Viewport) {
        let PixelMode::Pixel {
            picture: Some(picture),
        } = &self.mode
        else {
            return;
        };
        match picture {
            Picture::Graphics(image) => frame.set_image(Some(image.clone())),
            Picture::Blocks(samples) => frame.paint_samples(
                CellRect::of(viewport.grid(), viewport.origin_row()),
                samples,
            ),
        }
    }

    fn desired(&self, focused: FocusedPage, viewport: Viewport) -> Option<RequestedScreencast> {
        (self.enabled() && focused.attached && !focused.reader_active).then(|| {
            RequestedScreencast {
                tab: focused.id,
                size: self.frame_size(viewport),
            }
        })
    }

    fn frame_size(&self, viewport: Viewport) -> FrameSize {
        match self.output {
            PixelOutput::Graphics => FrameSize {
                width: viewport.css_width(),
                height: viewport.css_height(),
            },
            PixelOutput::HalfBlocks => {
                let grid = viewport.grid();
                FrameSize {
                    width: u32::from(grid.cols) * 2,
                    height: u32::from(grid.rows) * 4,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wwt_frame::{CellPos, CellSize, Frame, GridSize, Viewport};
    use wwt_page::ScreencastFrame;

    const TAB_0: TabId = TabId(0);
    const TAB_1: TabId = TabId(1);

    fn viewport() -> Viewport {
        Viewport::with_origin(GridSize { cols: 80, rows: 22 }, CellSize { w: 9, h: 20 }, 1)
    }

    fn page(id: TabId) -> FocusedPage {
        FocusedPage {
            id,
            attached: true,
            reader_active: false,
        }
    }

    fn larger_viewport() -> Viewport {
        Viewport::with_origin(
            GridSize {
                cols: 100,
                rows: 28,
            },
            CellSize { w: 9, h: 20 },
            1,
        )
    }

    fn frame(data: &str, ack: i64) -> ScreencastFrame {
        ScreencastFrame {
            data: data.to_string(),
            ack,
        }
    }

    fn fixture_frame() -> ScreencastFrame {
        ScreencastFrame {
            data: include_str!("../../wwt-png/tests/fixtures/screencast.txt")
                .trim()
                .to_string(),
            ack: 1,
        }
    }

    fn graphics_pixel() -> PixelPresentation {
        let mut pixel = PixelPresentation::new();
        pixel.set_output(PixelOutput::Graphics);
        pixel.set_enabled(true);
        pixel.reconcile(page(TAB_0), viewport());
        pixel
    }

    fn half_block_pixel() -> PixelPresentation {
        let mut pixel = PixelPresentation::new();
        pixel.set_enabled(true);
        pixel.reconcile(page(TAB_0), viewport());
        pixel
    }

    fn painted(pixel: &PixelPresentation) -> Frame {
        let mut frame = Frame::new(GridSize { cols: 80, rows: 24 });
        pixel.paint(&mut frame, viewport());
        frame
    }

    #[test]
    fn text_mode_requests_no_screencast() {
        let mut pixel = PixelPresentation::new();
        assert_eq!(pixel.reconcile(page(TAB_0), viewport()), vec![]);
    }

    #[test]
    fn enabling_starts_only_an_attached_live_page() {
        let mut pixel = PixelPresentation::new();
        assert_eq!(
            pixel.set_enabled(true),
            PixelOutcome {
                changed: true,
                refresh_live_runs: false,
            }
        );
        assert!(matches!(
            pixel.reconcile(page(TAB_0), viewport()).as_slice(),
            [PresentationRequest::Start(TAB_0, _)]
        ));

        let mut reader = PixelPresentation::new();
        reader.set_enabled(true);
        assert_eq!(
            reader.reconcile(
                FocusedPage {
                    id: TAB_0,
                    attached: true,
                    reader_active: true,
                },
                viewport(),
            ),
            vec![]
        );

        let mut opening = PixelPresentation::new();
        opening.set_enabled(true);
        assert_eq!(
            opening.reconcile(
                FocusedPage {
                    id: TAB_0,
                    attached: false,
                    reader_active: false,
                },
                viewport(),
            ),
            vec![]
        );
    }

    #[test]
    fn focus_change_stops_before_it_starts() {
        let mut pixel = PixelPresentation::new();
        pixel.set_enabled(true);
        pixel.reconcile(page(TAB_0), viewport());
        assert!(matches!(
            pixel.reconcile(page(TAB_1), viewport()).as_slice(),
            [
                PresentationRequest::Stop(TAB_0),
                PresentationRequest::Start(TAB_1, _)
            ]
        ));
    }

    #[test]
    fn forgetting_a_gone_target_emits_no_stop() {
        let mut pixel = PixelPresentation::new();
        pixel.set_enabled(true);
        pixel.reconcile(page(TAB_0), viewport());
        pixel.forget(TAB_0);
        assert_eq!(pixel.stop(), None);
    }

    #[test]
    fn resize_restarts_at_the_new_size() {
        let mut pixel = PixelPresentation::new();
        pixel.set_enabled(true);
        pixel.reconcile(page(TAB_0), viewport());
        let larger = Viewport::with_origin(
            GridSize {
                cols: 100,
                rows: 28,
            },
            CellSize { w: 9, h: 20 },
            1,
        );
        assert!(matches!(
            pixel.restart(page(TAB_0), larger).as_slice(),
            [
                PresentationRequest::Stop(TAB_0),
                PresentationRequest::Start(TAB_0, _)
            ]
        ));
    }

    #[test]
    fn disabling_clears_once_and_refreshes_live_runs() {
        let mut pixel = PixelPresentation::new();
        assert_eq!(pixel.set_enabled(false), PixelOutcome::default());
        pixel.set_enabled(true);
        assert_eq!(
            pixel.set_enabled(false),
            PixelOutcome {
                changed: true,
                refresh_live_runs: true,
            }
        );
        assert_eq!(pixel.set_enabled(false), PixelOutcome::default());
        assert!(!pixel.enabled());
    }

    #[test]
    fn stop_returns_the_requested_target_once() {
        let mut pixel = PixelPresentation::new();
        pixel.set_enabled(true);
        pixel.reconcile(page(TAB_0), viewport());
        assert_eq!(pixel.stop(), Some(PresentationRequest::Stop(TAB_0)));
        assert_eq!(pixel.stop(), None);
    }

    #[test]
    fn a_closed_source_is_neither_acked_nor_painted() {
        let mut pixel = graphics_pixel();
        let outcome = pixel.accept_frame(TAB_0, false, page(TAB_1), frame("GONE", 7), viewport());
        assert_eq!(outcome, FrameOutcome::default());
        assert!(painted(&pixel).image().is_none());
    }

    #[test]
    fn an_existing_hidden_frame_is_acked_and_discarded() {
        let mut pixel = graphics_pixel();
        let outcome = pixel.accept_frame(TAB_1, true, page(TAB_0), frame("STALE", 7), viewport());
        assert_eq!(outcome.request, Some(PresentationRequest::Ack(TAB_1, 7)));
        assert!(painted(&pixel).image().is_none());
    }

    #[test]
    fn a_reader_frame_is_acked_and_discarded() {
        let mut pixel = graphics_pixel();
        let reader = FocusedPage {
            id: TAB_0,
            attached: true,
            reader_active: true,
        };
        let outcome = pixel.accept_frame(TAB_0, true, reader, frame("READER", 7), viewport());
        assert_eq!(outcome.request, Some(PresentationRequest::Ack(TAB_0, 7)));
        assert!(painted(&pixel).image().is_none());
    }

    #[test]
    fn an_existing_frame_after_disable_is_acked_and_discarded() {
        let mut pixel = graphics_pixel();
        pixel.set_enabled(false);
        let outcome = pixel.accept_frame(TAB_0, true, page(TAB_0), frame("LATE", 7), viewport());
        assert_eq!(outcome.request, Some(PresentationRequest::Ack(TAB_0, 7)));
        assert!(painted(&pixel).image().is_none());
    }

    #[test]
    fn graphics_keep_the_payload_and_advance_generation() {
        let mut pixel = graphics_pixel();
        pixel.accept_frame(TAB_0, true, page(TAB_0), frame("SAME", 1), viewport());
        let first = painted(&pixel).image().expect("first image").clone();
        pixel.accept_frame(TAB_0, true, page(TAB_0), frame("SAME", 2), viewport());
        let second = painted(&pixel).image().expect("second image").clone();
        assert_eq!(second.payload.as_str(), "SAME");
        assert_ne!(first.generation, second.generation);
    }

    #[test]
    fn half_blocks_decode_and_paint_the_fixture() {
        let mut pixel = half_block_pixel();
        pixel.accept_frame(TAB_0, true, page(TAB_0), fixture_frame(), viewport());
        let frame = painted(&pixel);
        assert_eq!(
            frame.cell(CellPos { col: 0, row: 1 }).expect("painted").ch,
            '\u{2580}'
        );
        assert!(frame.image().is_none());
    }

    #[test]
    fn a_bad_half_block_frame_keeps_the_previous_picture() {
        let mut pixel = half_block_pixel();
        pixel.accept_frame(TAB_0, true, page(TAB_0), fixture_frame(), viewport());
        let outcome = pixel.accept_frame(
            TAB_0,
            true,
            page(TAB_0),
            frame("not a picture", 7),
            viewport(),
        );
        assert_eq!(outcome.request, Some(PresentationRequest::Ack(TAB_0, 7)));
        assert_eq!(
            outcome.notice.as_deref(),
            Some("that picture could not be read")
        );
        assert_eq!(
            painted(&pixel)
                .cell(CellPos { col: 0, row: 1 })
                .expect("old picture")
                .ch,
            '\u{2580}'
        );
    }

    #[test]
    fn resize_moves_a_graphics_picture_and_advances_generation() {
        let mut pixel = graphics_pixel();
        pixel.accept_frame(TAB_0, true, page(TAB_0), frame("IMAGE", 1), viewport());
        let before = painted(&pixel).image().expect("image").generation;
        let larger = larger_viewport();
        pixel.restart(page(TAB_0), larger);
        let mut frame = Frame::new(GridSize {
            cols: 100,
            rows: 30,
        });
        pixel.paint(&mut frame, larger);
        let image = frame.image().expect("resized image");
        assert_eq!(image.area.cols, 100);
        assert_eq!(image.area.rows, 28);
        assert_ne!(image.generation, before);
    }
}
