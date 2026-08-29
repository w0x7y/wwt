use crate::effect::Source;
use crate::event::Failure;
use crate::tab::{Presence, ReaderState, Tab};
use wwt_frame::HintTarget;
use wwt_page::{Extraction, ReaderExtraction, Status};
use wwt_reader::Layout;
use wwt_ui::chrome::State;

const CHROME_ERROR_SCHEME: &str = "chrome-error://";

pub(crate) struct PageLifecycle<'a> {
    tab: &'a mut Tab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadDemand {
    pub focused: bool,
    pub pixel: bool,
    pub columns: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadRequest {
    Extract(Source),
    Reader,
    Status,
}

pub(crate) enum ReadResult {
    Extracted(Source, Result<Extraction, Failure>),
    Reader(Result<ReaderExtraction, Failure>),
    Status(Result<Status, Failure>),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PageOutcome {
    pub next: Option<ReadRequest>,
    pub save: bool,
    pub reader_became_active: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum HintRequest {
    Cached(Vec<HintTarget>),
    Query(Source),
}

impl<'a> PageLifecycle<'a> {
    pub(crate) fn new(tab: &'a mut Tab) -> Self {
        Self { tab }
    }

    pub(crate) fn changed(&mut self) {
        self.tab.dirty = true;
        self.tab.reader.dirty = true;
        self.tab.hints = None;
    }

    pub(crate) fn live_changed(&mut self) {
        self.tab.dirty = true;
    }

    pub(crate) fn detach(&mut self) {
        self.tab.presence = Presence::Detached;
        self.tab.reading = false;
        self.tab.navigating = false;
        self.tab.hinting = false;
        self.tab.hints = None;
        self.tab.degraded = false;
        self.tab.dirty = true;
        self.tab.reader.dirty = true;
    }

    pub(crate) fn replace_document(&mut self) {
        self.tab.reader = ReaderState::default();
        self.tab.hints = None;
    }

    pub(crate) fn begin_navigation(&mut self) -> bool {
        if self.tab.navigating {
            return false;
        }
        self.replace_document();
        self.tab.navigating = true;
        self.tab.degraded = false;
        self.tab.state = State::Loading;
        true
    }

    pub(crate) fn begin_reattach(&mut self) -> bool {
        if self.tab.presence != Presence::Detached {
            return false;
        }
        self.tab.presence = Presence::Opening;
        self.tab.navigating = true;
        self.tab.state = State::Loading;
        true
    }

    pub(crate) fn navigation_settled(&mut self) {
        self.tab.navigating = false;
        self.tab.state = State::Ready;
        self.changed();
    }

    pub(crate) fn operation_failed(&mut self, failure: Failure) {
        self.tab.reading = false;
        self.tab.navigating = false;
        self.tab.state = match failure {
            Failure::TimedOut => State::Stalled,
            Failure::Failed(message) => State::Error(message),
        };
    }

    pub(crate) fn begin_hints(&mut self) -> Option<HintRequest> {
        if let Some(targets) = &self.tab.hints {
            return Some(HintRequest::Cached(targets.clone()));
        }
        if self.tab.hinting || !self.tab.attached() {
            return None;
        }
        self.tab.hinting = true;
        let source = if self.tab.degraded {
            Source::Snapshot
        } else {
            Source::Script
        };
        Some(HintRequest::Query(source))
    }

    pub(crate) fn complete_hints(
        &mut self,
        result: Result<Vec<HintTarget>, Failure>,
    ) -> Result<Vec<HintTarget>, Failure> {
        self.tab.hinting = false;
        match &result {
            Ok(targets) => self.tab.hints = Some(targets.clone()),
            Err(Failure::TimedOut) => self.tab.state = State::Stalled,
            Err(Failure::Failed(message)) => self.tab.state = State::Error(message.clone()),
        }
        result
    }

    pub(crate) fn begin_read(&mut self, demand: ReadDemand) -> Option<ReadRequest> {
        if !self.tab.attached() || self.tab.reading {
            return None;
        }
        if self.tab.reader.wanted || self.tab.reader.active {
            if !demand.focused || !self.tab.reader.dirty {
                return None;
            }
            self.tab.reading = true;
            self.tab.reader.dirty = false;
            return Some(ReadRequest::Reader);
        }
        if !self.tab.dirty || (!demand.focused && self.tab.read) {
            return None;
        }
        self.tab.reading = true;
        self.tab.dirty = false;
        if demand.focused && demand.pixel && self.tab.read && !self.tab.degraded {
            Some(ReadRequest::Status)
        } else {
            let source = if self.tab.degraded {
                Source::Snapshot
            } else {
                Source::Script
            };
            Some(ReadRequest::Extract(source))
        }
    }

    pub(crate) fn complete(&mut self, result: ReadResult, demand: ReadDemand) -> PageOutcome {
        self.tab.reading = false;
        let mut outcome = PageOutcome::default();

        match result {
            ReadResult::Extracted(source, result) => match result {
                Ok(extraction) => {
                    self.tab.read = true;
                    self.tab.runs = extraction.runs;
                    self.tab.caret = extraction.caret;
                    outcome.save = self.apply_status(extraction.status);
                    outcome.next = self.begin_read(demand);
                }
                Err(Failure::TimedOut) => self.tab.state = State::Stalled,
                Err(failure) => match source {
                    Source::Script => {
                        self.tab.degraded = true;
                        self.tab.dirty = true;
                        outcome.next = self.begin_read(demand);
                    }
                    Source::Snapshot => self.tab.state = State::Error(failure.message()),
                },
            },
            ReadResult::Reader(result) => match result {
                Ok(extraction) => {
                    let was_active = self.tab.reader.active;
                    if extraction.document.blocks.is_empty() {
                        self.tab.state = State::Notice("nothing to read".to_string());
                        if !was_active {
                            self.tab.reader.wanted = false;
                        }
                        outcome.next = self.begin_read(demand);
                        return outcome;
                    }

                    let document = extraction.document;
                    let layout = Layout::new(&document, demand.columns);
                    let max_top = layout.rows().saturating_sub(usize::from(demand.rows));
                    self.tab.reader.top_row = self.tab.reader.top_row.min(max_top);
                    self.tab.reader.document = Some(document);
                    self.tab.reader.layout = Some(layout);
                    self.tab.reader.active = self.tab.reader.wanted;
                    outcome.reader_became_active = !was_active && self.tab.reader.active;
                    outcome.save = self.apply_status(extraction.status);
                    outcome.next = self.begin_read(demand);
                }
                Err(failure) => {
                    self.tab.state = match failure {
                        Failure::TimedOut => State::Stalled,
                        Failure::Failed(message) => State::Error(message),
                    };
                    if !self.tab.reader.active {
                        self.tab.reader.wanted = false;
                        self.tab.reader.dirty = true;
                    }
                    outcome.next = self.begin_read(demand);
                }
            },
            ReadResult::Status(result) => match result {
                Ok(status) => {
                    outcome.save = self.apply_status(status);
                    outcome.next = self.begin_read(demand);
                }
                Err(Failure::TimedOut) => self.tab.state = State::Stalled,
                Err(Failure::Failed(_)) => {
                    self.tab.degraded = true;
                    self.tab.dirty = true;
                    outcome.next = self.begin_read(demand);
                }
            },
        }

        outcome
    }

    fn apply_status(&mut self, status: Status) -> bool {
        let was = (
            self.tab.url.clone(),
            self.tab.title.clone(),
            self.tab.scroll_y,
        );
        self.tab.progress = status.scroll_progress();
        self.tab.scroll_y = status.scroll_y;
        self.tab.title = status.title;

        if status.url.starts_with(CHROME_ERROR_SCHEME) {
            self.tab.state = State::Error("could not be reached".to_string());
        } else {
            self.tab.url = status.url;
            if !self.tab.navigating {
                self.tab.state = State::Ready;
            }
        }

        was != (
            self.tab.url.clone(),
            self.tab.title.clone(),
            self.tab.scroll_y,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tab::{Presence, Tab, TabId};

    fn attached() -> Tab {
        let mut tab = Tab::new(TabId(0), "https://example.com".to_string());
        tab.presence = Presence::Attached;
        tab
    }

    fn text() -> ReadDemand {
        ReadDemand {
            focused: true,
            pixel: false,
            columns: 80,
            rows: 22,
        }
    }

    #[test]
    fn one_read_slot_refuses_a_second_request() {
        let mut tab = attached();
        assert_eq!(
            PageLifecycle::new(&mut tab).begin_read(text()),
            Some(ReadRequest::Extract(Source::Script))
        );
        tab.reader.wanted = true;
        tab.reader.dirty = true;
        assert_eq!(PageLifecycle::new(&mut tab).begin_read(text()), None);
    }

    #[test]
    fn a_dirty_signal_during_a_read_is_spent_after_completion() {
        let mut tab = attached();
        assert!(PageLifecycle::new(&mut tab).begin_read(text()).is_some());
        PageLifecycle::new(&mut tab).changed();
        assert!(tab.reading);
        assert!(tab.dirty);
    }

    #[test]
    fn a_clean_active_reader_does_not_fall_through_to_a_live_read() {
        let mut tab = attached();
        tab.reader.active = true;
        tab.reader.dirty = false;
        tab.dirty = true;
        assert_eq!(PageLifecycle::new(&mut tab).begin_read(text()), None);
    }

    #[test]
    fn a_script_failure_selects_snapshot_once() {
        let mut tab = attached();
        assert!(PageLifecycle::new(&mut tab).begin_read(text()).is_some());
        let outcome = PageLifecycle::new(&mut tab).complete(
            ReadResult::Extracted(
                Source::Script,
                Err(Failure::Failed("script broke".to_string())),
            ),
            text(),
        );
        assert!(tab.degraded);
        assert_eq!(outcome.next, Some(ReadRequest::Extract(Source::Snapshot)));
    }

    #[test]
    fn a_timeout_stalls_without_selecting_snapshot() {
        let mut tab = attached();
        assert!(PageLifecycle::new(&mut tab).begin_read(text()).is_some());
        let outcome = PageLifecycle::new(&mut tab).complete(
            ReadResult::Extracted(Source::Script, Err(Failure::TimedOut)),
            text(),
        );
        assert_eq!(tab.state, State::Stalled);
        assert!(!tab.degraded);
        assert_eq!(outcome.next, None);
    }

    #[test]
    fn detachment_keeps_content_and_clears_in_flight_work() {
        let mut tab = attached();
        tab.title = "cached title".to_string();
        tab.reading = true;
        tab.navigating = true;
        tab.hinting = true;
        PageLifecycle::new(&mut tab).detach();
        assert_eq!(tab.presence, Presence::Detached);
        assert_eq!(tab.title, "cached title");
        assert!(!tab.reading);
        assert!(!tab.navigating);
        assert!(!tab.hinting);
        assert!(tab.dirty);
        assert!(tab.reader.dirty);
    }

    #[test]
    fn navigation_replaces_document_state_without_blank_content() {
        let mut tab = attached();
        tab.degraded = true;
        tab.title = "cached title".to_string();
        assert!(PageLifecycle::new(&mut tab).begin_navigation());
        assert!(tab.navigating);
        assert!(!tab.degraded);
        assert_eq!(tab.state, State::Loading);
        assert_eq!(tab.title, "cached title");
        assert!(tab.reader.document.is_none());
    }
}
