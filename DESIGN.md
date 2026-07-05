# puppetty — Design Document

Status: M1–M3 done · MCP mode done · M4 (npm) done · M5 (Rust host) done · Last updated: 2026-07-05

> 2026-07-05: the Rust port (§4.3 / M5) is complete and is now the only
> engine. `engine-rs` (portable-pty + alacritty_terminal as the screen
> model) speaks the same JSON-lines protocol; the Node engine is gone and
> `bin/puppetty.js` is a thin launcher shipped by npm alongside per-platform
> `@puppetty/*` binary packages (M4). Credentials moved to the Rust
> `keyring` crate (Credential Manager / Keychain / Linux kernel keyring).
> Distribution deviates from D7/§4.3: no MSI — the GUI installs from a
> GitHub Pages script endpoint (`install.ps1` / `install.sh`) that verifies
> SHA-256 and bundles the engine sidecar, on Windows and Linux. Linux is
> now CI-tested (engine + GUI); macOS GUI packages are deferred until a
> proper `.app` bundle flow exists (the CLI does ship macOS binaries via
> npm, untested in CI).

> 2026-07-02 (M3): credential store, policy editor, and notifications landed.
> Credentials use the OS keyring via @napi-rs/keyring (prebuilt, no toolchain);
> new `credential` rule class fetches by ref and types the secret straight to
> the PTY — verified end-to-end (sudo-style prompt auto-answered, secret in
> NEITHER log, answer event records ref + redacted:true). CLI `cred
> set/list/rm` (hidden TTY input, `--stdin` for the GUI) and `config
> show/validate`. GUI Settings panel (effective rules / JSONC editor with
> engine validation / credential management) and OS toast on the rising edge
> of attention — all verified live via CDP, including a full keyring
> add→list→remove round-trip reflected in the real Credential Manager.

> 2026-07-02 (later): M2 exit criterion met — ask-human secure-input dialog
> verified live against a simulated pinentry: a `forbid` prompt auto-popped a
> masked dialog, the typed passphrase reached the program, and the secret
> appeared in NEITHER the .jsonl nor .cast log (stdin logged as byte count
> only). Prompt classification (`forbid`/`confirm`/`auto`/`unmatched`) is now
> returned by the `wait` op and surfaced to the GUI. MCP server mode
> (`puppetty mcp`, §4.5) also landed and was verified with a real MCP client:
> start_session/wait/send/read/keys/list/kill exposed as tools, agent loop
> works end to end. NOTE: puppetty never logs input, but a child that echoes
> a secret to its own output (unlike real no-echo password prompts) would
> have that echo in the .cast — documented, not a puppetty leak.

> M2 alpha landed 2026-07-02: engine `attach` op (streaming, replay,
> bidirectional, multi-client) + CLI `puppetty attach` (Ctrl+] detach) +
> Tauri 2 GUI (`gui/`) with xterm.js tabs, companion-tab backend, decision
> feed from the .jsonl log, and blocked-on-prompt banner. Verified live via
> CDP: GUI keystrokes reach the real PTY; companion sessions inherit cwd.
> M2 polish (owner review, 2026-07-02): per-tab ✕ kill; attach now sends a
> serialized screen restore at the client's dimensions (raw-history replay
> caused phantom scrollback); attention banner is transition-based (blocked
> + recent autonomous output; excludes local-typing echo, attach restores,
> and resize repaints); buttons renamed to plain language ("＋ New session…",
> ">_ Shell in this folder"); app icon generated from
> gui/scripts/make-icon.ps1. Remaining for M2 exit criterion: ask-human
> secure-input dialog and a real gpg-pinentry walkthrough.

> M1 landed 2026-07-02: hardened `wait` (`--gone`, `--since-start`,
> `--stable`, `--prompt`/controller mode), JSONC policy loader with severity
> classes + danger-word escalation, `--cwd`/`--cwd-of` (companion sessions),
> event log (.cast + .jsonl, attributed, secret-safe). All exit criteria in
> §6 M1 verified by live tests, including the claude done-detector via
> `--gone "esc to interrupt"`.

