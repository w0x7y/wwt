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
  // Field state changes at typing speed, and a caret that lags behind the
  // keystroke that moved it is worse than no caret at all.
  const FIELD_DEBOUNCE_MS = 16;

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
  const onField = debounce(signal, FIELD_DEBOUNCE_MS);

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
  // A form control's value and selection are element state rather than DOM:
  // nothing mutates when you type into an `input` or walk the insertion
  // point through it, so the observer above sees none of it. The second
  // extraction pass reads exactly that state, and these are its dirty
  // source; without them what you typed and where the caret sits stay on
  // screen as they were until something unrelated changed the page.
  //
  // Focus is on the list because it decides whether a control has an
  // insertion point at all. This does not let the page drive the mode: it
  // repaints what the page already looks like, and only a keystroke ever
  // changes what mode we are in.
  document.addEventListener("input", onField, true);
  document.addEventListener("selectionchange", onField, true);
  document.addEventListener("focusin", onField, true);
  document.addEventListener("focusout", onField, true);

  window.addEventListener("load", signal);

  // Whether an element is painted at all. Every pass over the document asks
  // this, and a pass that asked it differently would put text on screen that
  // the page does not show.
  function isVisible(cs) {
    return (
      cs.visibility !== "hidden" && cs.display !== "none" && cs.opacity !== "0"
    );
  }

  // Whether a box overlaps the viewport. The dimensions are passed in rather
  // than read here: they are a layout read, and every caller is in a loop.
  function onScreen(r, vw, vh) {
    return r.bottom >= 0 && r.top <= vh && r.right >= 0 && r.left <= vw;
  }

  // The descender is roughly a fifth of the font size. Close enough to put a
  // baseline in the right cell row, and the one number that decides which row
  // any text lands on, so it is stated once.
  const DESCENDER = 0.21;

  function baselineOf(bottom, fontSize) {
    return bottom - fontSize * DESCENDER;
  }

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

  // The first offset in [lo, hi) for which `past` is true.
  //
  // Both searches this script does are this one: character positions
  // increase monotonically down a wrapped string and along a line, so
  // finding where one crosses an edge is a bisection rather than a scan.
  // Pure, and the only part of either search that can be wrong on its own.
  function firstWhere(lo, hi, past) {
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (past(mid)) hi = mid;
      else lo = mid + 1;
    }
    return lo;
  }

  // Split a string into one entry per line box.
  //
  // The line boxes come from getClientRects, so the only unknown is where in
  // the string each line begins. `topOf(index, fallback)` answers where a
  // character sits vertically; everything else here is arithmetic, which is
  // why the two are separated.
  function splitLines(rects, text, topOf) {
    if (rects.length === 0) return [];
    if (rects.length === 1) {
      return [{ rect: rects[0], text, start: 0, end: text.length }];
    }

    const lines = [];
    let start = 0;
    for (let i = 1; i < rects.length; i++) {
      // The first offset that has moved down to line i.
      const threshold = rects[i].top - 0.5;
      const previous = rects[i - 1].top;
      const boundary = firstWhere(
        start,
        text.length,
        (k) => topOf(k, previous) >= threshold
      );
      lines.push({
        rect: rects[i - 1],
        text: text.slice(start, boundary),
        start,
        end: boundary,
      });
      start = boundary;
    }
    lines.push({
      rect: rects[rects.length - 1],
      text: text.slice(start),
      start,
      end: text.length,
    });
    return lines;
  }

  // Split a text node into one entry per line box.
  function linesOf(range, node) {
    const text = node.nodeValue;
    range.selectNodeContents(node);
    const rects = Array.from(range.getClientRects()).filter(
      (r) => r.width > 0 || r.height > 0
    );
    return splitLines(rects, text, (index, fallback) =>
      topAt(range, node, index, fallback)
    );
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
      if (!isVisible(cs)) continue;
      if (parent.tagName === "SCRIPT" || parent.tagName === "STYLE") continue;

      const fontSize = parseFloat(cs.fontSize) || 16;
      const weight = parseInt(cs.fontWeight, 10) || 400;

      for (const line of linesOf(range, node)) {
        const content = line.text.replace(/\s+/g, " ").trim();
        if (!content) continue;

        const r = line.rect;
        // Cull runs entirely outside the viewport.
        if (!onScreen(r, vw, vh)) continue;

        runs.push({
          text: content,
          x: r.left,
          y: r.top,
          w: r.width,
          h: r.height,
          baseline: baselineOf(r.bottom, fontSize),
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
  function offsetPast(range, node, lo, hi, x) {
    return firstWhere(lo, hi, (mid) => {
      range.setStart(node, mid);
      range.setEnd(node, mid + 1);
      return range.getBoundingClientRect().right > x;
    });
  }

  // The insertion point, as a line and a count of characters into it.
  //
  // Deliberately not a pixel position. The frame paints a run one character
  // per cell from the column its box starts in, so a caret placed by CSS
  // pixels drifts away from the character it belongs beside as soon as the
  // font is not exactly one cell wide. Counting characters is what the
  // painting does, so counting characters is what the caret does.
  function caretIn(lines, selection) {
    if (lines.length === 0) return null;

    // The last line that starts at or before the selection. A soft wrap
    // leaves one offset belonging to two lines; the browser puts the caret
    // at the start of the second, so we do too.
    let index = 0;
    for (let i = 1; i < lines.length; i++) {
      if (lines[i].start <= selection) index = i;
    }

    const line = lines[index];
    // Clamped: the offset can sit in the space a line was broken at, which
    // is not painted, and it can sit left of a scrolled window.
    const offset = Math.min(Math.max(selection - line.start, 0), line.text.length);
    return { x: line.x, y: line.y, offset };
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
        let cut = line.start;

        // A control scrolled sideways shows a window into its value, not the
        // head of it.
        if (scrollLeft > 0) {
          cut = offsetPast(range, node, line.start, line.end, origin.left + scrollLeft);
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
          // Where this line's first painted character sits in the value,
          // which is what turns the insertion point into a column.
          start: cut,
        });
      }

      return { lines, caret: focused ? caretIn(lines, el.selectionStart) : null };
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
      if (!isVisible(cs)) continue;

      const r = el.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) continue;
      if (!onScreen(r, vw, vh)) continue;

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

      // A control that needs no mirror is a control whose value is one line
      // at the origin of its content box, with its insertion point at the
      // start of it. That is the shape a measured control comes back in, so
      // saying it here is what lets one emit path below serve both.
      const measured = needsMirror
        ? measureField(el, shown, multiline, width, focused)
        : {
            lines: [{ text: shown, x: 0, y: 0, start: 0 }],
            caret: focused ? { x: 0, y: 0, offset: 0 } : null,
          };

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
          baseline: baselineOf(y + lineHeight, fontSize),
          color,
          bold,
          z: 0,
        });
      }

      if (measured.caret) {
        // The same x and baseline the run for that line was given, so the
        // caret counts cells from the cell that line's first character
        // landed in.
        const y = top + centring + measured.caret.y;
        caret = {
          x: left + measured.caret.x,
          baseline: baselineOf(y + lineHeight, fontSize),
          offset: measured.caret.offset,
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
      if (!isVisible(cs)) continue;

      const r = el.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) continue;
      if (!onScreen(r, vw, vh)) continue;

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

  // The arithmetic half, exposed so it can be asserted on directly.
  //
  // These are the sharpest functions here and the ones whose mistakes are
  // invisible from a rendered frame: an offset that is two too far still
  // looks like a caret in roughly the right place. Reaching them needs no
  // page, only data, so their tests cost data.
  window.__wwt = { extract, hints, __pure: { firstWhere, splitLines, caretIn } };
})()
