//! Live reproduction for an application shell that stops while video keeps playing.
//!
//! This is ignored because it depends on public web sites. Run one case with:
//!
//! ```text
//! WWT_LIVE_CASE=baseline cargo test -p wwt --test youtube_frame_pacing -- --ignored --nocapture
//! ```
//!
//! Cases are `baseline`, `uncapped`, `gpu-enabled`, `text`, and `twitch`.
//! `uncapped` restores the launch flag that caused the regression, so the
//! red/green comparison remains runnable after the production fix. Override
//! the default URL with `WWT_LIVE_URL` and the observation window in seconds
//! with `WWT_LIVE_SECONDS`. Set `WWT_LIVE_TRIAL` to retain separately named
//! artifacts from repeated runs. Set `WWT_LIVE_PATH=spa` to reach the YouTube
//! watch page through a real click on its search results page.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, interval, sleep_until};
use wwt::effect::{Effect, Source};
use wwt::event::{Event, Failure, Job};
use wwt::session::{Session, page_viewport};
use wwt::tab::TabId;
use wwt_cdp::{Chromium, Client, Event as CdpEvent, find_chromium};
use wwt_frame::{CellSize, CssPoint, GridSize};
use wwt_page::{Extraction, KeyInput, MouseInput, Page, Status};

const YOUTUBE_URL: &str = "https://www.youtube.com/watch?v=pi3hXvj2A4g";
const YOUTUBE_SEARCH_URL: &str = "https://www.youtube.com/results?search_query=pi3hXvj2A4g";
const TWITCH_URL: &str = "https://www.twitch.tv/chilledchaos";
const GRID: GridSize = GridSize {
    cols: 190,
    rows: 50,
};
const CELL: CellSize = CellSize { w: 10, h: 20 };
const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);
const DEFAULT_WINDOW: Duration = Duration::from_secs(45);
const DIAGNOSTIC_PREFIX: &str = "[youtube-frame-pacing]";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Site {
    YouTube,
    Twitch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaseConfig {
    name: &'static str,
    site: Site,
    pixel: bool,
    dropped_flag: Option<&'static str>,
    added_flag: Option<&'static str>,
}

const CASES: &[CaseConfig] = &[
    CaseConfig {
        name: "baseline",
        site: Site::YouTube,
        pixel: true,
        dropped_flag: None,
        added_flag: None,
    },
    CaseConfig {
        name: "uncapped",
        site: Site::YouTube,
        pixel: true,
        dropped_flag: None,
        added_flag: Some("--disable-frame-rate-limit"),
    },
    CaseConfig {
        name: "gpu-enabled",
        site: Site::YouTube,
        pixel: true,
        dropped_flag: Some("--disable-gpu"),
        added_flag: None,
    },
    CaseConfig {
        name: "text",
        site: Site::YouTube,
        pixel: false,
        dropped_flag: None,
        added_flag: None,
    },
    CaseConfig {
        name: "twitch",
        site: Site::Twitch,
        pixel: true,
        dropped_flag: None,
        added_flag: None,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavigationPath {
    Direct,
    Spa,
}

impl NavigationPath {
    fn from_env(case: CaseConfig) -> Self {
        match std::env::var("WWT_LIVE_PATH")
            .as_deref()
            .unwrap_or("direct")
        {
            "direct" => Self::Direct,
            "spa" if case.site == Site::YouTube => Self::Spa,
            "spa" => panic!("WWT_LIVE_PATH=spa is only supported for YouTube"),
            other => panic!("unknown WWT_LIVE_PATH {other:?}"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Spa => "spa",
        }
    }
}

impl CaseConfig {
    fn named(name: &str) -> Option<Self> {
        CASES.iter().copied().find(|case| case.name == name)
    }

    fn from_env() -> Self {
        let name = std::env::var("WWT_LIVE_CASE").unwrap_or_else(|_| "baseline".to_string());
        Self::named(&name).unwrap_or_else(|| panic!("unknown WWT_LIVE_CASE {name:?}"))
    }
}

#[test]
fn case_configuration_keeps_each_policy_together() {
    let uncapped = CaseConfig::named("uncapped").expect("known diagnostic case");

    assert_eq!(uncapped.name, "uncapped");
    assert_eq!(uncapped.site, Site::YouTube);
    assert!(uncapped.pixel);
    assert_eq!(uncapped.dropped_flag, None);
    assert_eq!(uncapped.added_flag, Some("--disable-frame-rate-limit"));
    assert_eq!(CaseConfig::named("unknown"), None);
}

#[test]
fn gpu_enabled_case_removes_only_the_gpu_disabling_flag() {
    let gpu_enabled = CaseConfig::named("gpu-enabled").expect("known diagnostic case");

    assert_eq!(gpu_enabled.site, Site::YouTube);
    assert!(gpu_enabled.pixel);
    assert_eq!(gpu_enabled.dropped_flag, Some("--disable-gpu"));
    assert_eq!(gpu_enabled.added_flag, None);
    assert_eq!(CaseConfig::named("gpu"), None);
}

#[derive(Debug)]
enum Background {
    Ack {
        latency_ms: f64,
        result: Result<(), String>,
    },
    Status(Result<Status, Failure>),
}

struct PreparedPage {
    page: Arc<Page>,
    navigation_started: Instant,
    navigation_finished: Instant,
}

struct WatchNavigation {
    metrics: Value,
    started: Instant,
    reached: Instant,
}

#[derive(Debug, Default)]
struct NetworkLog {
    console_errors: Vec<String>,
    failed_requests: Vec<String>,
    requests: BTreeMap<String, String>,
}

struct ObservedEvents<'a> {
    receiver: &'a mut mpsc::UnboundedReceiver<CdpEvent>,
    log: &'a mut NetworkLog,
    session_id: &'a str,
}

impl<'a> ObservedEvents<'a> {
    fn new(
        receiver: &'a mut mpsc::UnboundedReceiver<CdpEvent>,
        log: &'a mut NetworkLog,
        session_id: &'a str,
    ) -> Self {
        Self {
            receiver,
            log,
            session_id,
        }
    }

    async fn recv(&mut self) -> Option<CdpEvent> {
        let event = self.receiver.recv().await?;
        self.log.observe(&event, self.session_id);
        Some(event)
    }

    fn into_log(self) -> &'a mut NetworkLog {
        self.log
    }
}

impl NetworkLog {
    fn observe(&mut self, event: &CdpEvent, session_id: &str) {
        if event.session_id.as_deref() != Some(session_id) {
            return;
        }
        match event.method.as_str() {
            "Network.requestWillBeSent" => {
                if let (Some(id), Some(url)) = (
                    event.params["requestId"].as_str(),
                    event.params["request"]["url"].as_str(),
                ) {
                    self.requests
                        .insert(id.to_string(), redact_url(url).to_string());
                }
            }
            "Runtime.consoleAPICalled"
                if matches!(event.params["type"].as_str(), Some("error" | "assert")) =>
            {
                let message = event.params["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|argument| {
                        argument["value"]
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| argument["value"].as_f64().map(|value| value.to_string()))
                            .or_else(|| argument["description"].as_str().map(str::to_string))
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                self.push_console(&message);
            }
            "Runtime.exceptionThrown" => self.push_console(
                event.params["exceptionDetails"]["text"]
                    .as_str()
                    .unwrap_or("JavaScript exception"),
            ),
            "Log.entryAdded" => {
                let entry = &event.params["entry"];
                if matches!(entry["level"].as_str(), Some("error" | "warning")) {
                    self.push_console(entry["text"].as_str().unwrap_or("log entry"));
                }
            }
            "Network.loadingFailed" => {
                let id = event.params["requestId"]
                    .as_str()
                    .unwrap_or("unknown request");
                let url = self.requests.get(id).map(String::as_str).unwrap_or(id);
                let error = event.params["errorText"].as_str().unwrap_or("failed");
                self.push_failed(&format!("{url}: {error}"));
            }
            "Network.responseReceived" => {
                let response = &event.params["response"];
                let status = response["status"].as_f64().unwrap_or_default();
                if status >= 400.0 {
                    let url = response["url"].as_str().unwrap_or("unknown URL");
                    self.push_failed(&format!("{status:.0} {}", redact_url(url)));
                }
            }
            _ => {}
        }
    }

    fn push_console(&mut self, message: &str) {
        if self.console_errors.len() < 40 {
            self.console_errors
                .push(message.chars().take(300).collect());
        }
    }

    fn push_failed(&mut self, message: &str) {
        if self.failed_requests.len() < 80 {
            self.failed_requests
                .push(message.chars().take(300).collect());
        }
    }
}

#[test]
fn console_error_events_are_logged() {
    let mut log = NetworkLog::default();
    log.observe(
        &CdpEvent {
            session_id: Some("page".to_string()),
            method: "Runtime.consoleAPICalled".to_string(),
            params: json!({
                "type": "error",
                "args": [
                    { "type": "string", "value": "application failed" },
                    { "type": "number", "value": 503 }
                ]
            }),
        },
        "page",
    );

    assert_eq!(log.console_errors, ["application failed 503"]);
}

#[tokio::test]
async fn observed_events_are_logged_before_navigation_receives_them() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    tx.send(CdpEvent {
        session_id: Some("page".to_string()),
        method: "Runtime.consoleAPICalled".to_string(),
        params: json!({
            "type": "error",
            "args": [{ "type": "string", "value": "transition failed" }]
        }),
    })
    .expect("queue a CDP event");
    let mut log = NetworkLog::default();
    let mut events = ObservedEvents::new(&mut rx, &mut log, "page");

    let event = events.recv().await.expect("receive the queued event");
    assert_eq!(event.method, "Runtime.consoleAPICalled");
    assert_eq!(events.into_log().console_errors, ["transition failed"]);
}

fn redact_url(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

async fn click(page: &Page, at: CssPoint) -> Result<(), String> {
    page.dispatch_mouse(&MouseInput::press(at))
        .await
        .map_err(|error| error.to_string())?;
    page.dispatch_mouse(&MouseInput::release(at))
        .await
        .map_err(|error| error.to_string())
}

fn observation_window() -> Duration {
    std::env::var("WWT_LIVE_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_WINDOW)
}

fn site_url(case: CaseConfig) -> String {
    std::env::var("WWT_LIVE_URL").unwrap_or_else(|_| match case.site {
        Site::YouTube => YOUTUBE_URL.to_string(),
        Site::Twitch => TWITCH_URL.to_string(),
    })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn chromium_wrapper(dir: &Path, dropped: Option<&str>, added: Option<&str>) -> PathBuf {
    let real = find_chromium(None).expect("find Chromium");
    let path = dir.join("chromium-case");
    let filter = dropped.map_or(String::new(), |flag| {
        format!(
            "if [[ \"$argument\" == {} ]]; then continue; fi\n",
            shell_quote(flag)
        )
    });
    let script = format!(
        "#!/usr/bin/bash\narguments=()\nfor argument in \"$@\"; do\n  {filter}  arguments+=(\"$argument\")\ndone\nexec {} \"${{arguments[@]}}\" {}\n",
        shell_quote(&real.display().to_string()),
        added.map(shell_quote).unwrap_or_default(),
    );
    fs::write(&path, script).expect("write Chromium wrapper");
    let mut permissions = fs::metadata(&path).expect("wrapper metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("make Chromium wrapper executable");
    path
}

fn process_cpu(profile: &Path) -> f64 {
    let output = Command::new("ps")
        .args(["-eo", "pcpu=,args="])
        .output()
        .expect("read Chromium CPU from ps");
    let profile = profile.display().to_string();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains("chrom") && line.contains(&profile))
        .filter_map(|line| line.split_whitespace().next()?.parse::<f64>().ok())
        .sum()
}

const PAGE_PROBE: &str = r#"
(() => {
  const state = { raf: 0, longTasks: [], installedAt: performance.now() };
  try {
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        state.longTasks.push({ start: entry.startTime, duration: entry.duration });
      }
    }).observe({ type: "longtask", buffered: true });
  } catch (_) {}
  function frame() {
    state.raf += 1;
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
  window.__wwtYoutubeFramePacingProbe = state;
})();
"#;

const SAMPLE: &str = include_str!("youtube_frame_pacing_sample.js");

async fn prepare_live_page(client: Arc<Client>, url: &str) -> Result<PreparedPage, String> {
    let vp = page_viewport(GRID, CELL);
    let page = Arc::new(
        Page::open(Arc::clone(&client), "about:blank", vp)
            .await
            .map_err(|error| error.to_string())?,
    );
    for (method, params) in [
        ("Network.enable", json!({})),
        ("Log.enable", json!({})),
        (
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": PAGE_PROBE }),
        ),
    ] {
        client
            .call_on(page.session_id(), method, params)
            .await
            .map_err(|error| error.to_string())?;
    }
    let navigation_started = Instant::now();
    page.navigate(url)
        .await
        .map_err(|error| error.to_string())?;
    Ok(PreparedPage {
        page,
        navigation_started,
        navigation_finished: Instant::now(),
    })
}

async fn acknowledge_screencast_event(
    page: &Page,
    session: &mut Session,
    event: &CdpEvent,
) -> Result<(), String> {
    let Some(frame) = page.screencast_frame(event) else {
        return Ok(());
    };
    for effect in session.on(Event::Frame(TabId(0), Box::new(frame))) {
        if let Effect::AckFrame(_, ack) = effect {
            tokio::time::sleep(FRAME_INTERVAL).await;
            page.ack_frame(ack)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

async fn navigate_youtube_spa(
    page: &Page,
    events: &mut ObservedEvents<'_>,
    session: &mut Session,
) -> Result<WatchNavigation, String> {
    let raf_before = page
        .eval("window.__wwtYoutubeFramePacingProbe?.raf || 0")
        .await
        .map_err(|error| error.to_string())?;
    let warmup_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        tokio::select! {
            Some(event) = events.recv() => {
                acknowledge_screencast_event(page, session, &event).await?;
            }
            _ = sleep_until(warmup_deadline) => break,
        }
    }
    let raf_after = page
        .eval("window.__wwtYoutubeFramePacingProbe?.raf || 0")
        .await
        .map_err(|error| error.to_string())?;
    let target = page
        .eval(
            r#"(() => {
          const link = document.querySelector("a[href*='watch?v=pi3hXvj2A4g']");
          if (!link) return null;
          const rect = link.getBoundingClientRect();
          return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
        })()"#,
        )
        .await
        .map_err(|error| error.to_string())?;
    if target.is_null() {
        return Err("the exact video was absent from YouTube search results".to_string());
    }
    let at = CssPoint {
        x: target["x"]
            .as_f64()
            .ok_or("search result has no center x")?,
        y: target["y"]
            .as_f64()
            .ok_or("search result has no center y")?,
    };
    let started = Instant::now();
    click(page, at).await?;

    let navigation_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let href = page
            .eval("location.href")
            .await
            .map_err(|error| error.to_string())?;
        if href
            .as_str()
            .is_some_and(|href| href.contains("watch?v=pi3hXvj2A4g"))
        {
            return Ok(WatchNavigation {
                metrics: json!({
                    "rafBeforeWarmup": raf_before,
                    "rafAfterWarmup": raf_after,
                }),
                started,
                reached: Instant::now(),
            });
        }
        tokio::select! {
            Some(event) = events.recv() => {
                acknowledge_screencast_event(page, session, &event).await?;
            }
            _ = sleep_until(navigation_deadline) => {
                return Err("real click did not trigger the YouTube SPA navigation".to_string());
            }
        }
    }
}

async fn ensure_video_playing(page: &Page) -> Result<Instant, String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let value = page
            .eval(
                r#"(() => {
              const video = document.querySelector("video");
              if (!video) return null;
              const rect = video.getBoundingClientRect();
              return {
                paused: video.paused,
                readyState: video.readyState,
                x: rect.left + rect.width / 2,
                y: rect.top + rect.height / 2
              };
            })()"#,
            )
            .await
            .map_err(|error| error.to_string())?;
        if value.is_null() || value["readyState"].as_u64().unwrap_or_default() == 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        if value["paused"] == false {
            return Ok(Instant::now());
        }
        let at = CssPoint {
            x: value["x"].as_f64().ok_or("video has no center x")?,
            y: value["y"].as_f64().ok_or("video has no center y")?,
        };
        click(page, at).await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err("a real mouse click did not start the video within 15s".to_string())
}