## 1. Problem

AI coding agents (Claude Code, Codex, …) frequently run scripts that block on
interactive input — `[y/N]` confirmations, free-text questions, password
prompts, "press Enter" pauses. Agents handle this badly: the command hangs
until a timeout or a Ctrl+C, the work stalls, and the human has to sit in
front of the monitor to unblock it.

Goal: a terminal layer that keeps agent-driven development moving without a
human present, while never silently doing something unsafe.

Origin note: the idea started as "a terminal emulator for Windows that
auto-answers prompts." Two reframings happened during design, recorded here
because they shaped everything:

1. **Auto-answering alone is not a terminal emulator problem.** It's a PTY
   interception problem (classic `expect`, modernized). No rendering needed.
2. **Auto-answering alone is also not enough.** The stronger primitive is a
   *controllable session*: any outside process can type into a running
   terminal session and read its rendered screen. Auto-answering then becomes
   one optional policy layered on top.

## 2. What exists today (validated proof of concept)

Rust engine (`engine-rs`) plus a thin npm launcher in this repo (originally
a Node.js PoC — see the 2026-07-05 note). All behaviors below are tested on
Windows 11, including against a live Claude Code TUI; Linux is CI-tested.

- **Sessions**: `puppetty [run] [-d] [--name x] -- <cmd>` hosts any command
  under a PTY (`portable-pty`: ConPTY on Windows, openpty elsewhere).
  The child believes it's on a real terminal; TUIs render correctly.
- **Screen model**: an `alacritty_terminal` grid mirrors the PTY, so `read`
  returns the rendered screen (what a human would see), not
  escape-sequence soup.
- **Control API**: each session serves JSON-lines over a named pipe
  (`\\.\pipe\puppetty-<name>`; Unix socket elsewhere). Ops:
  `send`, `keys`, `read`, `wait`, `resize`, `info`, `kill`, `attach`,
  `set-auto`.
