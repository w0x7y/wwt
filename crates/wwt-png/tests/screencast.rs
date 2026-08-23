//! The fixture is a screenshot of a page painted `#ff0000`, taken by the
//! probe in Task 1 of the M6 plan. Every synthetic test in `src` agrees
//! with this crate's idea of a PNG; only this one agrees with Chromium's.

#[test]
fn a_real_screencast_frame_decodes_to_the_colour_the_page_was() {
    let base64 = include_str!("fixtures/screencast.txt");
    let image = wwt_png::decode_base64(base64.trim()).expect("decode the fixture");

    assert!(image.width > 0 && image.height > 0, "{}x{}", image.width, image.height);
    assert_eq!(
        image.pixels.len(),
        image.width * image.height * 4,
        "four bytes a pixel, whatever the file's colour type was"
    );

    // The page was solid red. Sampling the middle rather than a corner
    // avoids whatever a scrollbar or a border might be doing at an edge.
    let middle = ((image.height / 2) * image.width + image.width / 2) * 4;
    assert_eq!(&image.pixels[middle..middle + 4], &[255, 0, 0, 255]);
}

#[test]
fn the_bytes_and_the_base64_are_the_same_picture() {
    let bytes = include_bytes!("fixtures/screencast.png");
    let base64 = include_str!("fixtures/screencast.txt");
    assert_eq!(
        wwt_png::decode(bytes).expect("bytes"),
        wwt_png::decode_base64(base64.trim()).expect("base64")
    );
}