async fn verify_youtube_interactions(page: &Page) -> Result<Value, String> {
    let search = page
        .eval(
            r#"(() => {
          const inputs = Array.from(document.querySelectorAll(
            "ytd-searchbox input, yt-searchbox input, input#search, input[name='search_query']"
          ));
          const input = inputs.find((candidate) => {
            const rect = candidate.getBoundingClientRect();
            return rect.width > 0 && rect.height > 0;
          });
          if (!input) return null;
          const rect = input.getBoundingClientRect();
          return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
        })()"#,
        )
        .await
        .map_err(|error| error.to_string())?;
    if search.is_null() {
        return Err("YouTube has no visible search control".to_string());
    }
    let search_at = CssPoint {
        x: search["x"].as_f64().ok_or("search has no center x")?,
        y: search["y"].as_f64().ok_or("search has no center y")?,
    };
    click(page, search_at).await?;
    page.dispatch_key(&KeyInput {
        key: "z".to_string(),
        code: "KeyZ".to_string(),
        windows_virtual_key_code: 90,
        text: "z".to_string(),
        modifiers: 0,
    })
    .await
    .map_err(|error| error.to_string())?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let search_typed = page
        .eval("document.activeElement?.value?.endsWith('z') || false")
        .await
        .map_err(|error| error.to_string())?;
    page.blur().await.map_err(|error| error.to_string())?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let play = page
        .eval(
            r#"(() => {
          const video = document.querySelector("video");
          if (!video) return null;
          const rect = video.getBoundingClientRect();
          return {
            paused: video.paused,
            x: rect.left + rect.width / 2,
            y: rect.top + rect.height / 2
          };
        })()"#,
        )
        .await
        .map_err(|error| error.to_string())?;
    let play_at = CssPoint {
        x: play["x"].as_f64().ok_or("play control has no center x")?,
        y: play["y"].as_f64().ok_or("play control has no center y")?,
    };
    click(page, play_at).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let paused_after_click = page
        .eval("document.querySelector('video')?.paused")
        .await
        .map_err(|error| error.to_string())?
        .as_bool();
    let play_toggled = paused_after_click != play["paused"].as_bool();
    click(page, play_at).await?;

    let scroll_before = page
        .eval("window.scrollY")
        .await
        .map_err(|error| error.to_string())?;
    page.scroll_by(400.0, page_viewport(GRID, CELL))
        .await
        .map_err(|error| error.to_string())?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let scroll_after = page
        .eval("window.scrollY")
        .await
        .map_err(|error| error.to_string())?;
    let scrolled =
        scroll_after.as_f64().unwrap_or_default() > scroll_before.as_f64().unwrap_or_default();

    Ok(json!({
        "searchTyped": search_typed,
        "playToggled": play_toggled,
        "scrollBefore": scroll_before,
        "scrollAfter": scroll_after,
        "scrolled": scrolled,
    }))
}

