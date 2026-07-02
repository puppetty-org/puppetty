#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import fs from 'node:fs';
import path from 'node:path';
import { Session, KEYMAP, metaPath } from '../src/session.js';
import { request, listSessions, freeName } from '../src/client.js';
import { attachAutopilot } from '../src/autopilot.js';
import { loadPolicy } from '../src/policy.js';
import { EventLog } from '../src/eventlog.js';

const HELP = `puppetty — controllable virtual terminal sessions for AI agents

Usage:
  puppetty [run] [options] [--] <command> [args...]   start a session
  puppetty send <name> <text> [--no-enter]            type text (+ Enter) into a session
  puppetty keys <name> <key> [key...]                 send named keys (${Object.keys(KEYMAP).join(', ')})
  puppetty read <name> [--json] [--scrollback]        print the session's rendered screen
  puppetty wait <name> [conditions] [--json]          block until a condition is met
  puppetty attach <name>                              attach this terminal (detach: Ctrl+])
  puppetty list                                       list live sessions
  puppetty kill <name>                                terminate a session
  puppetty mcp                                        run as an MCP server (stdio) for AI agents
  puppetty cred set <ref>                             store a secret (prompted, hidden) in the OS keyring
  puppetty cred list                                  list stored credential refs (names only)
  puppetty cred rm <ref>                              remove a stored credential
  puppetty config show                                print the effective merged policy (JSON)
  puppetty config validate                            validate a policy JSON read from stdin

Wait conditions (first met wins; child exit and --timeout always apply):
  --for <regex>          screen matches (add --since-start to ignore text that
                         was already on screen when the wait began)
  --gone <regex>         pattern is absent (e.g. --gone "esc to interrupt")
  --stable <ms>          rendered screen unchanged for N ms (animation-proof)
  --prompt               screen settled on a prompt-looking line (controller mode)
  --idle <ms>            no output bytes for N ms (default 2000 if no other condition)
  --timeout <sec>        give up (exit code 1; default 60)
  --flags <f>            regex flags for --for/--gone

Run options:
  --name <name>          session name (default: command basename)
  -d, --detach           run the session in the background and return
  --cwd <dir>            working directory for the session
  --cwd-of <session>     use another session's cwd (companion sessions)
  --cols <n> --rows <n>  terminal size (default 120x30, or your TTY size)
  --auto                 answer prompts per policy (~/.puppetty/config.json)
  --decider "<cmd>"      consult this command for unrecognized prompts (implies --auto)
  --quiet-ms <n>         silence before prompt detection (default 700)
  --prompt-timeout <n>   seconds before an unanswered prompt escalates
  --no-log               disable the session event log (.cast/.jsonl)

Examples:
  puppetty claude
  puppetty run -d --cwd-of claude -- pwsh     companion shell in claude's folder
  puppetty send claude "fix the bug in src/app.ts"
  puppetty wait claude --gone "esc to interrupt" --timeout 600
  puppetty wait claude --prompt               wait until it needs input
`;

function fail(msg) {
  process.stderr.write(msg + '\n');
  process.exit(2);
}

const argv = process.argv.slice(2);
if (argv.length === 0 || argv[0] === '-h' || argv[0] === '--help') {
  process.stdout.write(HELP);
  process.exit(argv.length ? 0 : 2);
}

const SUBCOMMANDS = new Set(['run', 'send', 'keys', 'read', 'wait', 'attach', 'list', 'kill', 'mcp', 'cred', 'config', '__host']);
const sub = SUBCOMMANDS.has(argv[0]) ? argv.shift() : 'run';

try {
  if (sub === 'run' || sub === '__host') await cmdRun(argv, sub === '__host');
  else if (sub === 'send') await cmdSend(argv);
  else if (sub === 'keys') await cmdKeys(argv);
  else if (sub === 'read') await cmdRead(argv);
  else if (sub === 'wait') await cmdWait(argv);
  else if (sub === 'attach') await cmdAttach(argv);
  else if (sub === 'list') await cmdList();
  else if (sub === 'kill') await cmdKill(argv);
  else if (sub === 'mcp') { const { runMcpServer } = await import('../src/mcp.js'); await runMcpServer(); }
  else if (sub === 'cred') await cmdCred(argv);
  else if (sub === 'config') await cmdConfig(argv);
} catch (err) {
  fail(`puppetty: ${err.message}`);
}