- **`wait` op**: blocks until screen matches a regex / output idles / child
  exits / timeout. Removed all sleep-and-poll from callers. (Measured: TUI
  ready in 751ms, claude's answer detected 4.1s after send — zero sleeps.)
- **Autopilot (opt-in)**: `--auto` answers rule-matched prompts; `--decider
  "<cmd>"` refers unknown prompts to an external command (e.g. `claude -p`)
  via a SEND/ENTER/CANCEL/WAIT protocol.
- **Safety invariants** (already enforced): secrets are never auto-typed
  (deny rule outranks all); same-prompt loop guard (3 strikes → cancel);
  cancel escalation Ctrl+C → kill; exit 130 distinguishes "gave up on a
  prompt" from "completed".
- **Registry**: `~/.puppetty/sessions/*.json`, self-cleaning; sessions
  outlive the launching terminal; 3s post-exit grace so clients can read the
  final screen.

## 3. Decisions made (and alternatives considered)

| # | Decision | Alternatives rejected | Why |
|---|----------|----------------------|-----|
| D1 | Core = PTY interceptor + session API, not a from-scratch terminal emulator | Win32 terminal app; forking Windows Terminal | Rendering/IME/fonts add months and no product value; xterm.js solves rendering when a GUI is wanted |
| D2 | Node.js for the PoC/engine | Rust-first (portable-pty) | `@lydell/node-pty` prebuilds made install trivial; fastest iteration on the hard part (detection heuristics, protocol). Rust port stays open as Phase 2 (D6) |
| D3 | Screen reads via headless xterm.js buffer | Regex ANSI-stripping of the raw stream | ConPTY constantly repaints; raw-stream heuristics proved unreliable in testing (false "prompt" detections). Battle-tested screen model instead |
| D4 | tmux-like model: named sessions + `send-keys`/`capture-pane` equivalents + JSON pipe protocol | Auto-answer-only wrapper; MCP-only interface | Any language/agent can drive it without puppetty's own code; auto-answer becomes optional policy, not the core |
| D5 | Autopilot is opt-in (`--auto`), off by default | Always-on rules | Rules firing into a TUI like claude would corrupt the session; explicit opt-in per run |
| D6 | GUI = Tauri 2 + xterm.js, as a *client* of the same session engine | Electron; native Win32; GUI with its own embedded engine | User has shipped Tauri 2 (Voice2Text: sidecars, MSI/NSIS, updater). Single engine keeps CLI + agents + GUI on one session simultaneously (the tmux property). Two-engine drift is the failure mode to avoid |
| D7 | Distribution: npm global first, standalone MSI later | MSI-first | Target users (agent users) have Node; npm ships today. MSI matters once the Rust port removes the Node dependency |

## 4. Proposed next stage (not yet built)

### 4.1 Policy configuration — the actual product

Declarative policy, enforced by the engine, edited by the GUI. First match
wins; per-command overrides; three answer sources: static rule, decider
agent, human.

```jsonc
// ~/.puppetty/config.json  (draft schema — refine before building)
{
  "profiles": {
    "default": {
      "rules": [
        { "match": "\\[y/N\\]|\\(yes/no\\)",        "action": "send", "text": "y" },
        { "match": "press (enter|any key)",          "action": "enter" },
        { "match": "password|passphrase|token",      "action": "ask-human" },
        { "match": "delete|rm -rf|force|irreversib", "action": "ask-human" },
        { "match": "*",                              "action": "decider", "decider": "claude" }
      ],
      "deciders": { "claude": { "command": "claude -p", "timeoutSec": 60 } },
      "onUnanswered": { "afterSec": 30, "do": "cancel" }
    },
    "overrides": [
      { "command": "git push*", "rules": [{ "match": "*", "action": "ask-human" }] }
    ]
  }
}
```

`ask-human` semantics: GUI attached → secure dialog / click-to-approve toast
(secrets typed by the human go straight to the PTY; decider agents never see
them). Headless → falls back to `onUnanswered`.

### 4.2 Tauri GUI (puppetty-gui)

- xterm.js tabs attached live to sessions; human and agent can both type.
- Decision feed sidebar: every auto-answer with its cause ("`[y/N]` → `y`,
  rule: yes-no-bracket", "`Project name:` → `my-app`, decider: claude") —
  the glance-at-the-monitor audit trail.
- Policy editor UI for the config above.
- Notifications for `ask-human` prompts.
- **Companion tabs** (owner workflow, 2026-07-02): when driving an agent
  session (e.g. claude), one click opens a sibling shell tab in the *same
  working directory* — for `git status`, `echo test | gpg --clear-sign`,
  and similar side commands, without leaving the app. Companion sessions
  are ordinary sessions (agents can drive them too); the GUI just groups
  them under the parent tab. Note the gpg example blocks on pinentry —
  companion tabs get the same policy engine, so the passphrase prompt
  routes to `forbid`/ask-human like any other.
- Vanilla JS UI (Voice2Text style) — no framework needed.

Prerequisite engine work:
- `attach` op: long-lived pipe connection streaming raw PTY bytes (xterm.js
  needs the byte stream; snapshots are for agents).
- Policy loader in the core, shared by CLI autopilot and GUI.
- Structured event log per session (feeds the decision feed; also useful
  headless).
- `--cwd <dir>` on `run` + cwd in `info` responses (today a session's cwd is
  the launcher's cwd) — enables companion tabs and
  `puppetty run --cwd-of <session> -- pwsh` from the CLI.

### 4.3 Phase 2 — Rust port of the session host ✅ (done, shape changed)

Shipped, with two deviations from this plan: the host became the standalone
`engine-rs` binary (not code inside the Tauri backend — the GUI spawns it
as a sidecar, keeping the single-engine property of D6), and the screen
model is `alacritty_terminal` rather than `vt100`/`wezterm-term`. The pipe
protocol stayed compatible so CLI and agents were untouched. Distribution
landed as per-platform npm binary packages plus a Pages script installer
for the GUI — no MSI needed.

### 4.4 Full-rendering TUIs — Claude Code v2 findings (measured 2026-07-02)

Modern agent TUIs (Claude Code v2+) repaint their UI region continuously
instead of appending lines. This stresses two of our primitives. Measured
against a live claude session:

- **At rest, the TUI is quiet**: `wait --idle 3000` fired normally (~0.9s of
  settling). No rest-state animation today.
- **During work, spinner/status repaints keep the byte stream busy**: with
  `--idle 1500` sent mid-task, idle could not fire early — it fired at 6.6s,
  exactly when the answer completed and rendering stopped. So for claude,
  `send` → `wait --idle 1500` currently *is* a correct "done or needs my
  attention" signal.
- **Scrollback adds nothing for claude** (visible == scrollback in test);
  the visible screen is the source of truth. `read --scrollback` remains
  useful only for scrolling CLIs.

Why this is still fragile, and the planned hardening:

1. **Byte-idle breaks the moment a TUI animates at rest** (rendered blinking
   cursor, clock, shimmer). Version-dependent luck, not a contract.
   → Keep `--idle`, but treat it as a heuristic.
2. **Stale-pattern matches**: full-render screens keep old text visible, so
   `wait --for "●"` matches the welcome screen instantly, not the new
   answer. We dodged it in testing by matching exact text (`"● 42"`).
   → Add **diff-scoped matching**: `wait --for X --since-start` snapshots the
   screen when the wait begins and matches only lines that changed/appeared
   after. This is the robust general fix.
3. **Negative patterns are the semantic "done" signal for agent TUIs**:
   claude shows `esc to interrupt` exactly while working.
   → Add `wait --gone <regex>`: resolve when the pattern *disappears* from
   the screen. `send` → `wait --for "esc to interrupt"` → `wait --gone
   "esc to interrupt"` is a version-tolerant done-detector.
4. **Screen-stability wait** as the animation-proof idle: resolve when the
   *rendered screen hash* is unchanged for N ms (`--stable <ms>`), optionally
   ignoring lines matching a noise regex (spinner/timer lines).

Priority: `--gone` and `--since-start` are small additions to the existing
`wait` machinery and cover the known claude cases; `--stable` follows.

### 4.5 Done since original plan

- **MCP server mode** (`puppetty mcp`): sessions as first-class agent tools
  (`puppetty_start_session`, `send`, `read`, `wait`, `keys`, `list`, `kill`).
  Verified with a real MCP client.
- **`attach` CLI command** (reconnect a human terminal, tmux-style; Ctrl+]).

- **Linux support**: engine + GUI build and test in CI; the script
  installer publishes a Linux package.

Still planned:
- macOS: CLI binaries ship via npm but are untested in CI; no GUI packages
  (needs a proper `.app` bundle flow).

## 5. Resolved questions (2026-07-02 review)

Owner feedback: mostly "recommend the best." Decisions below are adopted
unless overturned in a later review.

**Q1 — Config format & layering → JSONC, two layers.**
JSON-with-comments, matching the Claude Code / VS Code convention the target
users already live in (schema-validatable, no YAML footguns). Layering:
`~/.puppetty/config.json` (user) ← `.puppetty/config.json` (project root,
committed) — project overrides user, same model as Claude Code settings.
Per-command overrides live inside profiles as designed.

**Q2 — Danger gating → severity classes, decider-with-veto.**
Every rule resolves to a class, and the class — not the rule — dictates who
may answer:

| Class | Who may answer | Example |
|-------|----------------|---------|
| `auto` | rules / decider freely | `[y/N]` on `npm create` |
| `confirm` | decider *proposes*, human must approve (GUI toast with proposal pre-filled; headless → cancel) | overwrite/delete/force/push prompts |
| `forbid` | never automated: human types or it's cancelled | passwords, passphrases (e.g. gpg pinentry) |

Danger keywords remain only as heuristics that *assign* `confirm`; they are
no longer the mechanism itself. Ships with a conservative default table.

**Q3 — Decider context → two decider modes; controller mode preferred.**
(a) *External decider* (today's `--decider "cmd"`): receives a structured
payload — visible screen, command line, cwd, session name, recent decision
history. Scrollback tail is opt-in (`contextLines`). Secrets never included.
(b) *Controller mode* (new, recommended default for agent-driven sessions):
the engine doesn't answer at all — it marks the session `blocked-on-prompt`,
and the agent already driving the session discovers this via `wait`
(new resolve reason: `prompt`) or the event log, and answers with `send`.
The driving agent (often the same main agent, per owner) has full session
context by construction — no context-shipping problem, no second LLM call.

**Q4 — Passwords → phased: ask-human now, credential store later.**
v1: `forbid` class → GUI secure input / headless cancel. v1.x: OS-backed
store (Windows Credential Manager / DPAPI; keychain elsewhere) enabling
`{ "action": "credential", "ref": "github-token" }`. Invariants: deciders
and logs only ever see the *ref*; human-typed secret input is never logged
(event log records `answered-by-human (redacted)`).

**Q5 — Multi-writer → free-for-all + attribution, no locks.**
tmux behavior (least surprise). Every input event in the log carries its
source (`human-gui`, `human-cli`, `autopilot`, `decider`, `controller`), so
conflicts are auditable. Revisit an advisory lock only if real-world
conflicts hurt; don't pre-build it.

**Q6 — Event log → asciinema `.cast` for replay + `.jsonl` for decisions.**
Two files per session under `~/.puppetty/logs/`:
- `<name>-<ts>.cast` — asciinema v2 format (timestamped output; input is
  deliberately not recorded — see the M2 note on secret-safety).
  Industry-standard replay: existing players, `asciinema play`, embeddable.
- `<name>-<ts>.jsonl` — structured control events (send/keys/wait/prompt/
  answer/kill) with source attribution and redaction, cross-referenced to
  the cast by timestamp. This feeds the GUI decision feed.
Retention: configurable, default 30 days / 200 MB, oldest-first pruning.

**Q7 — Naming → `puppetty` (npm-available, checked 2026-07-02).**
"Puppeteer for terminals" — says exactly what it does; `ttypilot` is the
runner-up (also free). `puppetty` stays as the working name until
pre-release; rename package *and* binary together then (one rename, not
two). Re-verify availability at publish time.

**Q8 — v0.1 release → deferred.**
Owner dogfoods locally first. npm publish moves after policy config,
`attach`, hardened `wait`, and the event log have survived local use
(reordered milestones, §6).

**Q9 — Autopilot vs TUIs → split detection from resolution.**
Detection (is something waiting for input?) stays universal and moves to
screen-stability + diff heuristics (§4.4). Resolution splits by session
kind: line-based CLIs → autopilot rules may answer directly; TUIs → engine
only *surfaces* the prompt (controller mode / ask-human), because a decider
reading a rendered screen and answering via `send`/`keys` is strictly more
capable than regex rules against a repainting UI.

## 6. Milestones (proposed)

| Milestone | Contents | Exit criterion |
|-----------|----------|----------------|
| M1: engine hardening ✅ | hardened `wait` (`--gone`, `--since-start`, `--stable`, `prompt` reason), policy loader (JSONC, layered), severity classes, controller mode, `--cwd`, event log (.cast + .jsonl) | agent + human co-drive one session; policy file replaces hardcoded rules; claude done-detection works via `--gone` |
| M2: GUI alpha ✅ | Tauri shell, `attach` op, xterm tabs, companion tabs, decision feed, ask-human dialog | gpg pinentry answered by human via GUI while an agent session continues in another tab |
| (bonus) MCP mode ✅ | `puppetty mcp` stdio server, 7 session tools | verified with a real MCP client end-to-end |
| M3: policy editor + notifications ✅ | settings UI, toasts, credential store (keyring) | config fully editable in GUI; a stored credential answers a prompt without any agent seeing it |
| M4: npm v0.1 ✅ | rename to `puppetty`, README, publish | `npm i -g` works on a clean machine after owner dogfooding |
| M5: Rust host ✅ | portable-pty port, protocol-compatible | standalone install, no Node runtime — shipped as `engine-rs` + per-platform npm packages + the GUI script installer (MSI not needed) |
