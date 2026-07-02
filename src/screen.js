import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const { Terminal } = require('@xterm/headless');
const { SerializeAddon } = require('@xterm/addon-serialize');

// Headless xterm.js screen model: feed raw PTY output in, read the rendered
// screen (what a human would see) back out.
export class Screen {
  constructor(cols, rows) {
    this.term = new Terminal({ cols, rows, scrollback: 5_000, allowProposedApi: true });
    this.serializer = new SerializeAddon();
    this.term.loadAddon(this.serializer);
  }

  // Escape-sequence string that restores the current screen (and up to
  // `scrollback` history lines) on another xterm — used for attach, instead
  // of replaying raw history full of stale repaints.
  async serialize({ scrollback = 1_000 } = {}) {
    await this.flush();
    return this.serializer.serialize({ scrollback });
  }

  write(data) {
    this.term.write(data);
  }

  resize(cols, rows) {
    this.term.resize(cols, rows);
  }

  // Resolves after all previously written data has been parsed.
  flush() {
    return new Promise((resolve) => this.term.write('', resolve));
  }

  async snapshot({ scrollback = false } = {}) {
    await this.flush();
    const buf = this.term.buffer.active;
    const start = scrollback ? 0 : buf.baseY;
    const end = buf.baseY + this.term.rows;
    const lines = [];
    for (let i = start; i < end; i++) {
      const line = buf.getLine(i);
      lines.push(line ? line.translateToString(true) : '');
    }
    // Drop trailing blank lines so `read` output stays compact.
    while (lines.length > 1 && lines[lines.length - 1] === '') lines.pop();
    return {
      lines,
      cursor: { x: buf.cursorX, y: buf.baseY + buf.cursorY - start },
    };
  }
}
