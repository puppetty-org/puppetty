import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';

// Per-session logs (DESIGN.md Q6):
//   <name>-<ts>.cast   asciinema v2 — output stream only, replayable with
//                      `asciinema play`. Input is deliberately NOT recorded
//                      here: non-echoed input (passwords) must never land in
//                      a log file.
//   <name>-<ts>.jsonl  structured control events with source attribution
//                      (send/keys/wait/prompt/answer/cancel/kill/exit).
//                      Attached-terminal keystrokes are logged as byte counts
//                      only, never content — a human may be typing a secret.

export function logsDir() {
  const dir = path.join(os.homedir(), '.puppetty', 'logs');
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}

export function prune(dir, { retentionDays = 30, maxTotalMB = 200 } = {}) {
  let files;
  try {
    files = fs.readdirSync(dir).map((f) => {
      const p = path.join(dir, f);
      const st = fs.statSync(p);
      return { p, mtime: st.mtimeMs, size: st.size };
    });
  } catch {
    return;
  }
  files.sort((a, b) => a.mtime - b.mtime); // oldest first
  const cutoff = Date.now() - retentionDays * 86_400_000;
  let total = files.reduce((s, f) => s + f.size, 0);
  const maxBytes = maxTotalMB * 1024 * 1024;
  for (const f of files) {
    if (f.mtime >= cutoff && total <= maxBytes) break;
    try {
      fs.unlinkSync(f.p);
      total -= f.size;
    } catch {}
  }
}

export class EventLog {
  constructor({ name, command, cols, rows, logging = {} }) {
    const dir = logsDir();
    prune(dir, logging);
    const ts = new Date().toISOString().replace(/[:.]/g, '-');
    this.t0 = Date.now();
    this.castPath = path.join(dir, `${name}-${ts}.cast`);
    this.jsonlPath = path.join(dir, `${name}-${ts}.jsonl`);
    this.cast = fs.createWriteStream(this.castPath, { flags: 'a' });
    this.jsonl = fs.createWriteStream(this.jsonlPath, { flags: 'a' });
    this.cast.write(
      JSON.stringify({
        version: 2,
        width: cols,
        height: rows,
        timestamp: Math.floor(this.t0 / 1000),
        title: command,
      }) + '\n'
    );
    this.event('start', { command, cols, rows });
  }

  #t() {
    return Math.round((Date.now() - this.t0)) / 1000;
  }

  output(data) {
    this.cast.write(JSON.stringify([this.#t(), 'o', data]) + '\n');
  }

  event(type, detail = {}) {
    this.jsonl.write(
      JSON.stringify({ ts: new Date().toISOString(), t: this.#t(), type, ...detail }) + '\n'
    );
  }

  close(exitCode) {
    this.event('exit', { exitCode });
    this.cast.end();
    this.jsonl.end();
  }
}
