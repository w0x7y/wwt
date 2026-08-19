use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use webinal::chrome::Mode;
use wb_frame::{CellSize, GridSize, Viewport};

fn fixture_url(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    format!("file://{}", path.display())
}

#[tokio::test]
async fn renders_a_page_into_the_cell_grid() {
    let vp = Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
    let frame = webinal::render_url(&fixture_url("skeleton.html"), vp)
        .await
        .expect("render the fixture");

    assert_eq!(frame.grid(), vp.grid());

    let rendered: Vec<String> = (0..vp.grid().rows).map(|r| frame.row_text(r)).collect();
    assert!(
        rendered.iter().any(|line| line.contains("WEBINAL WALKS")),
        "the page text is missing from the frame:\n{}",
        rendered.join("\n")
    );
}

#[tokio::test]
async fn text_lands_in_the_top_left_of_an_unstyled_page() {
    let vp = Viewport::new(GridSize { cols: 80, rows: 24 }, CellSize { w: 9, h: 20 });
    let frame = webinal::render_url(&fixture_url("skeleton.html"), vp)
        .await
        .expect("render the fixture");

    // body margin is 0 and the paragraph is the first thing on the page, so
    // it must land in row 0 starting at column 0. This is the assertion that
    // proves the coordinate model is wired correctly end to end.
    assert!(
        frame.row_text(0).starts_with("WEBINAL"),
        "row 0 was {:?}",
        frame.row_text(0)
    );
}

/// Drive the modal flow without a browser or a terminal: `:` opens the
/// command line, typing fills it, and what it holds parses to a navigation.
#[test]
fn the_command_line_opens_fills_and_closes() {
    // The keymap decides that `:` opens an empty command line.
    let vp = wb_frame::Viewport::new(
        wb_frame::GridSize { cols: 80, rows: 24 },
        wb_frame::CellSize { w: 9, h: 20 },
    );
    let action = webinal::keymap::action_for(
        KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE),
        vp,
    );
    let Some(webinal::keymap::Action::EnterCommand(prefill)) = action else {
        panic!("`:` should open the command line, got {action:?}");
    };
    let mut mode = Mode::Command(prefill);

    // Typing accumulates, and the chrome row shows it.
    if let Mode::Command(buffer) = &mut mode {
        for c in "open example.com".chars() {
            buffer.push(c);
        }
    }
    let mut frame = wb_frame::Frame::new(wb_frame::GridSize { cols: 40, rows: 3 });
    webinal::chrome::paint(&mut frame, &mode, &webinal::chrome::State::Ready, "", "", 0.0);
    assert!(
        frame.row_text(2).starts_with(":open example.com"),
        "row 2 was {:?}",
        frame.row_text(2)
    );

    // And the command it holds parses to the navigation we expect.
    if let Mode::Command(buffer) = &mode {
        assert_eq!(
            webinal::command::parse(buffer),
            Ok(webinal::command::Command::Open("https://example.com".to_string()))
        );
    }
}
