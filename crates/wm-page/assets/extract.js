// Extract every visible text run, one per line of text, with its layout box.
//
// M1 measures each character's rect individually and groups by rounded top.
// That is O(n) ranges per text node and slow on large pages, but it is exact
// and needs no heuristics about where lines break. M2 replaces the inner loop
// with a binary search over character offsets.
(() => {
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

    // Group the node's characters into lines by their rounded top edge.
    const lines = new Map();
    for (let i = 0; i < text.length; i++) {
      range.setStart(node, i);
      range.setEnd(node, i + 1);
      const r = range.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) continue;

      const key = Math.round(r.top);
      let line = lines.get(key);
      if (!line) {
        line = { chars: [], left: r.left, right: r.right, top: r.top, bottom: r.bottom };
        lines.set(key, line);
      }
      line.chars.push(text[i]);
      line.left = Math.min(line.left, r.left);
      line.right = Math.max(line.right, r.right);
      line.bottom = Math.max(line.bottom, r.bottom);
    }

    const fontSize = parseFloat(cs.fontSize) || 16;
    const weight = parseInt(cs.fontWeight, 10) || 400;

    for (const line of lines.values()) {
      const content = line.chars.join("").replace(/\s+/g, " ").trim();
      if (!content) continue;

      // Cull runs entirely outside the viewport.
      if (line.bottom < 0 || line.top > vh || line.right < 0 || line.left > vw) {
        continue;
      }

      runs.push({
        text: content,
        x: line.left,
        y: line.top,
        w: line.right - line.left,
        h: line.bottom - line.top,
        // The descender is roughly a fifth of the font size; close enough to
        // put the baseline in the right cell row.
        baseline: line.bottom - fontSize * 0.21,
        color: cs.color,
        bold: weight >= 600,
        z: 0,
      });
    }
  }

  return { runs, title: document.title };
})()
