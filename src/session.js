import net from 'node:net';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { createRequire } from 'node:module';
import { Screen } from './screen.js';
import { isPromptish, evaluate } from './policy.js';

const require = createRequire(import.meta.url);
const pty = require('@lydell/node-pty');

export const KEYMAP = {
  enter: '\r',
  tab: '\t',
  esc: '\x1b',
  space: ' ',
  backspace: '\x7f',
  up: '\x1b[A',
  down: '\x1b[B',
  right: '\x1b[C',
  left: '\x1b[D',
  home: '\x1b[H',
  end: '\x1b[F',
  pageup: '\x1b[5~',
  pagedown: '\x1b[6~',
  'ctrl-c': '\x03',
  'ctrl-d': '\x04',
  'ctrl-z': '\x1a',
};

export function sessionsDir() {
  const dir = path.join(os.homedir(), '.puppetty', 'sessions');
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}

export function pipePath(name) {
  return process.platform === 'win32'
    ? `\\\\.\\pipe\\puppetty-${name}`
    : path.join(os.tmpdir(), `puppetty-${name}.sock`);
}

export function metaPath(name) {
  return path.join(sessionsDir(), `${name}.json`);
}

// ConPTY needs a real executable path — resolve bare names via PATH/PATHEXT.
export function resolveExecutable(cmd) {
  if (process.platform !== 'win32') return cmd;
  if (cmd.includes('/') || cmd.includes('\\')) return cmd;
  const exts = (process.env.PATHEXT || '.COM;.EXE;.BAT;.CMD').split(';');
  for (const dir of (process.env.PATH || '').split(path.delimiter)) {
    if (!dir) continue;
    for (const ext of ['', ...exts]) {
      const full = path.join(dir, cmd + ext.toLowerCase());
      try {
        if (fs.statSync(full).isFile()) return full;
      } catch {}
    }
  }
  return cmd;
}

export class Session {
  constructor({ name, command, args = [], cols = 120, rows = 30, cwd, onData, onExit, logger = null, policy = null }) {
    this.name = name;
    this.command = command;
    this.args = args;
    this.cols = cols;
    this.rows = rows;
    this.cwd = cwd ?? process.cwd();
    this.onData = onData;
    this.onExit = onExit;
    this.logger = logger;
    this.policy = policy; // used to classify prompts for read/wait clients
    this.exited = false;
    this.exitCode = null;
    this.lastData = Date.now();
    this.dataListeners = new Set();
    this.attachClients = new Map(); // sock -> { source }
    this.startedAt = new Date().toISOString();
  }

  start() {
    let file = resolveExecutable(this.command);
    let args = this.args;
    if (/\.(bat|cmd)$/i.test(file)) {
      args = ['/c', file, ...args];
      file = process.env.ComSpec || 'cmd.exe';
    }

    this.screen = new Screen(this.cols, this.rows);
    this.pty = pty.spawn(file, args, {
      name: 'xterm-256color',
      cols: this.cols,
      rows: this.rows,
      cwd: this.cwd,
      env: process.env,
    });

    this.pty.onData((data) => {
      this.lastData = Date.now();
      this.screen.write(data);
      this.logger?.output(data);
      this.onData?.(data);
      for (const listener of this.dataListeners) listener();
      for (const sock of this.attachClients.keys()) {
        try { sock.write(JSON.stringify({ event: 'data', data }) + '\n'); } catch {}
      }
    });

    this.pty.onExit(({ exitCode }) => {
      this.exited = true;
      this.exitCode = exitCode ?? 0;
      for (const sock of this.attachClients.keys()) {
        try { sock.write(JSON.stringify({ event: 'exit', exitCode: this.exitCode }) + '\n'); } catch {}
      }
      this.logger?.close(this.exitCode);
      // Keep the server alive briefly so clients can read the final screen.
      setTimeout(() => this.dispose(), 3_000).unref?.();
      this.onExit?.(this.exitCode);
    });

    this.server = net.createServer((sock) => this.#serve(sock));
    this.server.listen(pipePath(this.name));
    this.server.on('error', () => {}); // client resets etc. must not kill the host

    fs.writeFileSync(
      metaPath(this.name),
      JSON.stringify(
        {
          name: this.name,
          pid: this.pty.pid,
          hostPid: process.pid,
          command: [this.command, ...this.args].join(' '),
          cols: this.cols,
          rows: this.rows,
          cwd: this.cwd,
          startedAt: this.startedAt,
          pipe: pipePath(this.name),
        },
        null,
        2
      )
    );
  }

  write(data) {
    if (!this.exited) this.pty.write(data);
  }

  resize(cols, rows) {
    this.cols = cols;
    this.rows = rows;
    if (!this.exited) this.pty.resize(cols, rows);
    this.screen.resize(cols, rows);
  }

  kill() {
    if (!this.exited) {
      try { this.pty.kill(); } catch {}
    }
  }

  dispose() {
    try { this.server?.close(); } catch {}
    try { fs.unlinkSync(metaPath(this.name)); } catch {}
  }

  #serve(sock) {
    sock.on('error', () => {});
    let buf = '';
    sock.on('data', async (chunk) => {
      buf += chunk.toString();
      let nl;
      while ((nl = buf.indexOf('\n')) !== -1) {
        const line = buf.slice(0, nl);
        buf = buf.slice(nl + 1);
        if (!line.trim()) continue;
        let req;
        try {
          req = JSON.parse(line);
        } catch (err) {
          sock.write(JSON.stringify({ ok: false, error: err.message }) + '\n');
          continue;
        }
        if (this.attachClients.has(sock)) {
          this.#handleAttached(sock, req);
        } else if (req.op === 'attach') {
          this.#attach(sock, req);
        } else {
          const reply = await this.#handle(req);
          sock.write(JSON.stringify(reply) + '\n');
        }
      }
    });
  }

