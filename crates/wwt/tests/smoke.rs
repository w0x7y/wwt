use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
/// command line, typing fills it, and what it holds parses to a navigation.
#[test]
fn the_command_line_opens_fills_and_closes() {
    // The keymap decides that `:` opens an empty command line.
    let vp = wwt_frame::Viewport::new(
        wwt_frame::GridSize { cols: 80, rows: 24 },
        wwt_frame::CellSize { w: 9, h: 20 },
    );
    let action = wwt::keymap::action_for(
        KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE),
        vp,
    );
    let Some(wwt::keymap::Action::EnterCommand(prefill)) = action else {
        panic!("`:` should open the command line, got {action:?}");
    };
    let mut mode = Mode::Command(prefill);

    // Typing accumulates, and the chrome row shows it.
    if let Mode::Command(buffer) = &mut mode {
        for c in "open example.com".chars() {
            buffer.push(c);
        }
    }
    let mut frame = wwt_frame::Frame::new(wwt_frame::GridSize { cols: 40, rows: 3 });
    wwt_ui::chrome::paint(&mut frame, &mode, &wwt_ui::chrome::State::Ready, "", "", 0.0);
    assert!(
        frame.row_text(2).starts_with(":open example.com"),
        "row 2 was {:?}",
        frame.row_text(2)
    );

    // And the command it holds parses to the navigation we expect.
    if let Mode::Command(buffer) = &mode {
        assert_eq!(
            wwt_ui::command::parse(buffer),
            Ok(wwt_ui::command::Command::Open("https://example.com".to_string()))
        );
    }
}

/// The same physical key means two different things depending on the mode,
/// which is the whole point of having modes. Normal mode's `q` quits; insert
/// mode's `q` is a letter.
#[test]
fn a_letter_is_a_command_in_normal_mode_and_a_keystroke_in_insert_mode() {
    let vp = wwt_frame::Viewport::new(
        wwt_frame::GridSize { cols: 80, rows: 24 },
        wwt_frame::CellSize { w: 9, h: 20 },
    );
    let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);

    assert_eq!(wwt::keymap::action_for(q, vp), Some(wwt::keymap::Action::Quit));
    assert_eq!(
        wwt::keys::describe(q).expect("q is a key we can send").text,
        "q"
    );

    let i = KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE);
    assert_eq!(
        wwt::keymap::action_for(i, vp),
        Some(wwt::keymap::Action::Insert),
        "`i` is what puts you in the mode where q is a letter"
    );
}
