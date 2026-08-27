(() => {
  const state = window.__wwtYoutubeFramePacingProbe || { raf: 0, longTasks: [] };
  const video = document.querySelector("video");
  const progressCandidates = Array.from(document.querySelectorAll(
    "yt-page-navigation-progress, yt-page-navigation-progress #progress, [id=progress]"
  )).map((element) => {
    const candidateStyle = getComputedStyle(element);
    const rect = element.getBoundingClientRect();
    return {
      tag: element.tagName,
      id: element.id,
      width: rect.width,
      height: rect.height,
      top: rect.top,
      display: candidateStyle.display,
      visibility: candidateStyle.visibility,
      opacity: Number(candidateStyle.opacity || 1),
      transform: candidateStyle.transform,
      backgroundColor: candidateStyle.backgroundColor
    };
  });
  const progress = progressCandidates.find((candidate) =>
    candidate.display !== "none" && candidate.visibility !== "hidden" &&
    candidate.opacity > 0 && candidate.width > 0 && candidate.height > 0 &&
    candidate.height <= 10 && candidate.top <= 10
  );
  const metadata = document.querySelector(
    "ytd-watch-metadata #title h1, ytd-watch-metadata h1, #above-the-fold #title h1"
  );
  const visible = (element) => {
    const rect = element.getBoundingClientRect();
    const style = getComputedStyle(element);
    return rect.width > 0 && rect.height > 0 && style.display !== "none" &&
      style.visibility !== "hidden";
  };
  const recommendationModelSelector =
    "ytd-compact-video-renderer, ytd-rich-item-renderer, yt-lockup-view-model, yt-lockup-metadata-view-model";
  const recommendationLinkSelector = "a[href*='/watch'], a[href*='youtu.be/']";
  const recommendationContainers = Array.from(document.querySelectorAll(
    "#secondary, #related, ytd-watch-next-secondary-results-renderer"
  ));
  const containerStats = recommendationContainers.map((container) => {
    const models = Array.from(container.querySelectorAll(recommendationModelSelector))
      .filter(visible);
    const links = Array.from(container.querySelectorAll(recommendationLinkSelector))
      .filter(visible);
    return {
      element: container,
      modelCount: models.length,
      linkCount: links.length,
      count: Math.max(models.length, links.length)
    };
  });
  const populatedContainer = containerStats.reduce(
    (best, candidate) => !best || candidate.count > best.count ? candidate : best,
    null
  );
  const containerRecommendationCount = populatedContainer ? populatedContainer.count : 0;
  const globalWatchLinks = Array.from(document.querySelectorAll("a[href*='/watch']"));
  const visibleWatchLinks = globalWatchLinks.filter(visible);
  const player = document.querySelector("#movie_player") || document.querySelector("#player");
  const playerRect = player ? player.getBoundingClientRect() : null;
  const rightColumnWatchLinks = playerRect ? visibleWatchLinks.filter((link) =>
    link.getBoundingClientRect().left >= playerRect.right - 1
  ) : [];
  const rightColumnRecommendationCount = rightColumnWatchLinks.length;
  const recommendationCount = Math.max(
    containerRecommendationCount,
    rightColumnRecommendationCount
  );
  const secondary = populatedContainer ? populatedContainer.element : null;
  const search = document.querySelector(
    "ytd-searchbox input, yt-searchbox input, input#search, input[name='search_query']"
  );
  const tasks = state.longTasks || [];
  return {
    href: location.href,
    title: document.title,
    now: performance.now(),
    raf: state.raf || 0,
    longTaskCount: tasks.length,
    longTaskTotal: tasks.reduce((sum, task) => sum + task.duration, 0),
    longTaskMax: tasks.reduce((max, task) => Math.max(max, task.duration), 0),
    videoTime: video ? video.currentTime : null,
    videoPaused: video ? video.paused : null,
    videoReadyState: video ? video.readyState : null,
    adPlaying: !!document.querySelector(".ad-showing, .video-ads.ytp-ad-module:not(:empty)"),
    youtubeLoadingBar: !!progress,
    youtubeProgressTransform: progress ? progress.transform : null,
    youtubeProgressCandidates: progressCandidates,
    metadataPresent: !!metadata && !!metadata.textContent.trim(),
    metadataText: metadata ? metadata.textContent.trim().slice(0, 160) : null,
    recommendationCount,
    recommendationContainerCount: recommendationContainers.length,
    containerRecommendationCount,
    rightColumnRecommendationCount,
    recommendationModelCount: populatedContainer ? populatedContainer.modelCount : 0,
    recommendationLinkCount: populatedContainer ? populatedContainer.linkCount : 0,
    globalWatchLinkCount: globalWatchLinks.length,
    visibleWatchLinkCount: visibleWatchLinks.length,
    visibleWatchLinkText: visibleWatchLinks.slice(0, 5).map((link) => link.innerText.trim()),
    secondaryPresent: !!secondary,
    secondaryTextLength: secondary ? secondary.innerText.length : 0,
    secondaryText: secondary ? secondary.innerText.slice(0, 300) : null,
    searchPresent: !!search,
    watchPlayerPresent: !!document.querySelector("#movie_player"),
    bodyTextLength: document.body ? document.body.innerText.length : 0
  };
})()
