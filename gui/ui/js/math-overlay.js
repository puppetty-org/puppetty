// Math overlay: detect LaTeX math on the visible xterm.js screen and draw
// KaTeX-typeset boxes over the raw source cells. Pure presentation — the
// terminal grid, the child process, and the engine's screen model never see
// any difference; toggling it off simply removes the boxes.
//
// The child (an AI CLI, a REPL, a pager) repaints freely, so overlays are
// recomputed from the rendered screen: every render schedules a debounced
// rescan, and each detected region is keyed by (row, col, source) so an
// unchanged formula keeps its DOM node — no flicker while output streams.
//
// Soft-wrapped rows are joined into logical lines before scanning, so a
// formula the terminal wrapped mid-`\partial` still typesets; the overlay
// then covers every wrapped fragment (the typeset box on the first, blank
// covers on the rest).

/* global katex */

// ---- detection ----------------------------------------------------------

// Delimiters, most specific first: $$…$$ and \[…\] are display math,
// \(…\) and $…$ inline. $…$ needs heuristics — `$` is everywhere in shell
// output — so a candidate only counts when the content looks like math.
const RE_REGION = /\$\$([^$]+?)\$\$|\\\[([\s\S]+?)\\\]|\\\(([\s\S]+?)\\\)|\$([^$\n]+?)\$/g;

// Something a formula would contain and a shell variable / price would not:
// a TeX command, super/subscript, braces, or an operator between operands.
const RE_MATHY = /\\[a-zA-Z]+|[_^{}]|[^\s]\s*[=+\-*/<>]\s*[^\s]/;