// ---------------------------------------------------------------- run

function parseRunArgs(argv) {
  const opts = {
    name: null, detach: false, cols: null, rows: null, cwd: null, cwdOf: null,
    auto: false, decider: null, quietMs: 700, promptTimeout: null, log: true,
  };
  let i = 0;
  for (; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--') { i++; break; }
    else if (a === '--name') opts.name = argv[++i];
    else if (a === '-d' || a === '--detach') opts.detach = true;
    else if (a === '--cwd') opts.cwd = argv[++i];
    else if (a === '--cwd-of') opts.cwdOf = argv[++i];
    else if (a === '--cols') opts.cols = Number(argv[++i]);
    else if (a === '--rows') opts.rows = Number(argv[++i]);
    else if (a === '--auto') opts.auto = true;
    else if (a === '--decider') { opts.decider = argv[++i]; opts.auto = true; }
    else if (a === '--quiet-ms') opts.quietMs = Number(argv[++i]);
    else if (a === '--prompt-timeout') opts.promptTimeout = Number(argv[++i]);
    else if (a === '--no-log') opts.log = false;
    else if (a.startsWith('-')) fail(`Unknown option: ${a}\n${HELP}`);
    else break;
  }
  return { opts, command: argv.slice(i) };
}

async function cmdRun(argv, isHost) {
  const { opts, command } = parseRunArgs(argv);
  if (command.length === 0) fail(HELP);

  let cwd = opts.cwd ? path.resolve(opts.cwd) : null;
  if (opts.cwdOf) {
    const info = await request(opts.cwdOf, { op: 'info' });
    if (!info.ok || !info.cwd) fail(`puppetty: cannot resolve cwd of session "${opts.cwdOf}"`);
    cwd = info.cwd;
  }
  cwd = cwd ?? process.cwd();
  if (!fs.existsSync(cwd)) fail(`puppetty: cwd does not exist: ${cwd}`);

  const name = isHost
    ? opts.name // host trusts the name the parent reserved
    : opts.name
      ? await assertFree(opts.name)
      : await freeName(command[0].replace(/\.[^.]+$/, '').split(/[\\/]/).pop());

  if (opts.detach && !isHost) {
    const hostArgs = [
      fileURLToPath(import.meta.url), '__host', '--name', name, '--cwd', cwd,
      '--cols', String(opts.cols ?? 120), '--rows', String(opts.rows ?? 30),
    ];
    if (opts.auto) hostArgs.push('--auto');
    if (opts.decider) hostArgs.push('--decider', opts.decider);
    if (!opts.log) hostArgs.push('--no-log');
    hostArgs.push('--quiet-ms', String(opts.quietMs));
    if (opts.promptTimeout != null) hostArgs.push('--prompt-timeout', String(opts.promptTimeout));
    hostArgs.push('--', ...command);
    spawn(process.execPath, hostArgs, { detached: true, stdio: 'ignore', windowsHide: true }).unref();
    await waitForSession(name);
    process.stdout.write(name + '\n');
    process.stderr.write(`[puppetty] detached session "${name}" started — read: puppetty read ${name}\n`);
    return;
  }

  const policy = loadPolicy(cwd);
  const attached = !isHost;
  const cols = opts.cols ?? (attached && process.stdout.columns) ?? 120;
  const rows = opts.rows ?? (attached && process.stdout.rows) ?? 30;

  const logger = opts.log && policy.logging.enabled
    ? new EventLog({
        name,
        command: [command[0], ...command.slice(1)].join(' '),
        cols,
        rows,
        logging: policy.logging,
      })
    : null;

  const session = new Session({
    name,
    command: command[0],
    args: command.slice(1),
    cols,
    rows,
    cwd,
    logger,
    policy,
    onData: (d) => {
      if (attached) process.stdout.write(d);
      pilot?.notifyData();
    },
    onExit: (code) => {
      pilot?.stop();
      // "gave up on a prompt" must be distinguishable from "completed".
      if (pilot?.cancelled && !code) code = 130;
      if (attached) {
        restoreStdin();
        setTimeout(() => { session.dispose(); process.exit(code); }, 50);
      } else {
        setTimeout(() => { session.dispose(); process.exit(code); }, 3_000);
      }
    },
  });
  session.start();

  let pilot = null;
  const pilotLog = (m) => { if (attached) process.stderr.write(`\x1b[2m[puppetty] ${m}\x1b[0m\n`); };
  const attachPilot = (promptTimeout = opts.promptTimeout) => attachAutopilot(session, {
    policy,
    quietMs: opts.quietMs,
    promptTimeout,
    decider: opts.decider,
    log: pilotLog,
  });
  if (opts.auto || opts.decider) pilot = attachPilot();

  // Runtime toggle (GUI): attach/detach the autopilot on demand. A GUI attaches
  // a human, so a runtime-enabled pilot never auto-cancels — the human answers
  // anything it won't (secrets/danger) via the ask dialog. Returns the state.
  session.onSetAuto = (enabled) => {
    if (enabled && !pilot) pilot = attachPilot(31_536_000);
    else if (!enabled && pilot) { pilot.stop(); pilot = null; }
    return !!pilot;
  };

  const stdinIsTty = attached && process.stdin.isTTY;
  if (stdinIsTty) {
    process.stdin.setRawMode(true);
    process.stdin.resume();
    process.stdin.on('data', (d) => {
      // Content is never logged: a human may be typing a secret.
      logger?.event('stdin', { bytes: d.length, source: 'human-cli' });
      session.write(d.toString());
    });
  }
  function restoreStdin() {
    if (stdinIsTty) {
      try { process.stdin.setRawMode(false); } catch {}
      process.stdin.pause();
    }
  }

  if (attached && process.stdout.isTTY) {
    process.stdout.on('resize', () => session.resize(process.stdout.columns, process.stdout.rows));
    process.stderr.write(
      `\x1b[2m[puppetty] session "${name}" — control it from another terminal: puppetty send ${name} "..."\x1b[0m\n`
    );
  }
}