fn status_line(session: &Session) -> String {
    session.compose().row_text(GRID.rows - 1)
}

fn initial_session(extraction: Extraction, pixel: bool) -> (Session, Vec<Effect>) {
    let mut session = Session::new(GRID, CELL);
    let id = session.focused_id();
    session.on(Event::Done(Job::Extracted(
        id,
        Source::Script,
        Ok(Box::new(extraction)),
    )));
    session.set_graphics(true);
    let effects = if pixel {
        session.on(Event::Key(key('p')))
    } else {
        Vec::new()
    };
    (session, effects)
}

fn first_number(samples: &[Value], key: &str) -> Option<f64> {
    samples.iter().find_map(|sample| sample[key].as_f64())
}

fn last_number(samples: &[Value], key: &str) -> Option<f64> {
    samples.iter().rev().find_map(|sample| sample[key].as_f64())
}

fn cumulative_video_advance(samples: &[Value]) -> f64 {
    samples
        .windows(2)
        .filter_map(|pair| {
            let before = pair[0]["videoTime"].as_f64()?;
            let after = pair[1]["videoTime"].as_f64()?;
            (after >= before).then_some(after - before)
        })
        .sum()
}

fn first_time(samples: &[Value], predicate: impl Fn(&Value) -> bool) -> Option<f64> {
    samples
        .iter()
        .find(|sample| predicate(sample))
        .and_then(|sample| sample["elapsedMs"].as_f64())
}

