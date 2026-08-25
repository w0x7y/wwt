(() => {
  const CANDIDATE = "article, main, [role='main']";
  const EXCLUDED = new Set([
    "SCRIPT", "STYLE", "TEMPLATE", "NOSCRIPT", "NAV", "ASIDE", "FORM", "DIALOG",
  ]);
  const BLOCK = new Set([
    "ADDRESS", "ARTICLE", "ASIDE", "BLOCKQUOTE", "DETAILS", "DIALOG", "DIV", "DL",
    "FIELDSET", "FIGCAPTION", "FIGURE", "FOOTER", "FORM", "HEADER", "HGROUP", "HR",
    "MAIN", "MENU", "NAV", "OL", "P", "PRE", "SEARCH", "SECTION", "TABLE", "UL",
    "DT", "DD", "H1", "H2", "H3", "H4", "H5", "H6",
  ]);
  const styles = new WeakMap();
  const blocks = [];
  const links = [];

  function computed(element) {
    let style = styles.get(element);
    if (!style) {
      style = getComputedStyle(element);
      styles.set(element, style);
    }
    return style;
  }

  function excludedElement(element) {
    if (EXCLUDED.has(element.tagName)) return true;
    if ((element.tagName === "HEADER" || element.tagName === "FOOTER") &&
        !element.closest("article")) return true;
    if (element.hidden || element.getAttribute("aria-hidden") === "true") return true;
    const style = computed(element);
    return style.display === "none" || style.visibility === "hidden" || style.opacity === "0";
  }

  function excludedByTree(element) {
    for (let current = element; current; current = current.parentElement) {
      if (excludedElement(current)) return true;
    }
    return false;
  }

  function candidateText(root, skipNestedCandidates) {
    let plain = 0;
    let linked = 0;
    let hasContent = false;
    let spacePending = false;
    let pendingInLink = false;

    function count(value, inLink) {
      for (const character of value) {
        if (/\s/u.test(character)) {
          if (hasContent) {
            spacePending = true;
            pendingInLink = inLink;
          }
          continue;
        }
        if (spacePending) {
          if (pendingInLink) linked += 1;
          else plain += 1;
        }
        spacePending = false;
        if (inLink) linked += 1;
        else plain += 1;
        hasContent = true;
      }
    }

    function visit(node, inLink) {
      if (node.nodeType === Node.TEXT_NODE) {
        count(node.nodeValue || "", inLink);
        return;
      }
      if (node.nodeType !== Node.ELEMENT_NODE) return;
      const element = node;
      if (excludedElement(element)) return;
      if (skipNestedCandidates && element !== root && element.matches(CANDIDATE)) return;
      const nextInLink = inLink || element.tagName === "A";
      for (const child of element.childNodes) visit(child, nextInLink);
    }

    visit(root, false);
    return { plain, linked };
  }

  const candidates = Array.from(document.querySelectorAll(CANDIDATE)).filter((element) => {
    if (excludedByTree(element)) return false;
    const text = candidateText(element, false);
    return text.plain + text.linked > 0;
  });
  let root = document.body;
  let bestScore = -1;
  for (const candidate of candidates) {
    const text = candidateText(candidate, true);
    const score = text.plain + text.linked / 4;
    if (score > bestScore) {
      root = candidate;
      bestScore = score;
    }
  }

  function appendSpan(spans, text, link) {
    if (!text) return;
    const previous = spans[spans.length - 1];
    if (previous && previous.link === link) previous.text += text;
    else spans.push({ text, link });
  }

  function ordinaryBuffer() {
    const spans = [];
    let spacePending = false;
    let pendingLink = null;
    let hasContent = false;

    return {
      write(value, link) {
        for (const character of value) {
          const code = character.codePointAt(0);
          if ((code < 32 || (code >= 127 && code <= 159)) && !/\s/u.test(character)) continue;
          if (/\s/u.test(character)) {
            if (hasContent) {
              spacePending = true;
              pendingLink = link;
            }
            continue;
          }
          if (spacePending) appendSpan(spans, " ", pendingLink);
          spacePending = false;
          pendingLink = null;
          appendSpan(spans, character, link);
          hasContent = true;
        }
      },
      break() {
        spacePending = false;
        pendingLink = null;
        if (hasContent && !spans[spans.length - 1].text.endsWith("\n")) {
          appendSpan(spans, "\n", null);
        }
      },
      finish() {
        while (spans.length > 0) {
          spans[spans.length - 1].text = spans[spans.length - 1].text.replace(/[\s]+$/u, "");
          if (spans[spans.length - 1].text) break;
          spans.pop();
        }
        return spans;
      },
    };
  }

  function destination(anchor) {
    const raw = anchor.getAttribute("href");
    if (raw === null || raw.trim() === "" || /^javascript:/iu.test(raw.trim())) return null;
    const id = links.length;
    links.push({ url: anchor.href, newTab: anchor.target.toLowerCase() === "_blank" });
    return id;
  }

  function collectInline(node, buffer, link = null, skipLists = false) {
    if (node.nodeType === Node.TEXT_NODE) {
      buffer.write(node.nodeValue || "", link);
      return;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return;
    const element = node;
    if (excludedElement(element)) return;
    if (skipLists && (element.tagName === "UL" || element.tagName === "OL")) return;
    if (element.tagName === "BR") {
      buffer.break();
      return;
    }
    if (element.tagName === "IMG") {
      const alt = element.getAttribute("alt");
      if (alt) buffer.write(`[${alt}]`, link);
      return;
    }
    let childLink = link;
    if (element.tagName === "A") childLink = destination(element);
    for (const child of element.childNodes) collectInline(child, buffer, childLink, skipLists);
  }

  function emit(kind, spans, fields = {}) {
    if (!spans.some((span) => /\S/u.test(span.text))) return;
    blocks.push({ kind, ...fields, spans });
  }

  function emitInline(element, kind, fields = {}, skipLists = false) {
    const buffer = ordinaryBuffer();
    collectInline(element, buffer, null, skipLists);
    emit(kind, buffer.finish(), fields);
  }

  function paragraphKind(context) {
    return context.quoteDepth > 0
      ? { kind: "quote", fields: { depth: context.quoteDepth } }
      : { kind: "paragraph", fields: {} };
  }

  function serializeContainer(element, context) {
    let buffer = ordinaryBuffer();
    const flush = () => {
      const paragraph = paragraphKind(context);
      emit(paragraph.kind, buffer.finish(), paragraph.fields);
      buffer = ordinaryBuffer();
    };

    for (const child of element.childNodes) {
      if (child.nodeType === Node.TEXT_NODE) {
        buffer.write(child.nodeValue || "", null);
        continue;
      }
      if (child.nodeType !== Node.ELEMENT_NODE) continue;
      const childElement = child;
      if (excludedElement(childElement)) continue;
      if (BLOCK.has(childElement.tagName)) {
        flush();
        serializeBlock(childElement, context);
      } else {
        collectInline(childElement, buffer);
      }
    }
    flush();
  }

  function listOrdinal(item, running) {
    const value = item.getAttribute("value");
    if (value === null) return running;
    const parsed = Number.parseInt(value, 10);
    return Number.isFinite(parsed) ? parsed : running;
  }

  function serializeList(list, context) {
    const items = Array.from(list.children).filter((child) => child.tagName === "LI");
    const ordered = list.tagName === "OL";
    const reversed = ordered && list.reversed;
    let running = ordered ? (list.hasAttribute("start") ? list.start : (reversed ? items.length : 1)) : 0;

    for (const item of items) {
      const ordinal = listOrdinal(item, running);
      const buffer = ordinaryBuffer();
      collectInline(item, buffer, null, true);
      emit(
        ordered ? "ordered-list-item" : "unordered-list-item",
        buffer.finish(),
        ordered ? { depth: context.listDepth, ordinal } : { depth: context.listDepth },
      );
      for (const child of item.children) {
        if (child.tagName === "UL" || child.tagName === "OL") {
          serializeList(child, { ...context, listDepth: context.listDepth + 1 });
        }
      }
      if (ordered) running = ordinal + (reversed ? -1 : 1);
    }
  }

  function serializeTable(table) {
    for (const row of table.rows) {
      const spans = [];
      for (const cell of row.cells) {
        const buffer = ordinaryBuffer();
        collectInline(cell, buffer);
        const cellSpans = buffer.finish();
        if (!cellSpans.some((span) => /\S/u.test(span.text))) continue;
        if (spans.length > 0) appendSpan(spans, " | ", null);
        for (const span of cellSpans) appendSpan(spans, span.text, span.link);
      }
      emit("paragraph", spans);
    }
  }

  function collectPreformatted(node, spans, link = null) {
    if (node.nodeType === Node.TEXT_NODE) {
      const value = Array.from(node.nodeValue || "")
        .filter((character) => {
          const code = character.codePointAt(0);
          return character === "\n" || (code >= 32 && !(code >= 127 && code <= 159));
        })
        .join("");
      appendSpan(spans, value, link);
      return;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return;
    const element = node;
    if (excludedElement(element)) return;
    if (element.tagName === "BR") {
      appendSpan(spans, "\n", null);
      return;
    }
    if (element.tagName === "IMG") {
      const alt = element.getAttribute("alt");
      if (alt) appendSpan(spans, `[${alt}]`, link);
      return;
    }
    let childLink = link;
    if (element.tagName === "A") childLink = destination(element);
    for (const child of element.childNodes) collectPreformatted(child, spans, childLink);
  }

  function preformatted(element) {
    const spans = [];
    for (const child of element.childNodes) collectPreformatted(child, spans);
    emit("preformatted", spans);
  }

  function serializeBlock(element, context) {
    const tag = element.tagName;
    if (/^H[1-6]$/.test(tag)) {
      emitInline(element, "heading", { level: Number(tag.slice(1)) });
    } else if (tag === "P" || tag === "DT" || tag === "DD") {
      const paragraph = paragraphKind(context);
      emitInline(element, paragraph.kind, paragraph.fields);
    } else if (tag === "UL" || tag === "OL") {
      serializeList(element, context);
    } else if (tag === "BLOCKQUOTE") {
      serializeContainer(element, { ...context, quoteDepth: context.quoteDepth + 1 });
    } else if (tag === "PRE") {
      preformatted(element);
    } else if (tag === "TABLE") {
      serializeTable(element);
    } else {
      serializeContainer(element, context);
    }
  }

  if (root && !excludedByTree(root)) serializeContainer(root, { quoteDepth: 0, listDepth: 0 });

  const doc = document.documentElement;
  return {
    blocks,
    links,
    title: document.title,
    url: location.href,
    scrollY: window.scrollY,
    scrollHeight: Math.max(doc.scrollHeight, document.body ? document.body.scrollHeight : 0),
    innerHeight: window.innerHeight,
  };
})()
