//! What a screencast does, against a real browser.

mod common;

use std::time::Duration;

use common::{harness, open, open_url, runtime, viewport};
use wwt_page::{Page, ScreencastFrame};

/// Wait for the next frame belonging to `page`, or give up.
///
/// Frames arrive on the same subscription every other event does, so this is
/// the shape every caller of `screencast_frame` has.
async fn next_frame(client: &wwt_cdp::Client, page: &Page) -> ScreencastFrame {
    let mut events = client.subscribe();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.expect("the browser is still there");
            if let Some(frame) = page.screencast_frame(&event) {
                return frame;
            }
        }
    })
    .await
    .expect("a frame within ten seconds")
}

#[test]
fn a_started_screencast_produces_a_frame() {
    let h = harness();
    runtime().block_on(async {
        let page = open(&h, "simple.html").await;
        page.activate().await.expect("bring it to the front");
        page.start_screencast(viewport().css_width(), viewport().css_height())
            .await
            .expect("start the screencast");

        let frame = next_frame(&h.client, &page).await;
        assert!(!frame.data.is_empty(), "a frame carries a picture");
        // Base64 PNG and nothing else: the whole design rests on never
        // having to decode this, so it must arrive already encoded.
        assert!(
            frame.data.starts_with("iVBOR"),
            "base64 PNG, got {:?}",
            &frame.data[..frame.data.len().min(8)]
        );

        page.ack_frame(frame.ack).await.expect("ack it");
        page.stop_screencast().await.expect("stop it");
    });
}

#[test]
fn a_screencast_keeps_producing_frames_while_they_are_acked() {
    // Chromium stops sending after one unacked frame, so this is the test
    // that catches forgetting the ack: without it the second never arrives.
    let h = harness();
    runtime().block_on(async {
        let page = open_url(
            &h,
            "data:text/html,<body><div id=x>a</div><script>\
             setInterval(()=>{document.getElementById('x').textContent=Math.random()},50)\
             </script></body>",
        )
        .await;
        page.activate().await.expect("bring it to the front");
        page.start_screencast(viewport().css_width(), viewport().css_height()).await.expect("start");

        for _ in 0..3 {
            let frame = next_frame(&h.client, &page).await;
            page.ack_frame(frame.ack).await.expect("ack it");
        }

        page.stop_screencast().await.expect("stop");
    });
}

#[test]
fn a_frame_from_another_page_is_not_this_ones() {
    // One browser, one subscription, several pages. Without the session id
    // the wrong tab's picture lands on the tab in front.
    let h = harness();
    runtime().block_on(async {
        let one = open(&h, "simple.html").await;
        let two = open(&h, "simple.html").await;
        two.activate().await.expect("bring it to the front");

        let mut events = h.client.subscribe();
        two.start_screencast(viewport().css_width(), viewport().css_height()).await.expect("start");

        let event = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = events.recv().await.expect("the browser is still there");
                if two.screencast_frame(&event).is_some() {
                    return event;
                }
            }
        })
        .await
        .expect("a frame within ten seconds");

        assert!(
            one.screencast_frame(&event).is_none(),
            "a frame belongs to the page whose session it names"
        );
        two.stop_screencast().await.expect("stop");
    });
}