fn elapsed_ms(origin: Instant, event: Instant) -> f64 {
    event.duration_since(origin).as_secs_f64() * 1_000.0
}

#[test]
fn milestone_offsets_are_relative_to_watch_navigation() {
    let navigation_started = Instant::now();
    let video_started = navigation_started + Duration::from_millis(4_250);

    assert_eq!(elapsed_ms(navigation_started, video_started), 4_250.0);
}

fn write_artifact(case: CaseConfig, report: &Value) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/youtube-frame-pacing");
    fs::create_dir_all(&dir).expect("create diagnostic artifact directory");
    let trial = std::env::var("WWT_LIVE_TRIAL")
        .ok()
        .map(|value| {
            format!(
                "-{}",
                value.replace(|character: char| !character.is_ascii_alphanumeric(), "_")
            )
        })
        .unwrap_or_default();
    let path = dir.join(format!("{}{trial}.json", case.name));
    fs::write(
        &path,
        serde_json::to_vec_pretty(report).expect("encode diagnostic report"),
    )
    .expect("write diagnostic report");
    path
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live YouTube/Twitch diagnostic; run explicitly with WWT_LIVE_CASE"]
async fn video_progress_does_not_outlive_the_application_shell() {
    let case = CaseConfig::from_env();
    let navigation_path = NavigationPath::from_env(case);
    let url = site_url(case);
    let initial_url = match navigation_path {
        NavigationPath::Direct => &url,
        NavigationPath::Spa => YOUTUBE_SEARCH_URL,
    };
    let window = observation_window();
    let fixture = tempfile::tempdir().expect("diagnostic directory");
    let profile = fixture.path().join("profile");
    fs::create_dir(&profile).expect("create diagnostic profile");
    let wrapper = chromium_wrapper(fixture.path(), case.dropped_flag, case.added_flag);

    eprintln!(
        "{DIAGNOSTIC_PREFIX} case={} path={} pixel={} dropped_flag={:?} added_flag={:?} url={} window={:?}",
        case.name,
        navigation_path.name(),
        case.pixel,
        case.dropped_flag,
        case.added_flag,
        redact_url(&url),
        window
    );

    let browser = Chromium::launch(Some(&profile), Some(&wrapper))
        .await
        .expect("launch diagnostic Chromium");
    let client = Arc::new(Client::connect(browser.ws_url()).await.expect("connect"));
    client.auto_attach().await.expect("turn on auto-attach");
    let mut cdp = client.subscribe();
    let mut log = NetworkLog::default();
    let prepared = prepare_live_page(Arc::clone(&client), initial_url)
        .await
        .expect("open live page");
    let page = prepared.page;
    let mut events = ObservedEvents::new(&mut cdp, &mut log, page.session_id());
    page.activate().await.expect("activate live page");

    let extraction_started = Instant::now();
    let extraction = page.extract().await.expect("initial WWT extraction");
    let extraction_ms = extraction_started.elapsed();
    let (mut session, effects) = initial_session(extraction, case.pixel);
    let id = TabId(0);
    for effect in effects {
        if let Effect::StartScreencast(_, size) = effect {
            page.start_screencast(size.width, size.height)
                .await
                .expect("start real WWT screencast");
        }
    }
    let watch_navigation = match navigation_path {
        NavigationPath::Direct => WatchNavigation {
            metrics: Value::Null,
            started: prepared.navigation_started,
            reached: prepared.navigation_finished,
        },
        NavigationPath::Spa => navigate_youtube_spa(&page, &mut events, &mut session)
            .await
            .expect("navigate to the exact video through YouTube's search results"),
    };
    let video_started_at = ensure_video_playing(&page)
        .await
        .expect("start video with a real page click");
    let first_status = status_line(&session);
    eprintln!("{DIAGNOSTIC_PREFIX} status={first_status:?}");
    if case.pixel {
        assert!(
            first_status.contains("[pixel]"),
            "WWT did not enter pixel mode: {first_status:?}"
        );
        assert!(
            !first_status.contains("[loading]"),
            "WWT remained loading: {first_status:?}"
        );
        assert!(
            !first_status.contains("[stalled]"),
            "WWT was already stalled: {first_status:?}"
        );
    }

    let (background_tx, mut background_rx) = mpsc::unbounded_channel();
    let mut next_frame_at = tokio::time::Instant::now();
    let mut samples = Vec::new();
    let mut sample_errors = Vec::new();
    let mut frame_count = 0_u64;
    let mut ack_count = 0_u64;
    let mut ack_latencies = Vec::new();
    let mut cdp_latencies = Vec::new();
    let mut status_reads = 0_u64;
    let mut reading_status = false;
    let cpu_start = process_cpu(&profile);
    let observed_at = Instant::now();
    let deadline = observed_at + window;
    let mut sampling = interval(SAMPLE_INTERVAL);
    sampling.set_missed_tick_behavior(MissedTickBehavior::Skip);

    while Instant::now() < deadline {
        tokio::select! {
            _ = sampling.tick() => {
                let started = Instant::now();
                match page.eval(SAMPLE).await {
                    Ok(mut sample) => {
                        cdp_latencies.push(started.elapsed().as_secs_f64() * 1_000.0);
                        sample["elapsedMs"] = json!(elapsed_ms(watch_navigation.started, Instant::now()));
                        sample["chromiumCpu"] = json!(process_cpu(&profile));
                        samples.push(sample);
                    }
                    Err(error) => {
                        sample_errors.push(error.to_string());
                    }
                }
            }
            Some(event) = events.recv() => {
                if let Some(frame) = page.screencast_frame(&event) {
                    frame_count += 1;
                    let effects = session.on(Event::Frame(id, Box::new(frame)));
                    for effect in effects {
                        if let Effect::AckFrame(_, ack) = effect {
                            let page = Arc::clone(&page);
                            let tx = background_tx.clone();
                            next_frame_at = next_frame_at.max(tokio::time::Instant::now()) + FRAME_INTERVAL;
                            let at = next_frame_at;
                            tokio::spawn(async move {
                                let started = Instant::now();
                                sleep_until(at).await;
                                let result = page.ack_frame(ack).await.map_err(|error| error.to_string());
                                let _ = tx.send(Background::Ack {
                                    latency_ms: started.elapsed().as_secs_f64() * 1_000.0,
                                    result,
                                });
                            });
                        }
                    }
                } else if page.is_dirty(&event) && !reading_status {
                    let effects = session.on(Event::Dirty(id));
                    if effects.iter().any(|effect| matches!(effect, Effect::ReadStatus(_))) {
                        reading_status = true;
                        status_reads += 1;
                        let page = Arc::clone(&page);
                        let tx = background_tx.clone();
                        tokio::spawn(async move {
                            let status = page.status().await.map_err(|error| Failure::from_error(&error));
                            let _ = tx.send(Background::Status(status));
                        });
                    }
                }
            }
            Some(background) = background_rx.recv() => match background {
                Background::Ack { latency_ms, result } => {
                    result.expect("ack real screencast frame");
                    ack_count += 1;
                    ack_latencies.push(latency_ms);
                }
                Background::Status(result) => {
                    reading_status = false;
                    session.on(Event::Done(Job::Status(id, result)));
                }
            }
        }
    }

    let interactions = match case.site {
        Site::YouTube => match verify_youtube_interactions(&page).await {
            Ok(interactions) => interactions,
            Err(error) => json!({
                "error": error,
                "searchTyped": false,
                "playToggled": false,
                "scrolled": false,
            }),
        },
        Site::Twitch => Value::Null,
    };
    if case.pixel {
        page.stop_screencast()
            .await
            .expect("stop real WWT screencast");
    }
    let final_status = status_line(&session);
    let cpu_end = process_cpu(&profile);
    let video_advance = cumulative_video_advance(&samples);
    let raf_start = first_number(&samples, "raf").unwrap_or_default();
    let raf_end = last_number(&samples, "raf").unwrap_or_default();
    let raf_rate = (raf_end - raf_start) / window.as_secs_f64();
    let metadata_ms = first_time(&samples, |sample| sample["metadataPresent"] == true);
    let recommendations_ms = first_time(&samples, |sample| {
        sample["recommendationCount"].as_u64().unwrap_or_default() >= 10
    });
    let loading_gone_ms = first_time(&samples, |sample| sample["youtubeLoadingBar"] == false);
    let final_sample = samples.last().cloned().unwrap_or_else(|| json!({}));
    let log = events.into_log();
    let unique_console: BTreeSet<_> = std::mem::take(&mut log.console_errors)
        .into_iter()
        .collect();
    let unique_failed: BTreeSet<_> = std::mem::take(&mut log.failed_requests)
        .into_iter()
        .collect();
    let report = json!({
        "case": case.name,
        "navigationPath": navigation_path.name(),
        "spaMetrics": watch_navigation.metrics,
        "interactions": interactions.clone(),
        "site": format!("{:?}", case.site),
        "url": redact_url(&url),
        "pixel": case.pixel,
        "droppedFlag": case.dropped_flag,
        "addedFlag": case.added_flag,
        "observationSeconds": window.as_secs_f64(),
        "initialNavigationLoadMs": elapsed_ms(
            prepared.navigation_started,
            prepared.navigation_finished,
        ),
        "watchNavigationReachedMs": elapsed_ms(
            watch_navigation.started,
            watch_navigation.reached,
        ),
        "observationStartedMs": elapsed_ms(watch_navigation.started, observed_at),
        "initialExtractionMs": extraction_ms.as_secs_f64() * 1_000.0,
        "videoStartedMs": elapsed_ms(watch_navigation.started, video_started_at),
        "youtubeLoadingGoneMs": loading_gone_ms,
        "metadataMs": metadata_ms,
        "recommendationsMs": recommendations_ms,
        "videoAdvanceSeconds": video_advance,
        "rafRate": raf_rate,
        "chromiumCpuStart": cpu_start,
        "chromiumCpuEnd": cpu_end,
        "screencastFrames": frame_count,
        "screencastAcks": ack_count,
        "ackRate": ack_count as f64 / window.as_secs_f64(),
        "ackLatencyMaxMs": ack_latencies.iter().copied().fold(0.0_f64, f64::max),
        "cdpLatencyMaxMs": cdp_latencies.iter().copied().fold(0.0_f64, f64::max),
        "cdpLatencyMeanMs": cdp_latencies.iter().sum::<f64>() / cdp_latencies.len().max(1) as f64,
        "statusReads": status_reads,
        "wwtInitialStatus": first_status,
        "wwtFinalStatus": final_status,
        "finalSample": final_sample,
        "sampleErrors": sample_errors,
        "consoleErrors": unique_console,
        "failedRequests": unique_failed,
        "samples": samples,
    });
    let artifact = write_artifact(case, &report);
    eprintln!("{DIAGNOSTIC_PREFIX} report={}", report);
    eprintln!("{DIAGNOSTIC_PREFIX} artifact={}", artifact.display());

    let advanced = video_advance >= 5.0;
    match case.site {
        Site::YouTube => {
            assert!(
                advanced,
                "the video did not advance five seconds; artifact: {}",
                artifact.display()
            );
            let hydrated = final_sample["metadataPresent"] == true
                && final_sample["recommendationCount"]
                    .as_u64()
                    .unwrap_or_default()
                    >= 10
                && final_sample["youtubeLoadingBar"] == false;
            assert!(
                hydrated,
                "video advanced while YouTube's application shell remained incomplete; artifact: {}",
                artifact.display()
            );
            assert_eq!(
                interactions["searchTyped"],
                true,
                "YouTube search did not accept real keyboard input; artifact: {}",
                artifact.display()
            );
            assert_eq!(
                interactions["playToggled"],
                true,
                "YouTube's player control did not respond; artifact: {}",
                artifact.display()
            );
            assert_eq!(
                interactions["scrolled"],
                true,
                "YouTube did not respond to a real wheel scroll; artifact: {}",
                artifact.display()
            );
        }
        Site::Twitch => {
            assert!(
                advanced,
                "the Twitch video did not advance five seconds; artifact: {}",
                artifact.display()
            );
        }
    }
}