async function assertFree(name) {
  const live = await listSessions();
  if (live.some((s) => s.name === name)) throw new Error(`session "${name}" already exists`);
  return name;
}

async function waitForSession(name, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (fs.existsSync(metaPath(name))) {
      try {
        await request(name, { op: 'info' }, 1_000);
        return;
      } catch {}
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`detached session "${name}" failed to start`);
}

// ---------------------------------------------------------------- client commands

async function cmdSend(argv) {
  const [name, ...rest] = argv;
  const enter = !rest.includes('--no-enter');
  const text = rest.filter((a) => a !== '--no-enter').join(' ');
  if (!name || !text) fail('usage: puppetty send <name> <text> [--no-enter]');
  const res = await request(name, { op: 'send', data: text, enter, source: 'cli' });
  if (!res.ok) fail(`puppetty: ${res.error}`);
}

async function cmdKeys(argv) {
  const [name, ...keys] = argv;
  if (!name || keys.length === 0) fail('usage: puppetty keys <name> <key> [key...]');
  const res = await request(name, { op: 'keys', keys, source: 'cli' });
  if (!res.ok) fail(`puppetty: ${res.error}`);
}

async function cmdRead(argv) {
  const [name, ...rest] = argv;
  if (!name) fail('usage: puppetty read <name> [--json] [--scrollback]');
  const res = await request(name, { op: 'read', scrollback: rest.includes('--scrollback') });
  if (!res.ok) fail(`puppetty: ${res.error}`);
  if (rest.includes('--json')) {
    process.stdout.write(JSON.stringify(res, null, 2) + '\n');
  } else {
    process.stdout.write(res.lines.join('\n') + '\n');
    if (!res.alive) process.stderr.write(`[puppetty] process exited (code ${res.exitCode})\n`);
  }
}