// Reject inline-$ candidates that are clearly money or code. A single letter
// is accepted — `$n$`, `$h$` are the most common inline math in AI answers,
// while shell/price noise is never a lone letter between two dollars.
function plausibleInline(src) {
  if (/^\s|\s$/.test(src)) return false; // "$ x$" / "$x $": not TeX convention
  if (/^[a-zA-Z]$/.test(src)) return true;
  return RE_MATHY.test(src);
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
// A display block's opener and closer may sit lines apart ("\[" on its own
// line); look this far down for the closer before giving up.
const MAX_BLOCK_LINES = 12;

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

  // One row as a string whose index equals the cell column. Wide (CJK)
  // glyphs cover two cells but translateToString would give them one char,
  // drifting every index after them — so the row is built per cell, with a
  // space standing in for each wide char's spacer cell. Formulas are ASCII,
  // so the stand-in never lands inside a match; it only keeps columns true.
  rowText(line) {
    const cols = this.term.cols;
    if (!line) return ' '.repeat(cols);
    let s = '';
    for (let c = 0; c < cols; c++) {
      const cell = line.getCell(c);
      const chars = cell?.getChars() || '';
      if (cell && cell.getWidth() === 0) s += ' '; // wide char's spacer cell
      // Multi-unit graphemes (emoji, combining marks) get a one-unit
      // placeholder — they can never be part of a formula, and the string
      // must stay exactly one UTF-16 unit per column.
      else s += chars.length === 1 ? chars : chars ? '�' : ' ';
    }
    return s;
  }

  // Viewport rows joined into logical lines: a run of soft-wrapped rows is
  // one line of text as the child wrote it. Rows are exactly `cols` wide
  // (see rowText), so an index into the joined text maps straight back to a
  // (row, col) cell.
  logicalLines() {
    const buf = this.term.buffer.active;
    const lines = [];
    for (let r = 0; r < this.term.rows; r++) {
      const line = buf.getLine(buf.viewportY + r);
      const text = this.rowText(line);
      if (line?.isWrapped && lines.length) {
        const prev = lines[lines.length - 1];
        prev.text += text;
        prev.rowCount++;
      } else {
        lines.push({ startRow: r, rowCount: 1, text });
      }
    }
    return lines;
  }

  // Split a [start, end) index range of a logical line into per-row cell
  // fragments {row, col, colSpan}.
  fragmentsOf(startRow, start, end) {
    const cols = this.term.cols;
    const frags = [];
    for (let i = start; i < end; ) {
      const stop = Math.min(end, (Math.floor(i / cols) + 1) * cols);
      frags.push({ row: startRow + Math.floor(i / cols), col: i % cols, colSpan: stop - i });
      i = stop;
    }
    return frags;
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
    const lines = this.logicalLines();
    const regions = []; // {src, display, frags} | {src, display: true, block}

    // Inline/display regions within one logical line.
    const claimed = new Set(); // logical-line indices that had a match
    for (let li = 0; li < lines.length; li++) {
      const L = lines[li];
      for (const d of detectMath(L.text)) {
        regions.push({
          src: d.src,
          display: d.display,
          frags: this.fragmentsOf(L.startRow, d.start, d.end),
        });
        claimed.add(li);
      }
    }

    // Multi-line display blocks. `\[` opens anywhere on an (unclaimed) line
    // with no `\]` after it; `$$` only when alone on its line — a bare `$$`
    // in prose (or the closer of a block scrolled half off-screen) must not
    // pair with an unrelated marker and swallow the text between them.
    for (let li = 0; li < lines.length; li++) {
      if (claimed.has(li)) continue;
      const text = lines[li].text;
      const dollars = text.trim() === '$$';
      const brIdx = text.indexOf('\\[');
      if (!dollars && (brIdx < 0 || text.indexOf('\\]', brIdx) >= 0)) continue;
      for (let le = li + 1; le < Math.min(lines.length, li + MAX_BLOCK_LINES); le++) {
        if (claimed.has(le)) break;
        const closeText = lines[le].text;
        const closeIdx = dollars ? (closeText.trim() === '$$' ? 0 : -1) : closeText.indexOf('\\]');
        if (closeIdx < 0) continue;
        const body = [dollars ? '' : text.slice(brIdx + 2)]
          .concat(lines.slice(li + 1, le).map((l) => l.text.trimEnd()))
          .concat(dollars ? '' : closeText.slice(0, closeIdx))
          .join('\n')
          .trim();
        if (body) {
          const indents = lines.slice(li, le + 1).map((l) => {
            const w = l.text.search(/\S/);
            return w < 0 ? Infinity : w;
          });
          const col = Math.min(...indents.filter(Number.isFinite), this.term.cols - 1);
          const row = lines[li].startRow;
          const endLine = lines[le];
          regions.push({
            src: body,
            display: true,
            block: {
              row,
              rowSpan: endLine.startRow + endLine.rowCount - row,
              col,
              colSpan: this.term.cols - col,
            },
          });
          for (let k = li; k <= le; k++) claimed.add(k);
        }
        break;
      }
    }

    // Rebuild the overlay set, reusing unchanged boxes so streaming output
    // doesn't flicker every debounce tick.
    const keep = new Set();
    const place = (el, row, col, colSpan, rowSpan) => {
      el.style.left = `${offX + col * cellW}px`;
      el.style.top = `${offY + row * cellH}px`;
      el.style.minWidth = `${colSpan * cellW}px`;
      el.style.minHeight = `${rowSpan * cellH}px`;
      el.style.maxWidth = `${(this.term.cols - col) * cellW}px`;
      el.style.fontSize = `${this.term.options.fontSize + 2}px`;
    };
    const ensure = (key, className, html) => {
      keep.add(key);
      let el = this.boxes.get(key);
      if (!el) {
        el = document.createElement('div');
        el.className = className;
        el.innerHTML = html;
        this.root.appendChild(el);
        this.boxes.set(key, el);
      }
      return el;
    };
    for (const g of regions) {
      const rows = g.block
        ? { first: g.block.row, last: g.block.row + g.block.rowSpan - 1 }
        : { first: g.frags[0].row, last: g.frags[g.frags.length - 1].row };
      // Never cover the row being typed on.
      if (cursorRow >= rows.first && cursorRow <= rows.last) continue;
      const html = this.typeset(g.src, g.display);
      if (!html) continue; // unparseable: leave the raw text visible
      const base = `${rows.first}:${g.display}:${g.src}`;
      if (g.block) {
        place(
          ensure(`${base}:${g.block.col}`, 'math-box display', html),
          g.block.row, g.block.col, g.block.colSpan, g.block.rowSpan
        );
      } else {
        // Typeset box over the first fragment; bare covers over the rest of
        // a wrapped formula so its raw tail doesn't peek out underneath.
        g.frags.forEach((f, i) => {
          const el = i === 0
            ? ensure(`${base}:${f.col}`, `math-box ${g.display ? 'display' : 'inline'}`, html)
            : ensure(`${base}:${f.col}:${i}`, 'math-box blank', '');
          place(el, f.row, f.col, f.colSpan, 1);
        });
      }
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
        html = ''; // ParseError with throwOnError:false is rare but possible
      }
      // katex-error spans mean the "math" wasn't (mispaired delimiters,
      // prose, an unsupported macro) — leave the raw text visible instead
      // of drawing red garbage over it.
      if (html.includes('katex-error')) html = '';
      this.cache.set(key, html);
      if (this.cache.size > 500) this.cache.delete(this.cache.keys().next().value);
    }
    return html;
  }
}
