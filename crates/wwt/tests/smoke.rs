use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use wwt::input::Input;
use wwt::session::{Effect, Event, Navigation, Session};
use wwt_frame::{CellSize, GridSize, Viewport};
use wwt_ui::Mode;

fn fixture_url(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    format!("file://{}", path.display())
}

#[tokio::test]
async fn renders_a_page_into_the_cell_grid() {
    let vp = Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
    let frame = wwt::render_url(&fixture_url("skeleton.html"), vp)
        .await
        .expect("render the fixture");

    assert_eq!(frame.grid(), vp.grid());

    let rendered: Vec<String> = (0..vp.grid().rows).map(|r| frame.row_text(r)).collect();
    assert!(
        rendered.iter().any(|line| line.contains("WWT WALKS")),
        "the page text is missing from the frame:\n{}",
        rendered.join("\n")
    );
}

#[tokio::test]
async fn text_lands_in_the_top_left_of_an_unstyled_page() {
    let vp = Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
    let frame = wwt::render_url(&fixture_url("skeleton.html"), vp)
        .await
        .expect("render the fixture");

    // body margin is 0 and the paragraph is the first thing on the page, so
    // it must land in row 0 starting at column 0. This is the assertion that
    // proves the coordinate model is wired correctly end to end.
    assert!(
        frame.row_text(0).starts_with("WWT"),
        "row 0 was {:?}",
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
        vec![Effect::Navigate(Navigation::Open("https://example.com".to_string()))]
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
    let [Effect::Send(Input::Key(sent))] = effects.as_slice() else {
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