async function cmdWait(argv) {
  const name = argv.shift();
  if (!name || name.startsWith('-')) {
    fail('usage: puppetty wait <name> [--for <regex>] [--gone <regex>] [--since-start] [--stable <ms>] [--prompt] [--idle <ms>] [--timeout <sec>] [--flags <f>] [--json]');
  }
  const req = { op: 'wait', source: 'cli' };
  let json = false;
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--for') req.pattern = argv[++i];
    else if (a === '--gone') req.gone = argv[++i];
    else if (a === '--since-start') req.sinceStart = true;
    else if (a === '--stable') req.stable = Number(argv[++i]);
    else if (a === '--prompt') req.prompt = true;
    else if (a === '--idle') req.idleMs = Number(argv[++i]);
    else if (a === '--timeout') req.timeoutMs = Number(argv[++i]) * 1_000;
    else if (a === '--flags') req.flags = argv[++i];
    else if (a === '--json') json = true;
    else fail(`Unknown option: ${a}`);
  }
  const res = await request(name, req, (req.timeoutMs ?? 60_000) + 5_000);
  if (!res.ok) fail(`puppetty: ${res.error}`);
  if (json) {
    process.stdout.write(JSON.stringify(res, null, 2) + '\n');
  } else {
    process.stdout.write(res.lines.join('\n') + '\n');
    process.stderr.write(`[puppetty] wait ended: ${res.reason} after ${res.waitedMs}ms\n`);
  }
  if (res.reason === 'timeout') process.exit(1);
}

async function cmdAttach(argv) {
  const [name] = argv;
  if (!name) fail('usage: puppetty attach <name>');
  const net = await import('node:net');
  const { pipePath } = await import('../src/session.js');

  await new Promise((resolve, reject) => {
    const sock = net.connect(pipePath(name));
    let buf = '';
    const stdinIsTty = process.stdin.isTTY;

    const cleanup = (code, msg) => {
      if (stdinIsTty) {
        try { process.stdin.setRawMode(false); } catch {}
        process.stdin.pause();
      }
      sock.destroy();
      if (msg) process.stderr.write(msg + '\n');
      process.exitCode = code;
      resolve();
    };

    sock.on('error', (err) => reject(new Error(`cannot reach session "${name}" (${err.code || err.message})`)));
    sock.on('connect', () => {
      sock.write(JSON.stringify({
        op: 'attach',
        source: 'human-cli-attach',
        cols: process.stdout.isTTY ? process.stdout.columns : undefined,
        rows: process.stdout.isTTY ? process.stdout.rows : undefined,
      }) + '\n');
      if (stdinIsTty) {
        process.stdin.setRawMode(true);
        process.stdin.resume();
        process.stdin.on('data', (d) => {
          if (d.includes('\x1d')) { // Ctrl+] detaches
            sock.write(JSON.stringify({ op: 'detach' }) + '\n');
            cleanup(0, `\n[puppetty] detached from "${name}" (session keeps running)`);
            return;
          }
          sock.write(JSON.stringify({ op: 'input', data: d.toString('binary') }) + '\n');
        });
      }
      if (process.stdout.isTTY) {
        sock.write(JSON.stringify({ op: 'resize', cols: process.stdout.columns, rows: process.stdout.rows }) + '\n');
        process.stdout.on('resize', () => {
          sock.write(JSON.stringify({ op: 'resize', cols: process.stdout.columns, rows: process.stdout.rows }) + '\n');
        });
      }
      process.stderr.write(`\x1b[2m[puppetty] attached to "${name}" — Ctrl+] to detach\x1b[0m\n`);
    });
    sock.on('data', (chunk) => {
      buf += chunk.toString();
      let nl;
      while ((nl = buf.indexOf('\n')) !== -1) {
        const line = buf.slice(0, nl);
        buf = buf.slice(nl + 1);
        if (!line.trim()) continue;
        let msg;
        try { msg = JSON.parse(line); } catch { continue; }
        if (msg.event === 'data') process.stdout.write(msg.data);
        else if (msg.event === 'exit') cleanup(msg.exitCode ?? 0, `\n[puppetty] session "${name}" exited (${msg.exitCode})`);
      }
    });
    sock.on('close', () => cleanup(process.exitCode ?? 0));
  });
}

async function cmdList() {
  const sessions = await listSessions();
  if (sessions.length === 0) {
    process.stdout.write('(no live sessions)\n');
    return;
  }
  for (const s of sessions) {
    process.stdout.write(
      `${s.name}\tpid=${s.pid}\t${s.alive ? 'alive' : `exited(${s.exitCode})`}\t${s.command}\t${s.cwd ?? ''}\n`
    );
  }
}

