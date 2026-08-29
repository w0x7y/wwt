//! Browser availability and login-handoff policy.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageWork {
    Request,
    Result,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkDecision {
    Proceed,
    Relaunch,
    Blocked,
}

pub(crate) enum BrowserSignal {
    Lost,
    Back,
    LoginRequested,
    LoginSaved(Result<(), String>),
    LoginFinished(Result<(), String>),
    RelaunchFinished(Result<(), String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserRequest {
    Relaunch,
    SaveForLogin,
    Login,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum TabDirective {
    #[default]
    Keep,
    DetachAll,
    ReopenFocused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BrowserStatus {
    Notice(String),
    Error(String),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct BrowserOutcome {
    pub(crate) request: Option<BrowserRequest>,
    pub(crate) tabs: TabDirective,
    pub(crate) status: Option<BrowserStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserState {
    Running,
    SavingLogin,
    Login,
    Missing,
    Relaunching,
}

pub(crate) struct BrowserLifecycle {
    state: BrowserState,
    login_available: bool,
}

impl BrowserLifecycle {
    pub(crate) fn new(login_available: bool) -> Self {
        Self {
            state: BrowserState::Running,
            login_available,
        }
    }

    pub(crate) fn set_login_available(&mut self, available: bool) {
        self.login_available = available;
    }

    pub(crate) fn gate(&mut self, work: PageWork) -> WorkDecision {
        match (self.state, work) {
            (BrowserState::Running, _) | (BrowserState::SavingLogin, PageWork::Result) => {
                WorkDecision::Proceed
            }
            (BrowserState::Missing, PageWork::Request) => {
                self.state = BrowserState::Relaunching;
                WorkDecision::Relaunch
            }
            _ => WorkDecision::Blocked,
        }
    }

    pub(crate) fn transition(&mut self, signal: BrowserSignal) -> BrowserOutcome {
        match signal {
            BrowserSignal::Lost
                if matches!(
                    self.state,
                    BrowserState::Running | BrowserState::SavingLogin
                ) =>
            {
                self.state = BrowserState::Relaunching;
                BrowserOutcome {
                    request: Some(BrowserRequest::Relaunch),
                    tabs: TabDirective::DetachAll,
                    status: Some(BrowserStatus::Notice(
                        "browser gone, restarting".to_string(),
                    )),
                }
            }
            BrowserSignal::Back => {
                self.state = BrowserState::Running;
                BrowserOutcome {
                    tabs: TabDirective::ReopenFocused,
                    ..BrowserOutcome::default()
                }
            }
            BrowserSignal::LoginRequested if !self.login_available => BrowserOutcome {
                status: Some(BrowserStatus::Error(
                    "login needs WWT's persistent profile".to_string(),
                )),
                ..BrowserOutcome::default()
            },
            BrowserSignal::LoginRequested if self.state == BrowserState::Running => {
                self.state = BrowserState::SavingLogin;
                BrowserOutcome {
                    request: Some(BrowserRequest::SaveForLogin),
                    status: Some(BrowserStatus::Notice(
                        "saving session for login".to_string(),
                    )),
                    ..BrowserOutcome::default()
                }
            }
            BrowserSignal::LoginRequested => BrowserOutcome {
                status: Some(BrowserStatus::Notice(
                    "a browser handoff is already in progress".to_string(),
                )),
                ..BrowserOutcome::default()
            },
            BrowserSignal::LoginSaved(result) if self.state == BrowserState::SavingLogin => {
                match result {
                    Ok(()) => {
                        self.state = BrowserState::Login;
                        BrowserOutcome {
                            request: Some(BrowserRequest::Login),
                            tabs: TabDirective::DetachAll,
                            status: Some(BrowserStatus::Notice(
                                "finish login in Chromium, then close it".to_string(),
                            )),
                        }
                    }
                    Err(message) => {
                        self.state = BrowserState::Running;
                        BrowserOutcome {
                            status: Some(BrowserStatus::Error(format!(
                                "login failed: save session: {message}"
                            ))),
                            ..BrowserOutcome::default()
                        }
                    }
                }
            }
            BrowserSignal::LoginFinished(result) if self.state == BrowserState::Login => {
                self.state = BrowserState::Relaunching;
                let status = match result {
                    Ok(()) => BrowserStatus::Notice("login window closed, restarting".to_string()),
                    Err(message) => {
                        BrowserStatus::Error(format!("login failed: {message}; restarting"))
                    }
                };
                BrowserOutcome {
                    request: Some(BrowserRequest::Relaunch),
                    status: Some(status),
                    ..BrowserOutcome::default()
                }
            }
            BrowserSignal::RelaunchFinished(result) if self.state == BrowserState::Relaunching => {
                self.state = BrowserState::Missing;
                BrowserOutcome {
                    status: result.err().map(|message| {
                        BrowserStatus::Error(format!("no browser: {message}. any key retries"))
                    }),
                    ..BrowserOutcome::default()
                }
            }
            _ => BrowserOutcome::default(),
        }
    }

    pub(crate) fn running(&self) -> bool {
        self.state == BrowserState::Running
    }

    pub(crate) fn allows_tab_change(&self) -> bool {
        self.state != BrowserState::SavingLogin
    }

    pub(crate) fn allows_command(&self, login: bool) -> bool {
        login || self.state != BrowserState::SavingLogin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_browser_accepts_requests_and_results() {
        let mut browser = BrowserLifecycle::new(true);
        assert_eq!(browser.gate(PageWork::Request), WorkDecision::Proceed);
        assert_eq!(browser.gate(PageWork::Result), WorkDecision::Proceed);
    }

    #[test]
    fn missing_page_work_requests_exactly_one_relaunch() {
        let mut browser = BrowserLifecycle::new(true);
        browser.transition(BrowserSignal::Lost);
        browser.transition(BrowserSignal::RelaunchFinished(Err("gone".to_string())));

        assert_eq!(browser.gate(PageWork::Request), WorkDecision::Relaunch);
        assert_eq!(browser.gate(PageWork::Request), WorkDecision::Blocked);
    }

    #[test]
    fn deliberate_login_blocks_page_work_without_requesting_relaunch() {
        let mut browser = BrowserLifecycle::new(true);
        browser.transition(BrowserSignal::LoginRequested);
        browser.transition(BrowserSignal::LoginSaved(Ok(())));

        assert_eq!(browser.gate(PageWork::Request), WorkDecision::Blocked);
        assert_eq!(browser.gate(PageWork::Result), WorkDecision::Blocked);
    }

    #[test]
    fn browser_loss_detaches_tabs_and_requests_relaunch() {
        let mut browser = BrowserLifecycle::new(true);
        let outcome = browser.transition(BrowserSignal::Lost);

        assert_eq!(outcome.request, Some(BrowserRequest::Relaunch));
        assert_eq!(outcome.tabs, TabDirective::DetachAll);
        assert_eq!(
            outcome.status,
            Some(BrowserStatus::Notice(
                "browser gone, restarting".to_string()
            ))
        );
        assert!(!browser.running());
    }

    #[test]
    fn browser_back_reopens_only_the_focused_tab() {
        let mut browser = BrowserLifecycle::new(true);
        browser.transition(BrowserSignal::Lost);
        let outcome = browser.transition(BrowserSignal::Back);

        assert_eq!(outcome.tabs, TabDirective::ReopenFocused);
        assert!(browser.running());
    }

    #[test]
    fn private_session_refuses_login_without_leaving_running() {
        let mut browser = BrowserLifecycle::new(false);
        let outcome = browser.transition(BrowserSignal::LoginRequested);

        assert_eq!(
            outcome.status,
            Some(BrowserStatus::Error(
                "login needs WWT's persistent profile".to_string()
            ))
        );
        assert_eq!(outcome.request, None);
        assert!(browser.running());
    }

    #[test]
    fn login_save_success_detaches_tabs_and_hands_off_the_browser() {
        let mut browser = BrowserLifecycle::new(true);
        let requested = browser.transition(BrowserSignal::LoginRequested);
        assert_eq!(requested.request, Some(BrowserRequest::SaveForLogin));
        assert!(!browser.allows_tab_change());
        assert!(!browser.allows_command(false));
        assert!(browser.allows_command(true));

        let saved = browser.transition(BrowserSignal::LoginSaved(Ok(())));
        assert_eq!(saved.request, Some(BrowserRequest::Login));
        assert_eq!(saved.tabs, TabDirective::DetachAll);
        assert_eq!(
            saved.status,
            Some(BrowserStatus::Notice(
                "finish login in Chromium, then close it".to_string()
            ))
        );
    }

    #[test]
    fn failed_login_save_returns_to_the_running_browser() {
        let mut browser = BrowserLifecycle::new(true);
        browser.transition(BrowserSignal::LoginRequested);
        let outcome = browser.transition(BrowserSignal::LoginSaved(Err("disk full".to_string())));

        assert!(browser.running());
        assert_eq!(
            outcome.status,
            Some(BrowserStatus::Error(
                "login failed: save session: disk full".to_string()
            ))
        );
    }

    #[test]
    fn closing_or_failing_visible_login_requests_one_relaunch() {
        for result in [Ok(()), Err("could not launch".to_string())] {
            let mut browser = BrowserLifecycle::new(true);
            browser.transition(BrowserSignal::LoginRequested);
            browser.transition(BrowserSignal::LoginSaved(Ok(())));

            let outcome = browser.transition(BrowserSignal::LoginFinished(result));
            assert_eq!(outcome.request, Some(BrowserRequest::Relaunch));
            assert_eq!(
                browser.transition(BrowserSignal::LoginFinished(Ok(()))),
                BrowserOutcome::default()
            );
        }
    }
}
