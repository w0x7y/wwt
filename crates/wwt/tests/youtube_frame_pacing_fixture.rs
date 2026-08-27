use std::sync::Arc;

use wwt::session::page_viewport;
use wwt_cdp::{Chromium, Client};
use wwt_frame::{CellSize, GridSize};
use wwt_page::Page;

const GRID: GridSize = GridSize {
    cols: 190,
    rows: 50,
};
const CELL: CellSize = CellSize { w: 10, h: 20 };
const SAMPLE: &str = include_str!("youtube_frame_pacing_sample.js");

#[tokio::test]
async fn stale_global_watch_links_do_not_count_as_watch_page_recommendations() {
    let browser = Chromium::launch(None, None)
        .await
        .expect("launch fixture Chromium");
    let client = Arc::new(Client::connect(browser.ws_url()).await.expect("connect"));
    client.auto_attach().await.expect("turn on auto-attach");
    let page = Page::open(
        Arc::clone(&client),
        "about:blank",
        page_viewport(GRID, CELL),
    )
    .await
    .expect("open fixture page");
    page.eval(
        r##"(() => {
          document.body.innerHTML = `
            <main id="player" style="width: 1000px; height: 500px"></main>
            <aside id="secondary"></aside>
            <section id="stale-results"></section>`;
          const stale = document.querySelector("#stale-results");
          for (let index = 0; index < 40; index += 1) {
            const link = document.createElement("a");
            link.href = `/watch?v=stale-${index}`;
            link.textContent = `stale result ${index}`;
            link.style.display = "block";
            stale.append(link);
          }
        })()"##,
    )
    .await
    .expect("arrange stale search results");

    let sample = page.eval(SAMPLE).await.expect("sample fixture");

    assert_eq!(sample["visibleWatchLinkCount"], 40);
    assert_eq!(sample["recommendationCount"], 0);

    page.eval(
        r##"(() => {
          const secondary = document.querySelector("#secondary");
          secondary.style.cssText = "position: absolute; left: 1100px; top: 0; width: 300px";
          for (let index = 0; index < 12; index += 1) {
            const link = document.createElement("a");
            link.href = `/watch?v=recommended-${index}`;
            link.textContent = `recommendation ${index}`;
            link.style.display = "block";
            secondary.append(link);
          }
        })()"##,
    )
    .await
    .expect("arrange right-column recommendations");

    let sample = page.eval(SAMPLE).await.expect("sample fixture");

    assert_eq!(sample["containerRecommendationCount"], 12);
    assert_eq!(sample["rightColumnRecommendationCount"], 12);
    assert_eq!(sample["recommendationCount"], 12);
}