// Read a secret from the TTY without echoing it.
function readHidden(prompt) {
  return new Promise((resolve, reject) => {
    process.stderr.write(prompt);
    const stdin = process.stdin;
    const wasRaw = stdin.isRaw;
    if (!stdin.isTTY) return reject(new Error('cannot read a secret without a TTY'));
    stdin.setRawMode(true);
    stdin.resume();
    let buf = '';
    const onData = (d) => {
      const s = d.toString('utf8');
      for (const ch of s) {
        if (ch === '\r' || ch === '\n') {
          stdin.setRawMode(wasRaw || false);
          stdin.pause();
          stdin.removeListener('data', onData);
          process.stderr.write('\n');
          return resolve(buf);
        } else if (ch === '\x03') { // Ctrl+C
          stdin.setRawMode(wasRaw || false);
          stdin.pause();
          process.stderr.write('\n');
          return reject(new Error('cancelled'));
        } else if (ch === '\x7f' || ch === '\b') {
          buf = buf.slice(0, -1);
        } else {
          buf += ch;
        }
      }
    };
    stdin.on('data', onData);
  });
}

async function cmdCred(argv) {
  const { setCredential, listRefs, deleteCredential } = await import('../src/credentials.js');
  const [action, ref] = argv;
  if (action === 'list') {
    const refs = listRefs();
    process.stdout.write(refs.length ? refs.join('\n') + '\n' : '(no stored credentials)\n');
  } else if (action === 'set') {
    if (!ref) fail('usage: puppetty cred set <ref> [--stdin]');
    let secret;
    if (argv.includes('--stdin')) {
      // Read the secret from stdin (used by the GUI); trim one trailing newline.
      secret = await new Promise((resolve) => {
        let buf = '';
        process.stdin.on('data', (d) => (buf += d));
        process.stdin.on('end', () => resolve(buf.replace(/\r?\n$/, '')));
      });
    } else {
      secret = await readHidden(`Secret for "${ref}" (input hidden): `);
    }
    if (!secret) fail('puppetty: empty secret, nothing stored');
    setCredential(ref, secret);
    process.stdout.write(`stored credential "${ref}"\n`);
  } else if (action === 'rm') {
    if (!ref) fail('usage: puppetty cred rm <ref>');
    process.stdout.write(deleteCredential(ref) ? `removed "${ref}"\n` : `"${ref}" not found\n`);
  } else {
    fail('usage: puppetty cred set|list|rm <ref>');
  }
}

async function cmdConfig(argv) {
  const { loadPolicy, userConfigPath, parseJsonc } = await import('../src/policy.js');
  const [action] = argv;
  if (action === 'show') {
    const p = loadPolicy(process.cwd());
    // Include disabled rules (with the flag) so a GUI can offer enable/disable;
    // the autopilot itself uses p.compiled, which already excludes disabled.
    const rules = p.rules.map((r) => ({
      name: r.name, match: r.match, flags: r.flags, action: r.action,
      class: r.class, ref: r.ref, text: r.text, scope: r.scope, ai: r.ai,
      describe: r.describe, enter: r.enter, disabled: !!r.disabled,
    }));
    process.stdout.write(JSON.stringify({
      rules,
      dangerWords: p.dangerWords,
      onUnanswered: p.onUnanswered,
      sources: p.sources,
      userConfigPath: userConfigPath(),
    }, null, 2) + '\n');
  } else if (action === 'validate') {
    const text = await new Promise((resolve) => {
      let buf = '';
      process.stdin.on('data', (d) => (buf += d));
      process.stdin.on('end', () => resolve(buf));
    });
    try {
      const obj = parseJsonc(text);
      for (const r of obj.rules ?? []) new RegExp(r.match, r.flags ?? ''); // compile-check
      process.stdout.write('ok\n');
    } catch (err) {
      process.stderr.write(`invalid: ${err.message}\n`);
      process.exit(1);
    }
  } else {
    fail('usage: puppetty config show|validate');
  }
}

async function cmdKill(argv) {
  const [name] = argv;
  if (!name) fail('usage: puppetty kill <name>');
  const res = await request(name, { op: 'kill', source: 'cli' });
  if (!res.ok) fail(`puppetty: ${res.error}`);
  process.stdout.write(`killed ${name}\n`);
}
