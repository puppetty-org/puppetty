# puppetty

**The terminal as an API.** Any interactive program becomes callable: type
into it, read its rendered screen, wait for it to need input — over a JSON
protocol. AI agents answer the prompts they should, known questions
auto-answer by policy, secrets come from the OS keyring, and nothing ever
hangs waiting for input.

![puppetty auto-answering interactive prompts](https://raw.githubusercontent.com/puppetty-org/puppetty/main/docs/demo.gif)

## Installation

```powershell
npm install -g puppetty
```

The engine is a native Rust binary; npm delivers the one for your platform
plus the thin Node launcher that runs it, so `puppetty` is now on your
PATH. Optional pieces you can add later:

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

#### Install the desktop app

Windows:

```powershell
iwr -useb https://raw.githubusercontent.com/puppetty-org/puppetty/main/gui/scripts/install-gui.ps1 | iex
```

Linux / macOS (Apple Silicon):

```sh
curl -fsSL https://raw.githubusercontent.com/puppetty-org/puppetty/main/gui/scripts/install-gui.sh | sh
```

The script resolves the newest stable release on GitHub Releases, downloads
your platform's package (release assets are immutable once published),
verifies its SHA-256 checksum, installs the GUI with the bundled engine,
and creates the platform shortcut/link. On Windows it also installs the
WebView2 runtime if it is missing. On Linux the app needs the WebKitGTK 4.1
runtime (`libwebkit2gtk-4.1-0` on Debian/Ubuntu); the installer warns when
it is absent. On macOS the app bundle installs into `~/Applications` (Apple
Silicon only — Intel Macs are not supported).

Prereleases are only installed on explicit opt-in: `CHANNEL=beta` before
`sh`, or `$env:PUPPETTY_CHANNEL = "beta"` before the `iwr` line on Windows.

To uninstall the desktop app, use Windows **Settings ▸ Apps ▸ Installed
apps**, select **puppetty-gui**, and choose **Uninstall**. On Linux, run
`~/.local/share/puppetty-gui/uninstall.sh`. On macOS, move
`~/Applications/puppetty-gui.app` to the Trash.

#### Run from source

The GUI is a Tauri 2 app in [`gui/`](gui/); this path needs the
[Rust / Tauri 2 toolchain](https://v2.tauri.app/start/prerequisites/)
in addition to Node:

```powershell
git clone https://github.com/puppetty-org/puppetty
cd puppetty/gui
npm install
npm run vendor       # copy xterm.js into ui/vendor + build the engine sidecar
npm run dev          # launch the desktop app
```

See [`gui/README.md`](gui/README.md) for the GUI's features and details.

`puppetty <command>` runs any program — an installer, a REPL, a dev server,
a full-screen TUI — inside a pseudo-terminal (ConPTY) session that **any
other process can drive programmatically**: type input into it, press keys,
and read the rendered screen — like tmux's `send-keys` / `capture-pane`,
but with a JSON API and Windows-first support.

```
┌──────────────┐  named pipe   ┌──────────────────────────────┐  ConPTY  ┌──────────┐
│ puppetty send│──────────────►│ session host                 │◄────────►│ npm,     │
│ puppetty read│◄──────────────│  · PTY                       │          │ python,  │
│ puppetty keys│  JSON lines   │  · headless terminal screen  │          │ ssh,     │
│ (or any app) │               │  · optional prompt autopilot │          │ any TUI… │
└──────────────┘               └──────────────────────────────┘          └──────────┘
```

The child process believes it is attached to a real interactive terminal, so
full-screen TUIs, inquirer menus, `[y/N]` confirmations, and password
prompts all behave exactly as they would for a human.

## Usage

```powershell
# Start a session (attached: you see it live, others can control it)
puppetty python

# Or detached (background), like tmux new -d
puppetty run -d --name py -- python

# From any other terminal / script / agent:
puppetty send py "6 * 7"
puppetty read py                  # prints the rendered screen -> 42
puppetty keys py down down enter  # navigate TUI menus
puppetty list
puppetty attach py                # reattach your terminal (Ctrl+] to detach)
puppetty kill py
```

Companion sessions share another session's working directory (side commands
like `git status` next to a long-running build or dev server):

```powershell
puppetty run -d --name dev -- npm run dev
puppetty run -d --name shell --cwd-of dev -- pwsh
```

No sleep-and-poll needed — `wait` blocks until the first condition is met
(child exit and `--timeout` always apply; timeout exits 1):

```powershell
puppetty wait dev --for "ready in"              # screen matches a regex
puppetty wait dev --for "Done" --since-start    # ...ignoring text already on
                                                #    screen when the wait began
puppetty wait dev --gone "Compiling"            # pattern DISAPPEARED — the
                                                #    done-detector for busy TUIs
puppetty wait dev --stable 2000                 # rendered screen unchanged 2s
                                                #    (animation-proof idle)
puppetty wait dev --prompt                      # settled on a prompt-looking
                                                #    line: it needs input
puppetty wait dev --idle 3000                   # no output bytes for 3s
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
match, `idleMs` on output silence (default 2000 when no other condition is
given), `timeoutMs` caps the wait (default 60000). Child exit always
resolves. Sessions also accept `attach` (a persistent bidirectional stream —
what the GUI and `puppetty attach` use) and `set-auto`.

Live sessions are registered in `~/.puppetty/sessions/*.json`.

## Autopilot & policy (optional)

With `--auto`, puppetty answers prompts according to a layered policy;
`--decider "<cmd>"` refers unrecognized prompts to an external command:

```powershell
puppetty --auto -- npm create vite@latest my-app
puppetty --decider "<your LLM CLI>" -- python setup.py
```

The classic failure this prevents: an AI is following install instructions,
the installer asks a question, and everything sits there until a timeout.
Under puppetty the prompt is detected, answered by a rule or referred to an
LLM with the screen as context — and the run keeps moving.

### Choosing your LLM CLI (or none)

The decider is any command that reads the screen from stdin and prints one
line back — `claude -p`, `codex exec`, a shell script, whatever you have:

- **Per run**: `--decider "codex exec"`
- **Set once**: a `default` decider in `~/.puppetty/config.json`:

  ```jsonc
  { "deciders": { "default": { "command": "claude -p" } } }
  ```

- **Per rule**: a rule with `"action": "decider", "decider": "<name>"` routes
  just that prompt to a named decider.

**No AI CLI installed?** Everything still works: rules answer the known
prompts, `wait --prompt` (or the MCP tools) tells the driving process a
session needs input so *it* can decide, and `onUnanswered` cancels safely
instead of hanging.

Policy is JSONC, layered: built-in defaults ← `~/.puppetty/config.json`
(user) ← `<cwd>/.puppetty/config.json` (project). First matching rule wins;
same-name rules in an earlier layer shadow later ones (`"disabled": true`
tombstones a default).

```jsonc
// .puppetty/config.json
{
  "rules": [
    // action: send | enter | forbid | decider | credential | ignore
    { "name": "project-name", "match": "Project name:\\s*$", "action": "send", "text": "my-app" },
  ],
  "dangerWords": ["\\boverwrite\\b", "git push"],   // escalate matches to confirm
  "onDanger": "human",   // or "decider": let your LLM judge danger-word
                         // prompts too (it gets an explicit caution preamble)
  "onUnanswered": { "afterSec": 30, "do": "cancel" },
  "logging": { "enabled": true, "retentionDays": 30, "maxTotalMB": 200 },
}
```

Severity classes decide who may answer:

- **auto** — rules/decider answer freely (`[y/N]`, "Press Enter", …).
- **ignore** — not a question at all: idle shell/REPL prompts
  (`PS C:\…>`, `user@host:~$`, `❯`, `>>>`) are skipped silently — no
  events, no feed noise. Built-in rules cover the common shells; add
  your own for exotic prompts.
- **confirm** — needs a human; any prompt containing a danger word
  (delete/overwrite/force/`git push`/…) is escalated here even if an auto
  rule matches. Headless: falls through to `onUnanswered` (Ctrl+C → kill).
  Prefer LLM judgment for these? Set `"onDanger": "decider"` — danger-word
  escalations (never explicit confirm/forbid rules) go to your decider with
  a caution preamble instead; a regex can't weigh ambiguity, an LLM can.
- **forbid** — never automated. Password/passphrase/token prompts live here;
  they are answered by a human or not at all.
- **credential** — a rule with `"action": "credential", "ref": "name"` is
  answered from the OS keyring: the secret is fetched at the last moment and
  written straight to the PTY. Only the *ref* is ever logged.

## Credentials

Secrets live in the OS keyring (Windows Credential Manager / macOS Keychain /
the Secret Service on Linux — GNOME Keyring or KWallet), never in a file or
log. Manage them from the CLI or the GUI
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
line: `SEND:<text>` / `ENTER` / `CANCEL` / `WAIT` — any LLM CLI that reads
stdin and prints a reply works as-is.
A loop guard aborts after answering the same prompt 3 times; exit code 130
means "gave up on a prompt" as opposed to "completed".

## MCP server (for AI agents)

`puppetty mcp` runs an MCP server over stdio, exposing sessions as tools so an
agent (Claude Code, etc.) drives them natively — no CLI shelling, no output
parsing. Tools: `puppetty_start_session`, `puppetty_send`, `puppetty_keys`,
`puppetty_read`, `puppetty_wait`, `puppetty_list`, `puppetty_kill`. The
start/read/wait tools return the rendered screen as text.

Register it in Claude Code (`.mcp.json` or user config):

```jsonc
{
  "mcpServers": {
    "puppetty": { "command": "npx", "args": ["-y", "puppetty", "mcp"] }
    // during local dev: { "command": "<repo>/engine-rs/target/release/puppetty-engine", "args": ["mcp"] }
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

Interactive test children live in `engine-rs/examples/`; build them once
with `cargo build --examples --release` in `engine-rs/`, then:

```powershell
# rule-based auto-answering (y/N, yes/no, press Enter)
puppetty --auto -- engine-rs\target\release\examples\prompt_demo.exe
# free-text prompt answered by a mock decider
puppetty --decider engine-rs\target\release\examples\decider_echo.exe -- engine-rs\target\release\examples\freeform_demo.exe
# password prompt: never typed, cancelled after 3s
puppetty --auto --prompt-timeout 3 -- engine-rs\target\release\examples\password_demo.exe
```

## Security

See [SECURITY.md](SECURITY.md) for vulnerability reporting, the security
model, build integrity, and privacy notes.

## Status / roadmap

Alpha. Everything documented above — sessions, `wait`, autopilot & policy,
credentials, the event log, the MCP server, `attach`, and the GUI — is
implemented and exercised against live full-screen TUIs, and CI builds and
tests Windows and Linux. Still open before a stable release:

- macOS: the CLI installs via npm, but it is not CI-tested and no GUI
  packages are published yet (needs a proper `.app` bundle flow)
- Code signing for the GUI packages (installs are SHA-256-verified by the
  install script, but the binaries themselves are unsigned)
