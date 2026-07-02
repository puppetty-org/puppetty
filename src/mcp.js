import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { z } from 'zod';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { request, listSessions, freeName } from './client.js';

const BIN = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'bin', 'puppetty.js');

function screenText(res) {
  return Array.isArray(res.lines) ? res.lines.join('\n') : '';
}

function statusLine(res) {
  const parts = [];
  if ('alive' in res) parts.push(res.alive ? 'running' : `exited(${res.exitCode})`);
  if (res.reason) parts.push(`wait ended: ${res.reason}`);
  if (res.promptClass) parts.push(`prompt class: ${res.promptClass}${res.promptRule ? ` (${res.promptRule})` : ''}`);
  return parts.join(' · ');
}

// Standard tool result: the rendered screen as text, prefixed with a status.
function screenResult(res) {
  const status = statusLine(res);
  const text = [status && `[${status}]`, screenText(res)].filter(Boolean).join('\n');
  return { content: [{ type: 'text', text: text || '(no output)' }] };
}

function errorResult(message) {
  return { content: [{ type: 'text', text: `error: ${message}` }], isError: true };
}

async function req(name, msg, timeoutMs) {
  const res = await request(name, msg, timeoutMs);
  if (!res.ok) throw new Error(res.error || 'request failed');
  return res;
}

function spawnDetached(args) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [BIN, 'run', '-d', ...args], {
      detached: true,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });
    let out = '';
    let err = '';
    child.stdout.on('data', (d) => (out += d));
    child.stderr.on('data', (d) => (err += d));
    child.on('error', reject);
    child.on('close', (code) => {
      if (code === 0) resolve(out.trim());
      else reject(new Error(err.trim() || `puppetty run exited ${code}`));
    });
    child.unref();
  });
}