  // Attach: the connection stays open. Server pushes {event:'data'|'exit'}
  // lines; the client may send {op:'input'|'resize'|'detach'} lines.
  // If the client sends its cols/rows, the session is resized to match FIRST,
  // then the restore is generated — so it always fits the client's terminal.
  async #attach(sock, req) {
    const source = req.source ?? 'attach';
    this.attachClients.set(sock, { source });
    this.logger?.event('attach', { source });
    sock.on('close', () => {
      if (this.attachClients.delete(sock)) this.logger?.event('detach', { source });
    });
    if (req.cols && req.rows && (req.cols !== this.cols || req.rows !== this.rows)) {
      this.resize(req.cols, req.rows);
    }
    sock.write(
      JSON.stringify({
        event: 'attached',
        name: this.name,
        cols: this.cols,
        rows: this.rows,
        alive: !this.exited,
        exitCode: this.exitCode,
      }) + '\n'
    );
    if (req.replay !== false) {
      // Serialized screen restore from the screen model — NOT raw history,
      // which is full of stale repaints for old terminal sizes.
      const restore = await this.screen.serialize();
      if (restore) {
        sock.write(JSON.stringify({ event: 'data', data: restore, replay: true }) + '\n');
      }
    }
    if (this.exited) {
      sock.write(JSON.stringify({ event: 'exit', exitCode: this.exitCode }) + '\n');
    }
  }

  #handleAttached(sock, req) {
    const { source } = this.attachClients.get(sock);
    switch (req.op) {
      case 'input':
        // Content is never logged: a human may be typing a secret.
        this.logger?.event('stdin', { bytes: (req.data ?? '').length, source });
        this.write(req.data ?? '');
        break;
      case 'resize':
        this.resize(req.cols, req.rows);
        break;
      case 'detach':
        sock.end();
        break;
    }
  }

  async #handle(req) {
    const source = req.source ?? 'pipe';
    switch (req.op) {
      case 'info':
        return {
          ok: true,
          name: this.name,
          pid: this.pty.pid,
          command: [this.command, ...this.args].join(' '),
          cwd: this.cwd,
          startedAt: this.startedAt,
          alive: !this.exited,
          exitCode: this.exitCode,
          cols: this.cols,
          rows: this.rows,
        };
      case 'send': {
        if (this.exited) return { ok: false, error: 'process has exited' };
        this.logger?.event('send', { text: req.data, enter: !!req.enter, source });
        this.write(req.data ?? '');
        if (req.enter) {
          // Small gap so TUI apps register the text before the Enter key.
          await new Promise((r) => setTimeout(r, req.enterDelay ?? 50));
          this.write('\r');
        }
        return { ok: true };
      }
      case 'keys': {
        if (this.exited) return { ok: false, error: 'process has exited' };
        this.logger?.event('keys', { keys: req.keys, source });
        for (const key of req.keys ?? []) {
          const seq = KEYMAP[key];
          if (!seq) return { ok: false, error: `unknown key: ${key}` };
          this.write(seq);
          await new Promise((r) => setTimeout(r, 30));
        }
        return { ok: true };
      }
      case 'read': {
        const snap = await this.screen.snapshot({ scrollback: !!req.scrollback });
        return { ok: true, alive: !this.exited, exitCode: this.exitCode, ...snap };
      }
      case 'wait':
        return this.#wait(req, source);
      case 'resize': {
        this.resize(req.cols, req.rows);
        return { ok: true };
      }
      case 'kill': {
        this.logger?.event('kill', { source });
        setTimeout(() => this.kill(), 10);
        return { ok: true };
      }
      case 'set-auto': {
        // Attach/detach the policy autopilot on a live session (GUI toggle).
        // The host wires this.onSetAuto; without it, auto isn't available here.
        if (typeof this.onSetAuto !== 'function') return { ok: false, error: 'auto toggle not supported' };
        const auto = this.onSetAuto(!!req.enabled);
        this.logger?.event('set-auto', { enabled: !!req.enabled, source });
        return { ok: true, auto };
      }
      default:
        return { ok: false, error: `unknown op: ${req.op}` };
    }
  }

  // Block until the first of (any combination may be requested):
  //   pattern — screen matches regex; with sinceStart, only lines that
  //             changed after the wait began are matched (full-render TUIs
  //             keep stale text visible — DESIGN.md §4.4)
  //   gone    — regex does NOT appear on screen (resolves immediately if it
  //             never appeared; the "esc to interrupt" done-detector)
  //   stable  — rendered screen unchanged for N ms (animation-proof idle)
  //   prompt  — screen stable ≥ quietMs and last line looks like a prompt
  //             (controller mode: the driving agent answers via send)
  //   idle    — no PTY output for idleMs (byte-level; kept as a heuristic)
  //   exit / timeout — always active
  async #wait(req, source = 'pipe') {
    const timeoutMs = req.timeoutMs ?? 60_000;
    const hasCondition =
      req.pattern || req.gone || req.stable != null || req.prompt || req.idleMs != null;
    const idleMs = req.idleMs ?? (hasCondition ? null : 2_000);

    let regex = null;
    let goneRe = null;
    try {
      if (req.pattern) regex = new RegExp(req.pattern, req.flags ?? '');
      if (req.gone) goneRe = new RegExp(req.gone, req.flags ?? '');
    } catch (err) {
      return { ok: false, error: `bad pattern: ${err.message}` };
    }

    const baseline = req.sinceStart ? (await this.screen.snapshot()).lines : null;
    const started = Date.now();
    let lastText = null;
    let stableSince = Date.now();
    let checking = false;

    const reason = await new Promise((resolve) => {
      const finish = (why) => {
        clearInterval(poll);
        resolve(why);
      };
      const poll = setInterval(async () => {
        if (checking) return;
        checking = true;
        try {
          const snap = await this.screen.snapshot();
          const text = snap.lines.join('\n');
          if (text !== lastText) {
            lastText = text;
            stableSince = Date.now();
          }
          if (regex) {
            const target = baseline
              ? snap.lines.filter((l, i) => l !== (baseline[i] ?? null)).join('\n')
              : text;
            if (regex.test(target)) return finish('pattern');
          }
          if (goneRe && !goneRe.test(text)) return finish('gone');
          const stableFor = Date.now() - stableSince;
          if (req.stable != null && stableFor >= req.stable) return finish('stable');
          if (req.prompt && stableFor >= (req.quietMs ?? 700)) {
            const line = [...snap.lines].reverse().find((l) => l.trim())?.trim() || '';
            if (line && isPromptish(line)) return finish('prompt');
          }
          if (idleMs !== null && Date.now() - this.lastData >= idleMs) return finish('idle');
          if (this.exited) return finish('exit');
          if (Date.now() - started >= timeoutMs) return finish('timeout');
        } finally {
          checking = false;
        }
      }, 120);
    });

    const snap = await this.screen.snapshot();
    this.logger?.event('wait', {
      reason,
      waitedMs: Date.now() - started,
      source,
      ...(req.pattern && { pattern: req.pattern }),
      ...(req.gone && { gone: req.gone }),
    });
    return {
      ok: true,
      reason,
      waitedMs: Date.now() - started,
      alive: !this.exited,
      exitCode: this.exitCode,
      ...(reason === 'prompt' ? this.classifyPrompt(snap.lines) : {}),
      ...snap,
    };
  }

  // Classify the last visible line against the policy so clients (GUI/agent)
  // know who may answer: 'forbid' -> secure human input only (passwords),
  // 'confirm' -> human approves (danger words), 'auto' -> safe to automate,
  // 'unmatched' -> no rule. Returns {} if there's no policy or no prompt line.
  classifyPrompt(lines) {
    if (!this.policy) return {};
    const line = [...lines].reverse().find((l) => l.trim())?.trim() || '';
    if (!line) return {};
    const match = evaluate(this.policy, line, lines.join('\n'));
    return {
      promptLine: line,
      promptClass: match ? match.class : 'unmatched',
      promptRule: match ? match.rule.name : null,
      promptAction: match ? match.rule.action : null,
      promptText: match && match.class === 'auto' ? match.rule.text ?? null : null,
    };
  }
}
