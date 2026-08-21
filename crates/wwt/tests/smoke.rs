use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use wwt::effect::{Effect, Navigation};
use wwt::event::{Event, Job};
use wwt::session::{Session, page_viewport};
use wwt_cdp::{Chromium, Client};
use wwt_frame::{CellSize, Frame, GridSize};
use wwt_page::{Input, Page};
use wwt_ui::Mode;

const GRID: GridSize = GridSize { cols: 80, rows: 24 };
const CELL: CellSize = CellSize { w: 9, h: 20 };

fn fixture_url(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    format!("file://{}", path.display())
}

/// A real page, read by a real extraction, painted by the session that owns
/// the chrome rows.
///
/// Every step the product takes to get from a URL to a frame, and no step it
/// does not: the viewport is the one the session hands a page, and the frame
/// is the one `compose` produces, chrome and all. There used to be a second
/// path here, `wwt::render_url`, kept from M1 for these two tests. It painted
/// runs into a full-grid viewport with no chrome, which stopped being what
/// the browser does the moment the tab bar arrived, so the assertion that
/// called itself end to end was describing something nothing else did.
async fn composed(fixture: &str) -> Frame {
    let browser = Chromium::launch(None).await.expect("launch chromium");
    let client = Arc::new(Client::connect(browser.ws_url()).await.expect("connect"));
    client.auto_attach().await.expect("turn on auto-attach");

    let page = Page::open(client, &fixture_url(fixture), page_viewport(GRID, CELL))
        .await
        .expect("open the fixture");
    let extraction = page.extract().await.expect("extract");

    let mut session = Session::new(GRID, CELL);
    let id = session.focused().id;
    session.on(Event::Done(Job::Extracted(id, Box::new(extraction))));
    session.compose()
}

#[tokio::test]
async fn renders_a_page_into_the_cell_grid() {
    let frame = composed("skeleton.html").await;

    assert_eq!(frame.grid(), GRID, "the frame is the whole terminal");

    let rendered: Vec<String> = (0..GRID.rows).map(|r| frame.row_text(r)).collect();
    assert!(
        rendered.iter().any(|line| line.contains("WWT WALKS")),
        "the page text is missing from the frame:\n{}",
        rendered.join("\n")
    );
}

#[tokio::test]
async fn text_lands_in_the_first_row_the_page_owns() {
    let frame = composed("skeleton.html").await;

    // body margin is 0 and the paragraph is the first thing on the page, so
    // it lands on the page's first row starting at column 0. That is the
    // frame's second row: the first belongs to the tab bar, and the page
    // does not know it exists. This is the assertion that proves the
    // coordinate model is wired correctly end to end, origin row included.
    assert!(
        frame.row_text(1).starts_with("WWT"),
        "row 1 was {:?}",
        frame.row_text(1)
    );
    assert!(
        !frame.row_text(0).starts_with("WWT"),
        "the page painted over the tab bar: {:?}",
        frame.row_text(0)
    );
}

/// Drive the modal flow without a browser or a terminal: `:` opens the
/// command line, typing fills it, and Enter turns it into a navigation the
/// loop would perform. The session is the thing under test, not a
/// hand-rolled imitation of it.
#[test]
fn the_command_line_opens_fills_and_closes() {
    let mut session = Session::new(GridSize { cols: 40, rows: 3 }, CellSize { w: 9, h: 20 });
    session.begin();

    type_into(&mut session, ":open example.com");
    assert!(matches!(session.mode(), Mode::Command(buffer) if buffer == "open example.com"));

    // The chrome row shows what has been typed, caret and all.
    let frame = session.compose();
    assert!(
        frame.row_text(2).starts_with(":open example.com"),
        "row 2 was {:?}",
        frame.row_text(2)
    );

    let effects = session.on(Event::Key(code(KeyCode::Enter)));
    assert_eq!(
        effects,
        vec![Effect::Navigate(
            wwt::tab::TabId(0),
            Navigation::Open("https://example.com".to_string())
        )]
    );
    assert_eq!(session.mode(), &Mode::Normal, "Enter closes the line it ran");
}

/// The same physical key means two different things depending on the mode,
/// which is the whole point of having modes. Normal mode's `q` quits; insert
/// mode's `q` is a letter on its way to the page.
#[test]
fn a_letter_is_a_command_in_normal_mode_and_a_keystroke_in_insert_mode() {
    let mut normal = Session::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
    assert_eq!(normal.on(Event::Key(key('q'))), vec![Effect::Quit]);

    let mut insert = Session::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
    insert.on(Event::Key(key('i')));
    assert_eq!(insert.mode(), &Mode::Insert, "`i` is what makes q a letter");

    let effects = insert.on(Event::Key(key('q')));
    let [Effect::Send(_, Input::Key(sent))] = effects.as_slice() else {
        panic!("insert mode should send the key, got {effects:?}");
    };
    assert_eq!(sent.text, "q");
}

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn code(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn type_into(session: &mut Session, text: &str) {
    for c in text.chars() {
        session.on(Event::Key(key(c)));
    }
}
