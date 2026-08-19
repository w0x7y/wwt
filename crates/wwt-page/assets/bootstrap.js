// Installed once per document via Page.addScriptToEvaluateOnNewDocument, so
// it survives navigation. It defines the extraction entry point and the
// listeners that tell us the page changed.
//
// The extraction body asks each text node for its line boxes directly with
// getClientRects, then binary-searches character offsets to find where the
// text splits between them: O(lines * log chars) forced layouts rather than
// the O(chars) of measuring every character.
(() => {
  if (window.__wwt) return;

  // Trailing debounces. Mutations are bursty and cheap to coalesce; scroll
  // fires per frame and must not outrun a single extraction.
  const MUTATION_DEBOUNCE_MS = 50;
  const SCROLL_DEBOUNCE_MS = 16;

  function signal() {
    // The binding may not be installed yet on the very first document.
    if (typeof window.__wwt_dirty === "function") {
      try {
        window.__wwt_dirty("");
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

  const OBSERVER_OPTIONS = {
    subtree: true,
    childList: true,
    characterData: true,
    attributes: true,
  };

  // `document` exists even at document-start, so the observer can be
  // attached before there is a body to observe.
  const observer = new MutationObserver(onMutation);
  observer.observe(document, OBSERVER_OPTIONS);

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
      return [{ rect: rects[0], text, start: 0, end: text.length }];
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
      lines.push({ rect: rects[i - 1], text: text.slice(start, lo), start, end: lo });
      start = lo;
    }
    lines.push({
      rect: rects[rects.length - 1],
      text: text.slice(start),
      start,
      end: text.length,
    });
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

    // A form control's text is not a text node, so the walk above missed it.
    const fields = fieldRuns();
    runs.push(...fields.runs);

    // Scroll geometry rides along with the runs so the statusline costs no
    // extra round trip.
    const doc = document.documentElement;
    return {
      runs,
      caret: fields.caret,
      title: document.title,
      url: location.href,
      scrollY: window.scrollY,
      scrollHeight: Math.max(doc.scrollHeight, document.body ? document.body.scrollHeight : 0),
      innerHeight: window.innerHeight,
    };
  }

  // The styles that decide where a character lands. Copied onto the mirror so
  // that it breaks its lines in exactly the places the control does.
  const MIRROR_STYLES = [
    "fontStyle", "fontVariant", "fontWeight", "fontStretch", "fontSize",
    "fontFamily", "lineHeight", "letterSpacing", "wordSpacing",
    "textTransform", "textIndent", "textRendering", "wordBreak",
    "overflowWrap", "tabSize", "direction",
  ];

  // A stand-in for a control's text box, laid out by the same engine on the
  // same inputs.
  //
  // There is no Range inside an `input`, so this is the only way to learn
  // where its characters are: where the browser wrapped a line, which part of
  // a scrolled value is on screen, and where the insertion point sits.
  function makeMirror(cs, width, multiline) {
    const mirror = document.createElement("div");
    for (const property of MIRROR_STYLES) mirror.style[property] = cs[property];
    mirror.style.position = "absolute";
    mirror.style.top = "0";
    mirror.style.left = "0";
    // Laid out but not painted. `display: none` would throw away the boxes
    // this exists to measure.
    mirror.style.visibility = "hidden";
    mirror.style.margin = "0";
    mirror.style.padding = "0";
    mirror.style.border = "0";
    // A textarea wraps at its content width; a single-line control never
    // wraps however long its value is.
    mirror.style.whiteSpace = multiline ? "pre-wrap" : "pre";
    mirror.style.width = width + "px";
    return mirror;
  }

  // Measure with `mirror` in the document, then take it out again.
  //
  // Inserting it is a DOM mutation and our own observer is watching the whole
  // document, so left alone this would signal dirtiness, cause another
  // extraction, and spin forever on an idle page. Extraction is synchronous
  // and JavaScript is single-threaded, so nothing real can mutate across this
  // window; takeRecords drops the records we caused ourselves.
  function withMirror(mirror, measure) {
    observer.disconnect();
    document.body.appendChild(mirror);
    try {
      return measure();
    } finally {
      mirror.remove();
      observer.takeRecords();
      observer.observe(document, OBSERVER_OPTIONS);
    }
  }

  // The first offset in [lo, hi) whose character reaches past `x`.
  //
  // Character positions increase monotonically along a line, so this is a
  // binary search rather than a scan, the same trick linesOf uses vertically.
  function offsetPast(range, node, lo, hi, x) {
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      range.setStart(node, mid);
      range.setEnd(node, mid + 1);
      if (range.getBoundingClientRect().right > x) hi = mid;
      else lo = mid + 1;
    }
    return lo;
  }

  // Where the lines and the insertion point of one control are, relative to
  // the top left of its content box.
  function measureField(el, shown, multiline, width, focused) {
    const cs = window.getComputedStyle(el);
    const mirror = makeMirror(cs, width, multiline);
    const node = document.createTextNode(shown);
    mirror.appendChild(node);

    return withMirror(mirror, () => {
      const origin = mirror.getBoundingClientRect();
      const range = document.createRange();
      const scrollLeft = el.scrollLeft;
      const scrollTop = el.scrollTop;
      const lines = [];

      for (const line of linesOf(range, node)) {
        let text = line.text;
        let x = line.rect.left - origin.left;

        // A control scrolled sideways shows a window into its value, not the
        // head of it.
        if (scrollLeft > 0) {
          const cut = offsetPast(range, node, line.start, line.end, origin.left + scrollLeft);
          if (cut >= line.end) continue;
          range.setStart(node, cut);
          range.setEnd(node, line.end);
          x = range.getBoundingClientRect().left - origin.left;
          text = node.nodeValue.slice(cut, line.end);
        }

        // Trailing space is where a line was broken, not something to paint.
        lines.push({
          text: text.replace(/\s+$/, ""),
          x: x - scrollLeft,
          y: line.rect.top - origin.top - scrollTop,
        });
      }

      let caret = null;
      if (focused) {
        // Measured from a real character rather than from a collapsed range,
        // which browsers are inconsistent about at the end of a line.
        const offset = el.selectionStart;
        if (node.nodeValue.length === 0) {
          caret = { x: 0, y: 0 };
        } else if (offset > 0) {
          range.setStart(node, offset - 1);
          range.setEnd(node, offset);
          const rect = range.getBoundingClientRect();
          caret = { x: rect.right - origin.left - scrollLeft, y: rect.top - origin.top - scrollTop };
        } else {
          range.setStart(node, 0);
          range.setEnd(node, 1);
          const rect = range.getBoundingClientRect();
          caret = { x: rect.left - origin.left - scrollLeft, y: rect.top - origin.top - scrollTop };
        }
      }

      return { lines, caret };
    });
  }

  // Input types whose value is not text on screen. A checkbox's value is the
  // string "on", and painting that would be inventing content.
  const VALUELESS_INPUTS = new Set([
    "checkbox", "radio", "range", "color", "file", "image", "hidden",
  ]);

  // What a form control shows, which is not always what it holds.
  function fieldText(el) {
    if (el.tagName === "SELECT") {
      const option = el.selectedOptions && el.selectedOptions[0];
      return option ? option.label : "";
    }

    const type = el.tagName === "INPUT"
      ? (el.getAttribute("type") || "text").toLowerCase()
      : "";
    if (VALUELESS_INPUTS.has(type)) return "";

    const value = el.value || "";
    if (value === "") return el.placeholder || "";
    // The frame shows what the browser shows. A password on screen is
    // bullets, so a password in the frame is bullets.
    if (type === "password") return "\u2022".repeat(value.length);
    return value;
  }

  // Runs for the text inside form controls, and the insertion point.
  //
  // A control's value is not in the DOM: `input.childNodes` is empty however
  // much you type into it, because the browser paints the value from element
  // state rather than from a text node. The walk in extract() therefore
  // cannot see it, and without this pass you cannot see what you are typing.
  //
  // Measuring a control costs a mirror of it, so only the controls that need
  // one get one: a textarea, which may wrap; a control whose value overflows
  // or is scrolled, which shows a window into itself rather than its head;
  // and the focused one, whose insertion point has to be found. A plain field
  // showing all of its value costs nothing beyond the styles already read.
  function fieldRuns() {
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const runs = [];
    let caret = null;

    for (const el of document.querySelectorAll("input, textarea, select")) {
      const cs = window.getComputedStyle(el);
      if (cs.visibility === "hidden" || cs.display === "none" || cs.opacity === "0") {
        continue;
      }

      const r = el.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) continue;
      if (r.bottom < 0 || r.top > vh || r.right < 0 || r.left > vw) continue;

      const shown = fieldText(el);
      const focused =
        el === document.activeElement && typeof el.selectionStart === "number";
      if (!shown && !focused) continue;

      const left = r.left + parseFloat(cs.borderLeftWidth) + parseFloat(cs.paddingLeft);
      const top = r.top + parseFloat(cs.borderTopWidth) + parseFloat(cs.paddingTop);
      const width = Math.max(
        0,
        el.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight)
      );
      const height = Math.max(
        0,
        el.clientHeight - parseFloat(cs.paddingTop) - parseFloat(cs.paddingBottom)
      );

      const fontSize = parseFloat(cs.fontSize) || 16;
      // `line-height: normal` parses to NaN, and a control's own line height
      // is what decides which row its text lands on.
      const lineHeight = parseFloat(cs.lineHeight) || fontSize * 1.2;
      const color = cs.color;
      const bold = (parseInt(cs.fontWeight, 10) || 400) >= 600;

      const multiline = el.tagName === "TEXTAREA";
      // A single-line control centres its text in the box; a textarea starts
      // at the top of it.
      const centring = multiline ? 0 : Math.max(0, (height - lineHeight) / 2);

      const overflowing =
        el.scrollWidth > el.clientWidth || el.scrollHeight > el.clientHeight;
      const scrolled = el.scrollLeft > 0 || el.scrollTop > 0;
      const measurable = el.tagName !== "SELECT" && shown !== "";
      const needsMirror =
        measurable && (multiline || overflowing || scrolled || focused);

      if (!needsMirror) {
        if (shown) {
          const y = top + centring;
          runs.push({
            text: shown,
            x: left,
            y,
            w: width,
            h: lineHeight,
            baseline: y + lineHeight - fontSize * 0.21,
            color,
            bold,
            z: 0,
          });
        }
        // An empty field still has an insertion point, at the start of it.
        if (focused) {
          caret = { x: left, y: top + centring, w: 0, h: lineHeight };
        }
        continue;
      }

      const measured = measureField(el, shown, multiline, width, focused);

      for (const line of measured.lines) {
        if (!line.text) continue;
        const y = top + centring + line.y;
        // Scrolled out of the top or bottom of its own box.
        if (y + lineHeight <= top || y >= top + height) continue;

        runs.push({
          text: line.text,
          x: left + line.x,
          y,
          w: Math.max(0, width - line.x),
          h: lineHeight,
          baseline: y + lineHeight - fontSize * 0.21,
          color,
          bold,
          z: 0,
        });
      }

      if (measured.caret) {
        caret = {
          x: left + measured.caret.x,
          y: top + centring + measured.caret.y,
          w: 0,
          h: lineHeight,
        };
      }
    }

    return { runs, caret };
  }

  // What counts as interactive. Anything a click or a keystroke does
  // something to, which is broader than "has an href".
  const HINT_SELECTOR = [
    "a[href]",
    "button",
    "input:not([type=hidden])",
    "select",
    "textarea",
    "[contenteditable='']",
    "[contenteditable='true']",
    "[role=button]",
    "[role=link]",
    "[role=checkbox]",
    "[role=radio]",
    "[role=menuitem]",
    "[role=tab]",
    "[role=textbox]",
    "[tabindex]:not([tabindex='-1'])",
  ].join(",");

  // Input types you type into, as opposed to the ones you click.
  const TYPABLE = new Set([
    "text", "search", "email", "url", "tel", "password", "number",
    "date", "time", "month", "week", "datetime-local",
  ]);

  function isEditable(el) {
    if (el.isContentEditable) return true;
    if (el.tagName === "TEXTAREA") return true;
    if (el.tagName !== "INPUT") return false;
    return TYPABLE.has((el.getAttribute("type") || "text").toLowerCase());
  }

  // Interactive boxes, in document order.
  //
  // This is deliberately not part of extract(): it sweeps the whole
  // document and pays a hit test per candidate, and extraction runs on
  // every scroll frame. This runs when someone presses `f`.
  function hints() {
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const out = [];

    for (const el of document.querySelectorAll(HINT_SELECTOR)) {
      if (el.disabled) continue;

      const cs = window.getComputedStyle(el);
      if (cs.visibility === "hidden" || cs.display === "none" || cs.opacity === "0") {
        continue;
      }

      const r = el.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) continue;
      if (r.bottom < 0 || r.top > vh || r.right < 0 || r.left > vw) continue;

      // The point a click would land on. If something else is on top of it,
      // a label here would lie about what pressing it does.
      const x = Math.min(Math.max(r.left + r.width / 2, 0), vw - 1);
      const y = Math.min(Math.max(r.top + r.height / 2, 0), vh - 1);
      const hit = document.elementFromPoint(x, y);
      if (!hit) continue;
      if (hit !== el && !el.contains(hit) && !hit.contains(el)) continue;

      out.push({
        x: r.left,
        y: r.top,
        w: r.width,
        h: r.height,
        editable: isEditable(el),
      });
    }

    return out;
  }

  window.__wwt = { extract, hints };
})()
