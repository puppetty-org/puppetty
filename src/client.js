import net from 'node:net';
import fs from 'node:fs';
import path from 'node:path';
import { pipePath, sessionsDir, metaPath } from './session.js';

export function request(name, msg, timeoutMs = 10_000) {
  return new Promise((resolve, reject) => {
    const sock = net.connect(pipePath(name));
    let buf = '';
    const timer = setTimeout(() => {
      sock.destroy();
      reject(new Error(`session "${name}" did not respond within ${timeoutMs}ms`));
    }, timeoutMs);

    sock.on('connect', () => sock.write(JSON.stringify(msg) + '\n'));
    sock.on('data', (d) => {
      buf += d.toString();
      const nl = buf.indexOf('\n');
      if (nl === -1) return;
      clearTimeout(timer);
      sock.end();
      try {
        resolve(JSON.parse(buf.slice(0, nl)));
      } catch (err) {
        reject(err);
      }
    });
    sock.on('error', (err) => {
      clearTimeout(timer);
      reject(new Error(`cannot reach session "${name}" (${err.code || err.message})`));
    });
  });
}

// List sessions from the registry, pinging each; stale entries are removed.
export async function listSessions() {
  const dir = sessionsDir();
  const out = [];
  for (const f of fs.readdirSync(dir).filter((f) => f.endsWith('.json'))) {
    const name = path.basename(f, '.json');
    let meta;
    try {
      meta = JSON.parse(fs.readFileSync(path.join(dir, f), 'utf8'));
    } catch {
      continue;
    }
    try {
      const info = await request(name, { op: 'info' }, 2_000);
      out.push({ ...meta, ...info });
    } catch {
      try { fs.unlinkSync(metaPath(name)); } catch {}
    }
  }
  return out;
}

// Pick a session name that is not currently live.
export async function freeName(base) {
  const live = new Set((await listSessions()).map((s) => s.name));
  if (!live.has(base)) return base;
  for (let i = 2; ; i++) {
    if (!live.has(`${base}-${i}`)) return `${base}-${i}`;
  }
}
