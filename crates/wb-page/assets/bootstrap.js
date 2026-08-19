// Installed once per document via Page.addScriptToEvaluateOnNewDocument, so
// it survives navigation. It defines the extraction entry point and the
// listeners that tell us the page changed.
//
// The extraction body asks each text node for its line boxes directly with
// getClientRects, then binary-searches character offsets to find where the
// text splits between them: O(lines * log chars) forced layouts rather than
// the O(chars) of measuring every character.
(() => {
  if (window.__webinal) return;

  // Trailing debounces. Mutations are bursty and cheap to coalesce; scroll
  // fires per frame and must not outrun a single extraction.
  const MUTATION_DEBOUNCE_MS = 50;
  const SCROLL_DEBOUNCE_MS = 16;

  function signal() {
    // The binding may not be installed yet on the very first document.
    if (typeof window.__webinal_dirty === "function") {
      try {
        window.__webinal_dirty("");
      } catch (e) {
        // A torn-down context is not worth reporting.
      }
    }
  }

  function debounce(fn, ms) {
    let timer = null;
    return () => {
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(() => {
        timer = null;
        fn();
      }, ms);
    };
  }

  const onMutation = debounce(signal, MUTATION_DEBOUNCE_MS);
  const onScroll = debounce(signal, SCROLL_DEBOUNCE_MS);

  // `document` exists even at document-start, so the observer can be
  // attached before there is a body to observe.
  new MutationObserver(onMutation).observe(document, {
    subtree: true,
    childList: true,
    characterData: true,
    attributes: true,
  });

  // Capture, because scrolling inside a nested scroller does not bubble.
  window.addEventListener("scroll", onScroll, { passive: true, capture: true });
  window.addEventListener("load", signal);

  // How far to scan forward past characters with no box (collapsed
  // whitespace) before giving up and using the caller's fallback.
  const EMPTY_SCAN_LIMIT = 8;

  // The top edge of the character at `index`, skipping over characters that
  // have no box of their own.
  function topAt(range, node, index, fallback) {
    const limit = Math.min(node.nodeValue.length, index + EMPTY_SCAN_LIMIT);
    for (let k = index; k < limit; k++) {
      range.setStart(node, k);
      range.setEnd(node, k + 1);
      const rect = range.getBoundingClientRect();
      if (rect.width > 0 || rect.height > 0) return rect.top;
    }
    return fallback;
  }

  // Split a text node into one entry per line box.
  //
  // getClientRects gives us the line boxes directly, so the only unknown is
  // where in the string each line begins. Character tops increase
  // monotonically through the string, so each boundary is a binary search
  // rather than a scan.
  function linesOf(range, node) {
    const text = node.nodeValue;
    range.selectNodeContents(node);
    const rects = Array.from(range.getClientRects()).filter(
      (r) => r.width > 0 || r.height > 0
    );
    if (rects.length === 0) return [];
    if (rects.length === 1) {
      return [{ rect: rects[0], text }];
    }

    const lines = [];
    let start = 0;
    for (let i = 1; i < rects.length; i++) {
      // The first offset that has moved down to line i.
      const threshold = rects[i].top - 0.5;
      let lo = start;
      let hi = text.length;
      while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (topAt(range, node, mid, rects[i - 1].top) >= threshold) {
          hi = mid;
        } else {
          lo = mid + 1;
        }
      }
      lines.push({ rect: rects[i - 1], text: text.slice(start, lo) });
      start = lo;
    }
    lines.push({ rect: rects[rects.length - 1], text: text.slice(start) });
    return lines;
  }

  function extract() {
    const runs = [];
    const vw = window.innerWidth;
    const vh = window.innerHeight;

    const walker = document.createTreeWalker(
      document.body,
      NodeFilter.SHOW_TEXT,
      null
    );

    const range = document.createRange();
    let node;

    while ((node = walker.nextNode())) {
      const text = node.nodeValue;
      if (!text || !text.trim()) continue;

      const parent = node.parentElement;
      if (!parent) continue;

      const cs = window.getComputedStyle(parent);
      if (cs.visibility === "hidden" || cs.display === "none" || cs.opacity === "0") {
        continue;
      }
      if (parent.tagName === "SCRIPT" || parent.tagName === "STYLE") continue;

      const fontSize = parseFloat(cs.fontSize) || 16;
      const weight = parseInt(cs.fontWeight, 10) || 400;

      for (const line of linesOf(range, node)) {
        const content = line.text.replace(/\s+/g, " ").trim();
        if (!content) continue;

        const r = line.rect;
        // Cull runs entirely outside the viewport.
        if (r.bottom < 0 || r.top > vh || r.right < 0 || r.left > vw) continue;

        runs.push({
          text: content,
          x: r.left,
          y: r.top,
          w: r.width,
          h: r.height,
          // The descender is roughly a fifth of the font size; close enough to
          // put the baseline in the right cell row.
          baseline: r.bottom - fontSize * 0.21,
          color: cs.color,
          bold: weight >= 600,
          z: 0,
        });
      }
    }

    // Scroll geometry rides along with the runs so the statusline costs no
    // extra round trip.
    const doc = document.documentElement;
    return {
      runs,
      title: document.title,
      url: location.href,
      scrollY: window.scrollY,
      scrollHeight: Math.max(doc.scrollHeight, document.body ? document.body.scrollHeight : 0),
      innerHeight: window.innerHeight,
    };
  }

  window.__webinal = { extract };
})()