export async function runMcpServer() {
  const server = new McpServer({ name: 'puppetty', version: '0.1.0' });

  server.registerTool(
    'puppetty_start_session',
    {
      title: 'Start a terminal session',
      description:
        'Launch a command inside a controllable pseudo-terminal (ConPTY) session that survives across tool calls. ' +
        'Use this for any interactive program that may prompt for input (installers, scaffolders, REPLs, TUIs, another agent). ' +
        'Returns the session name and its initial screen. Drive it with puppetty_send / puppetty_read / puppetty_wait.',
      inputSchema: {
        command: z.array(z.string()).min(1).describe('Command and args, e.g. ["npm","create","vite@latest","my-app"]'),
        name: z.string().optional().describe('Optional session name; auto-derived from the command if omitted'),
        cwd: z.string().optional().describe('Working directory for the session'),
        cwdOf: z.string().optional().describe('Use another session\'s working directory (companion sessions)'),
        auto: z.boolean().optional().describe('Auto-answer safe prompts per the policy config (off by default)'),
      },
    },
    async ({ command, name, cwd, cwdOf, auto }) => {
      try {
        const args = [];
        if (name) args.push('--name', name);
        if (cwd) args.push('--cwd', cwd);
        if (cwdOf) args.push('--cwd-of', cwdOf);
        if (auto) args.push('--auto');
        args.push('--', ...command);
        const started = await spawnDetached(args);
        // Give it a moment, then read the initial screen.
        await new Promise((r) => setTimeout(r, 400));
        const res = await req(started, { op: 'read', source: 'mcp' }, 5_000).catch(() => null);
        return {
          content: [
            {
              type: 'text',
              text: `session "${started}" started.\n${res ? screenText(res) : '(starting…)'}`,
            },
          ],
        };
      } catch (e) {
        return errorResult(e.message);
      }
    }
  );

  server.registerTool(
    'puppetty_send',
    {
      title: 'Send text to a session',
      description:
        'Type text into a session, followed by Enter (unless enter=false). Use this to answer a prompt or issue a command. ' +
        'Do NOT use this for secrets: password/passphrase prompts are classified "forbid" and must be handled by a human.',
      inputSchema: {
        name: z.string().describe('Session name'),
        text: z.string().describe('Text to type'),
        enter: z.boolean().optional().describe('Append Enter after the text (default true)'),
      },
    },
    async ({ name, text, enter }) => {
      try {
        await req(name, { op: 'send', data: text, enter: enter !== false, source: 'mcp' }, 5_000);
        return { content: [{ type: 'text', text: `sent to "${name}"` }] };
      } catch (e) {
        return errorResult(e.message);
      }
    }
  );

  server.registerTool(
    'puppetty_keys',
    {
      title: 'Send named keys to a session',
      description:
        'Send special keys to navigate TUIs. Keys: enter, tab, esc, space, backspace, up, down, left, right, home, end, ' +
        'pageup, pagedown, ctrl-c, ctrl-d, ctrl-z.',
      inputSchema: {
        name: z.string().describe('Session name'),
        keys: z.array(z.string()).min(1).describe('Keys in order, e.g. ["down","down","enter"]'),
      },
    },
    async ({ name, keys }) => {
      try {
        await req(name, { op: 'keys', keys, source: 'mcp' }, 5_000);
        return { content: [{ type: 'text', text: `sent keys to "${name}": ${keys.join(' ')}` }] };
      } catch (e) {
        return errorResult(e.message);
      }
    }
  );

  server.registerTool(
    'puppetty_read',
    {
      title: 'Read a session screen',
      description: 'Return the current rendered screen of a session (what a human would see). Use to inspect state.',
      inputSchema: {
        name: z.string().describe('Session name'),
        scrollback: z.boolean().optional().describe('Include scrollback history, not just the visible screen'),
      },
    },
    async ({ name, scrollback }) => {
      try {
        const res = await req(name, { op: 'read', scrollback: !!scrollback, source: 'mcp' }, 5_000);
        return screenResult(res);
      } catch (e) {
        return errorResult(e.message);
      }
    }
  );

  server.registerTool(
    'puppetty_wait',
    {
      title: 'Wait for a session condition',
      description:
        'Block until a condition is met, then return the screen. Combine conditions; the first met wins (child exit and ' +
        'timeout always apply). Recommended: after puppetty_send, use waitFor a known output, or gone="esc to interrupt" ' +
        'for agent TUIs, or prompt=true to detect that the session is blocked waiting for input. Avoids fixed sleeps.',
      inputSchema: {
        name: z.string().describe('Session name'),
        waitFor: z.string().optional().describe('Resolve when the screen matches this regex'),
        sinceStart: z.boolean().optional().describe('With waitFor: only match lines that changed after the wait began'),
        gone: z.string().optional().describe('Resolve when this regex is ABSENT from the screen'),
        stable: z.number().optional().describe('Resolve when the rendered screen is unchanged for N ms'),
        prompt: z.boolean().optional().describe('Resolve when the session settles on a prompt-looking line'),
        idleMs: z.number().optional().describe('Resolve after N ms of no output'),
        flags: z.string().optional().describe('Regex flags for waitFor/gone, e.g. "i"'),
        timeoutSec: z.number().optional().describe('Give up after N seconds (default 60)'),
      },
    },
    async ({ name, waitFor, sinceStart, gone, stable, prompt, idleMs, flags, timeoutSec }) => {
      try {
        const timeoutMs = (timeoutSec ?? 60) * 1_000;
        const msg = { op: 'wait', source: 'mcp', timeoutMs };
        if (waitFor) msg.pattern = waitFor;
        if (sinceStart) msg.sinceStart = true;
        if (gone) msg.gone = gone;
        if (stable != null) msg.stable = stable;
        if (prompt) msg.prompt = true;
        if (idleMs != null) msg.idleMs = idleMs;
        if (flags) msg.flags = flags;
        const res = await req(name, msg, timeoutMs + 5_000);
        return screenResult(res);
      } catch (e) {
        return errorResult(e.message);
      }
    }
  );

  server.registerTool(
    'puppetty_list',
    {
      title: 'List sessions',
      description: 'List live puppetty sessions with their command, pid, and working directory.',
      inputSchema: {},
    },
    async () => {
      try {
        const sessions = await listSessions();
        if (sessions.length === 0) return { content: [{ type: 'text', text: '(no live sessions)' }] };
        const text = sessions
          .map((s) => `${s.name}\t${s.alive ? 'running' : `exited(${s.exitCode})`}\t${s.command}\t${s.cwd ?? ''}`)
          .join('\n');
        return { content: [{ type: 'text', text }] };
      } catch (e) {
        return errorResult(e.message);
      }
    }
  );

  server.registerTool(
    'puppetty_kill',
    {
      title: 'Kill a session',
      description: 'Terminate a session (Ctrl+C then hard kill). Use when a session is stuck or no longer needed.',
      inputSchema: { name: z.string().describe('Session name') },
    },
    async ({ name }) => {
      try {
        await req(name, { op: 'kill', source: 'mcp' }, 5_000);
        return { content: [{ type: 'text', text: `killed "${name}"` }] };
      } catch (e) {
        return errorResult(e.message);
      }
    }
  );

  const transport = new StdioServerTransport();
  await server.connect(transport);
  // stderr is safe for logs; stdout is the MCP channel.
  process.stderr.write('[puppetty] MCP server ready on stdio\n');
}
