// Math overlay: detect LaTeX math on the visible xterm.js screen and draw
// KaTeX-typeset boxes over the raw source cells. Pure presentation — the
// terminal grid, the child process, and the engine's screen model never see
// any difference; toggling it off simply removes the boxes.
//
// The child (an AI CLI, a REPL, a pager) repaints freely, so overlays are
// recomputed from the rendered screen: every render schedules a debounced
// rescan, and each detected region is keyed by (row, col, source) so an
// unchanged formula keeps its DOM node — no flicker while output streams.

/* global katex */

// ---- detection ----------------------------------------------------------

// Delimiters, most specific first: $$…$$ and \[…\] are display math,
// \(…\) and $…$ inline. $…$ needs heuristics — `$` is everywhere in shell
// output — so a candidate only counts when the content looks like math.
const RE_REGION = /\$\$([^$]+?)\$\$|\\\[([\s\S]+?)\\\]|\\\(([\s\S]+?)\\\)|\$([^$\n]+?)\$/g;

// Something a formula would contain and a shell variable / price would not:
// a TeX command, super/subscript, braces, or an operator between operands.
const RE_MATHY = /\\[a-zA-Z]+|[_^{}]|[^\s]\s*[=+\-*/<>]\s*[^\s]/;

// Reject inline-$ candidates that are clearly money or code: "$5", "$PATH$"
// style (opener followed by space / closer preceded by space are rejected by
// the surrounding checks below).
function plausibleInline(src) {
  if (!RE_MATHY.test(src)) return false;
  if (/^\s|\s$/.test(src)) return false; // "$ x$" / "$x $": not TeX convention
  return true;
}

// Scan one logical line of screen text; returns {start, end, src, display}
// index ranges (end exclusive, delimiters included).
export function detectMath(text) {
  const out = [];
  RE_REGION.lastIndex = 0;
  let m;
  while ((m = RE_REGION.exec(text)) !== null) {
    const [whole, dd, br, par, sd] = m;
    const src = dd ?? br ?? par ?? sd;
    const display = dd != null || br != null;
    if (sd != null) {
      // Bare-$ candidate: apply the heuristics, and require the opener not
      // to sit inside a word/number ("US$40", "a$b$c" stays text).
      const prev = m.index > 0 ? text[m.index - 1] : '';
      if (/[\w$]/.test(prev) || !plausibleInline(sd)) continue;
    } else if (!src.trim()) {
      continue;
    }
    out.push({ start: m.index, end: m.index + whole.length, src: src.trim(), display });
  }
  return out;
}

// ---- overlay ------------------------------------------------------------

const RESCAN_DEBOUNCE_MS = 120;
// A display block's opener and closer may sit rows apart ("\[" on its own
// line); look this far down for the closer before giving up.
const MAX_BLOCK_ROWS = 12;

export class MathOverlay {
  constructor(term, holder) {
    this.term = term;
    this.holder = holder;
    this.root = document.createElement('div');
    this.root.className = 'math-overlay';
    holder.appendChild(this.root);
    this.boxes = new Map(); // key -> element
    this.cache = new Map(); // src|mode -> katex HTML
    this.enabled = false;
    this.timer = 0;
    this.disposables = [];
  }

  setEnabled(on) {
    if (this.enabled === on) return;
    this.enabled = on;
    if (on) {
      if (typeof katex === 'undefined') {
        console.warn('math overlay: katex not vendored — run `npm run vendor`');
        this.enabled = false;
        return;
      }
      this.disposables.push(
        this.term.onRender(() => this.schedule()),
        this.term.onScroll(() => this.schedule()),
        this.term.onResize(() => this.schedule())
      );
      this.schedule();
    } else {
      for (const d of this.disposables) d.dispose();
      this.disposables = [];
      clearTimeout(this.timer);
      this.clear();
    }
  }

  schedule() {
    clearTimeout(this.timer);
    this.timer = setTimeout(() => this.scan(), RESCAN_DEBOUNCE_MS);
  }

  clear() {
    this.root.replaceChildren();
    this.boxes.clear();
  }

  dispose() {
    this.setEnabled(false);
    this.root.remove();
  }

  // Untrimmed viewport rows: fixed cols-wide strings so a string index maps
  // straight back to a column.
  viewportRows() {
    const buf = this.term.buffer.active;
    const rows = [];
    for (let r = 0; r < this.term.rows; r++) {
      const line = buf.getLine(buf.viewportY + r);
      rows.push(line ? line.translateToString(false) : ' '.repeat(this.term.cols));
    }
    return rows;
  }

