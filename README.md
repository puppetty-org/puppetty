# puppetty

Controllable virtual terminal sessions for AI agents.

![puppetty auto-answering interactive prompts](https://raw.githubusercontent.com/puppetty-org/puppetty/main/docs/demo.gif)

## Installation

```powershell
npm install -g puppetty
```

Requires **Node.js 22+** (current LTS; needed for stable `require(esm)`).
That's all the setup the CLI needs — `puppetty` is now on your PATH. Optional
pieces you can add later:

- **Policy** — auto-answer rules in `~/.puppetty/config.json` (user) or
  `<cwd>/.puppetty/config.json` (project); see
  [Autopilot & policy](#autopilot--policy-optional).
- **Credentials** — store secrets in the OS keyring with
  `puppetty cred set <name>`; see [Credentials](#credentials).
- **MCP server** — register `puppetty mcp` in your agent's config; see
  [MCP server](#mcp-server-for-ai-agents).

### GUI

The desktop GUI shows terminal tabs attached to live sessions, a decision
feed of who typed what and which rules fired, and an ask-human dialog for
secret/danger prompts. Two ways to get it:

#### Install from the Releases page

1. Install the engine (if you haven't): `npm install -g puppetty`
2. Download and run the Windows installer (`puppetty-gui_..._x64-setup.exe`)
   from the [Releases page](https://github.com/puppetty-org/puppetty/releases)
3. Launch **puppetty-gui** from the Start menu — it finds the globally
   installed engine automatically

#### Run from source

The GUI is a Tauri 2 app in [`gui/`](gui/); this path needs the
[Rust / Tauri 2 toolchain](https://v2.tauri.app/start/prerequisites/)
in addition to Node:

```powershell
git clone https://github.com/puppetty-org/puppetty
cd puppetty
npm install          # engine dependencies (the GUI drives ../bin/puppetty.js)
cd gui
npm install
npm run vendor       # copy xterm.js into ui/vendor
npm run dev          # launch the desktop app
```

See [`gui/README.md`](gui/README.md) for the GUI's features and details.

`puppetty claude` runs `claude` (or any command) inside a pseudo-terminal
(ConPTY) session that **any other process can drive programmatically**: type
input into it, press keys, and read the rendered screen — like tmux's
`send-keys` / `capture-pane`, but with a JSON API and Windows-first support.

```
┌──────────────┐  named pipe   ┌──────────────────────────────┐  ConPTY  ┌─────────┐
│ puppetty send│──────────────►│ session host                 │◄────────►│ claude, │
│ puppetty read│◄──────────────│  · PTY                       │          │ npm,    │
│ puppetty keys│  JSON lines   │  · headless xterm.js screen  │          │ python… │
│ (or any app) │               │  · optional prompt autopilot │          └─────────┘
└──────────────┘               └──────────────────────────────┘
```

The child process believes it is attached to a real interactive terminal, so
TUIs (Claude Code, inquirer menus), `[y/N]` confirmations, and password
prompts all behave exactly as they would for a human.

## Usage

```powershell
# Start a session (attached: you see it live, others can control it)
puppetty claude

# Or detached (background), like tmux new -d
puppetty run -d --name c1 -- claude

# From any other terminal / script / agent:
puppetty send c1 "What is 2+2? Reply with just the number."
puppetty read c1                  # prints the rendered screen
puppetty keys c1 down down enter  # navigate TUI menus
puppetty list
puppetty kill c1
```

Companion sessions share another session's working directory (side commands
like `git status` next to a running agent):

```powershell
puppetty run -d --name shell --cwd-of claude -- pwsh
```

No sleep-and-poll needed — `wait` blocks until the first condition is met
(child exit and `--timeout` always apply; timeout exits 1):

```powershell
puppetty wait c1 --for "❯" --timeout 30         # screen matches a regex
puppetty wait c1 --for "Done" --since-start     # ...ignoring text already on
                                                #    screen when the wait began
puppetty wait c1 --gone "esc to interrupt"      # pattern DISAPPEARED — the
                                                #    done-detector for agent TUIs
puppetty wait c1 --stable 2000                  # rendered screen unchanged 2s
                                                #    (animation-proof idle)
puppetty wait c1 --prompt                       # settled on a prompt-looking
                                                #    line: it needs input
puppetty wait c1 --idle 3000                    # no output bytes for 3s
```

`wait` always prints the resulting screen; the end reason (`pattern`, `gone`,
`stable`, `prompt`, `idle`, `exit`, `timeout`) goes to stderr and `--json`.

**Controller mode** (recommended for agent-driven sessions): don't configure
any auto-answering — just `wait --prompt`, read the screen, decide, `send`.
The driving agent has full context; the engine only detects the block.

`read --json` returns `{ lines, cursor, alive, exitCode }`;
`read --scrollback` includes history, not just the visible screen.
`send` appends Enter by default (`--no-enter` to type without submitting).
Keys: enter, tab, esc, space, backspace, up, down, right, left, home, end,
pageup, pagedown, ctrl-c, ctrl-d, ctrl-z.

### Control protocol

Each session listens on a named pipe (`\\.\pipe\puppetty-<name>`; a Unix
socket elsewhere). One JSON object per line in, one out — usable directly
from any language without the CLI:

```
{"op":"send","data":"hello","enter":true}
{"op":"keys","keys":["down","enter"]}
{"op":"read","scrollback":false}   → {"ok":true,"alive":true,"lines":[...],"cursor":{...}}
{"op":"wait","pattern":"● ","flags":"i","idleMs":3000,"timeoutMs":60000}
                                   → {"ok":true,"reason":"pattern","waitedMs":4137,"lines":[...]}
{"op":"resize","cols":140,"rows":40}
{"op":"info"} / {"op":"kill"}
```

`wait` fields are all optional: `pattern` (+ `flags`) resolves on a screen
match, `idleMs` on output silence (default 2000 when no pattern is given),
`timeoutMs` caps the wait (default 60000). Child exit always resolves.

Live sessions are registered in `~/.puppetty/sessions/*.json`.

## Autopilot & policy (optional)

With `--auto`, puppetty answers prompts according to a layered policy;
`--decider "<cmd>"` refers unrecognized prompts to an external command:

```powershell
puppetty --auto -- npm create vite@latest my-app
puppetty --decider "claude -p" -- python setup.py
```

Policy is JSONC, layered: built-in defaults ← `~/.puppetty/config.json`
(user) ← `<cwd>/.puppetty/config.json` (project). First matching rule wins;
same-name rules in an earlier layer shadow later ones (`"disabled": true`
tombstones a default).

```jsonc
// .puppetty/config.json
{
  "rules": [
    // action: send | enter | forbid | decider   class: auto | confirm | forbid
    { "name": "project-name", "match": "Project name:\\s*$", "action": "send", "text": "my-app" },
  ],
  "dangerWords": ["\\boverwrite\\b", "git push"],   // escalate matches to confirm
  "onUnanswered": { "afterSec": 30, "do": "cancel" },
  "logging": { "enabled": true, "retentionDays": 30, "maxTotalMB": 200 },
}
```

Severity classes decide who may answer:

- **auto** — rules/decider answer freely (`[y/N]`, "Press Enter", …).
- **confirm** — needs a human; any prompt containing a danger word
  (delete/overwrite/force/`git push`/…) is escalated here even if an auto
  rule matches. Headless: falls through to `onUnanswered` (Ctrl+C → kill).
- **forbid** — never automated. Password/passphrase/token prompts live here;
  they are answered by a human or not at all.
- **credential** — a rule with `"action": "credential", "ref": "name"` is
  answered from the OS keyring: the secret is fetched at the last moment and
  written straight to the PTY. Only the *ref* is ever logged.

## Credentials

Secrets live in the OS keyring (Windows Credential Manager / macOS Keychain /
libsecret), never in a file or log. Manage them from the CLI or the GUI
Settings panel:

```powershell
puppetty cred set github-token     # prompts for the value (hidden input)
puppetty cred list                 # names only
puppetty cred rm github-token
```

Reference one from a policy rule so a known prompt is answered automatically
without any human or agent seeing the secret:

```jsonc
{ "name": "sudo", "match": "password for", "action": "credential", "ref": "sudo-pw" }
```

## Config commands

```powershell
puppetty config show       # print the effective merged policy (JSON)
puppetty config validate   # validate a policy JSON read from stdin
```

The decider receives the rendered screen tail on stdin and replies with one
line: `SEND:<text>` / `ENTER` / `CANCEL` / `WAIT`. `claude -p` works as-is.
A loop guard aborts after answering the same prompt 3 times; exit code 130
means "gave up on a prompt" as opposed to "completed".

## MCP server (for AI agents)

`puppetty mcp` runs an MCP server over stdio, exposing sessions as tools so an
agent (Claude Code, etc.) drives them natively — no CLI shelling, no output
parsing. Tools: `puppetty_start_session`, `puppetty_send`, `puppetty_keys`,
`puppetty_read`, `puppetty_wait`, `puppetty_list`, `puppetty_kill`. Each
returns the rendered screen as text.

Register it in Claude Code (`.mcp.json` or user config):

```jsonc
{
  "mcpServers": {
    "puppetty": { "command": "npx", "args": ["-y", "puppetty", "mcp"] }
    // during local dev: { "command": "node", "args": ["<repo>/bin/puppetty.js", "mcp"] }
  }
}
```

The recommended agent loop: `puppetty_start_session` → `puppetty_wait`
(`prompt: true`, or `gone: "esc to interrupt"` for agent TUIs) → inspect the
returned screen → `puppetty_send` the answer. `puppetty_wait` returns the
prompt classification, so an agent can see when a prompt is `forbid` (a
password it must leave to a human) rather than answering it.

## Session logs

Every session writes two files to `~/.puppetty/logs/` (disable: `--no-log`):

- `<name>-<ts>.cast` — asciinema v2 recording of the output stream; replay
  with `asciinema play`. Input is deliberately not recorded (non-echoed
  secrets must never reach a log).
- `<name>-<ts>.jsonl` — structured control events with source attribution:
  `start`, `send`/`keys` (with text, from `cli`/`pipe`), `stdin` (byte counts
  only — a human may be typing a secret), `answer`/`cancel` (from
  `autopilot`), `prompt-detected`, `wait`, `kill`, `exit`.

Retention is pruned oldest-first (default 30 days / 200 MB total).

## Demos

```powershell
npm run demo            # rule-based auto-answering (y/N, yes/no, press Enter)
npm run demo:decider    # free-text prompt answered by a mock decider
npm run demo:password   # password prompt: never typed, cancelled after 3s
```

## Status / roadmap

Working proof of concept (tested driving a live Claude Code TUI on Windows).
Before production:

- `attach` command (reconnect a real terminal to a detached session, tmux-style)
- Structured event log so a supervising agent can audit every keystroke
- MCP server mode: agents get `session_start` / `send` / `read` / `wait` as tools
- User-defined rules file (`.puppettyrc`), credential-store references for
  password prompts
- macOS/Linux CI (code paths exist — named pipe vs unix socket — but only
  Windows is tested)