  scan() {
    if (!this.enabled) return;
    const screen = this.holder.querySelector('.xterm-screen');
    if (!screen || !this.term.cols || !this.term.rows) return;
    const srect = screen.getBoundingClientRect();
    if (!srect.width) return; // hidden tab — nothing to place
    const hrect = this.holder.getBoundingClientRect();
    const cellW = srect.width / this.term.cols;
    const cellH = srect.height / this.term.rows;
    const offX = srect.left - hrect.left;
    const offY = srect.top - hrect.top;

    const buf = this.term.buffer.active;
    const cursorRow = buf.baseY + buf.cursorY - buf.viewportY;
    const rows = this.viewportRows();
    const regions = [];

    // Per-row inline/display regions.
    for (let r = 0; r < rows.length; r++) {
      for (const d of detectMath(rows[r])) {
        regions.push({ row: r, rowSpan: 1, col: d.start, colSpan: d.end - d.start, ...d });
      }
    }

    // Multi-row display blocks: an unmatched \[ or $$ opener whose closer
    // arrives on a later row. Rows already claimed by a same-row region keep
    // priority (RE_REGION consumed matched pairs, so leftovers are unmatched).
    const claimed = new Set(regions.map((g) => g.row));
    for (let r = 0; r < rows.length; r++) {
      if (claimed.has(r)) continue;
      const open = rows[r].match(/\\\[|\$\$/);
      if (!open) continue;
      const closerRe = open[0] === '$$' ? /\$\$/ : /\\\]/;
      for (let e = r + 1; e < Math.min(rows.length, r + MAX_BLOCK_ROWS); e++) {
        if (claimed.has(e)) break;
        const close = rows[e].match(closerRe);
        if (!close) continue;
        const body = [rows[r].slice(open.index + open[0].length)]
          .concat(rows.slice(r + 1, e))
          .concat(rows[e].slice(0, close.index))
          .join('\n')
          .trim();
        if (body) {
          const colStart = Math.min(open.index, ...rows.slice(r + 1, e + 1).map((t) => {
            const w = t.search(/\S/);
            return w < 0 ? Infinity : w;
          }));
          regions.push({
            row: r,
            rowSpan: e - r + 1,
            col: Number.isFinite(colStart) ? colStart : open.index,
            colSpan: this.term.cols - (Number.isFinite(colStart) ? colStart : open.index),
            src: body,
            display: true,
          });
          for (let k = r; k <= e; k++) claimed.add(k);
        }
        break;
      }
    }

    // Rebuild the overlay set, reusing unchanged boxes so streaming output
    // doesn't flicker every debounce tick.
    const keep = new Set();
    for (const g of regions) {
      // Never cover the row being typed on.
      if (cursorRow >= g.row && cursorRow < g.row + g.rowSpan) continue;
      const html = this.typeset(g.src, g.display);
      if (!html) continue; // unparseable: leave the raw text visible
      const key = `${g.row}:${g.col}:${g.rowSpan}:${g.display}:${g.src}`;
      keep.add(key);
      let el = this.boxes.get(key);
      if (!el) {
        el = document.createElement('div');
        el.className = `math-box ${g.display ? 'display' : 'inline'}`;
        el.innerHTML = html;
        this.root.appendChild(el);
        this.boxes.set(key, el);
      }
      el.style.left = `${offX + g.col * cellW}px`;
      el.style.top = `${offY + g.row * cellH}px`;
      el.style.minWidth = `${g.colSpan * cellW}px`;
      el.style.minHeight = `${g.rowSpan * cellH}px`;
      el.style.maxWidth = `${(this.term.cols - g.col) * cellW}px`;
      el.style.fontSize = `${this.term.options.fontSize + 2}px`;
    }
    for (const [key, el] of this.boxes) {
      if (!keep.has(key)) {
        el.remove();
        this.boxes.delete(key);
      }
    }
  }

  typeset(src, display) {
    const key = `${display ? 'D' : 'I'}|${src}`;
    let html = this.cache.get(key);
    if (html == null) {
      try {
        html = katex.renderToString(src, { displayMode: display, throwOnError: false });
      } catch {
        html = null; // ParseError with throwOnError:false is rare but possible
      }
      // Unparseable "math" stays raw terminal text (empty box would hide it).
      if (html == null) html = '';
      this.cache.set(key, html);
      if (this.cache.size > 500) this.cache.delete(this.cache.keys().next().value);
    }
    return html;
  }
}
