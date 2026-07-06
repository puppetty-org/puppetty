# puppetty — reference for AI agents

> Condensed operational reference for agents driving puppetty. The
> narrative guide — installation details, GUI, demos, design, roadmap — is
> in [README.md](README.md).

puppetty runs any command inside a background pseudo-terminal session
(ConPTY on Windows, PTY elsewhere) that you control with follow-up
commands: type text, press keys, read the rendered screen, and block until
something happens. Use it for anything interactive or long-running that
would hang a plain shell call — installers, scaffolders, REPLs, full-screen
TUIs, dev servers, SSH, another AI CLI.

Install: `npm install -g puppetty` (or invoke via `npx -y puppetty`).
Windows, Linux, macOS (Apple Silicon).

## The driving loop

```sh
puppetty run -d --name app -- npm create vite@latest my-app
puppetty wait app --prompt          # block until it needs input (or exits)
puppetty read app                   # rendered screen, what a human would see
puppetty send app "my-app"          # type text + Enter
puppetty keys app down down enter   # navigate TUI menus
puppetty kill app
```

Do not sleep-and-poll. `wait` blocks until a condition is met, then prints
the resulting screen; the end reason goes to stderr (and to `--json`).

`puppetty <command...>` with no subcommand means `puppetty run <command...>`
(attached to your terminal). Agents almost always want `run -d` instead:
detached, named, controllable from separate invocations.

## Command reference

```text
run [opts] -- <cmd> [args...]   Start a session.
  -d, --detach          run in background, print the session name, return
  --name <n>            session name (default: derived from the command)
  --cwd <dir>           working directory   --cwd-of <session>  share another
                        session's cwd (side commands next to a dev server)
  --cols/--rows <n>     terminal size (default 120x30 detached)
  --keep, --linger      stay readable after the child exits, until kill (-d only)
  --auto                auto-answer prompts per policy    --decider "<cmd>"
                        refer unknown prompts to a command (implies --auto)
  --prompt-timeout <s>  give up on an unanswered prompt   --no-log  disable logs

send <name> [--no-enter] <text...>    Type text (Enter appended by default).
keys <name> <key...>                  enter tab esc space backspace up down
                                      left right home end pageup pagedown
                                      ctrl-c ctrl-d ctrl-z
read <name> [--json] [--scrollback] [--last]
wait <name> [conditions] [--timeout <s>] [--json]
snap <name> [-o file.svg|file.png] [--last]     Screen as an image.
export <name> [-o file.gif] [--fps <n>]         Recording as an animated GIF.
list [--json]        info <name>        kill <name>        attach <name>
mcp                  cred set|list|rm <ref> [--stdin]      config show|validate
```

## Waiting

Conditions combine; the first one met wins. Child exit and `--timeout`
(default 60s) always resolve.

```sh
puppetty wait dev --for "ready in"       # screen matches regex (--flags "i")
puppetty wait dev --for "Done" --since-start   # only lines changed after wait began
puppetty wait dev --gone "Compiling"     # regex DISAPPEARED (busy-TUI done-detector)
puppetty wait dev --stable 2000          # rendered screen unchanged for 2s
puppetty wait dev --prompt               # settled on a prompt-looking line
puppetty wait dev --idle 3000            # no output bytes for 3s
```

End reasons: `pattern` `gone` `stable` `prompt` `idle` `exit` `timeout`.
On `prompt`, the JSON output adds a classification — `promptLine`,
`promptClass` (`auto` / `confirm` / `forbid` / `unmatched`), `promptRule`,
`promptAction` — telling you who may answer (see Secrets below).

`--prompt` judges only after the screen has been quiet (`--quiet-ms`,
default 700), and requires 3× that quiet time when the cursor is not
sitting at the end of the prompt line — the signature of output that merely
paused rather than a program reading input.

**Recommended pattern (controller mode):** don't configure auto-answering.
`wait --prompt` → `read` the screen → decide yourself → `send`. You have the
context; the engine only detects that the session is blocked.

## Session lifetime

- A detached session stays readable for **~3 seconds after its child
  exits**, then disappears. For one-shot commands whose final screen is the
  answer (`codex exec ...`, a test run), start with `--keep`: the session
  stays until you `kill` it.
- Missed the window anyway? `read <name> --last` rebuilds the final screen
  from the session's recording — works after the session is gone. The
  "cannot reach session" error hints this when a recording exists.
- Session names must be unique among live sessions; without `--name` a free
  name is derived (`python`, `python-2`, …). `run -d` prints the actual name
  on stdout — capture it.

## Screenshots and recordings

```sh
puppetty snap app                    # live screen -> app.svg (vector, selectable text)
puppetty snap app -o shot.png        # rasterized (fonts embedded; works anywhere)
puppetty snap app --last -o out.png  # final screen from the recording
puppetty export app -o run.gif       # recording -> looping GIF, recorded timing
```

Every session records a `.cast` log (asciinema v2) under
`~/.puppetty/logs/` by default, so `read --last`, `snap --last`, and
`export` work retroactively on any past session. Box-drawing and block
characters render geometrically, so TUI borders are always clean.

## Secrets — rules you must follow

- **Never type a secret into a session.** Password/passphrase/token prompts
  classify as `forbid`: leave them to a human, or reference a stored
  credential.
- Store secrets in the OS keyring: `puppetty cred set <ref>` (interactive)
  or pipe with `--stdin`. A policy rule
  `{ "action": "credential", "ref": "<ref>" }` answers the prompt directly
  from the keyring — the secret never passes through you and only the ref is
  logged.
- Prompts containing danger words (delete / overwrite / force / `git push`
  / …) classify as `confirm`. Surface them to the human unless they have
  explicitly pre-approved the action.
- Session logs never contain typed input, only output and event metadata.

## MCP server

If you can register MCP servers, prefer this over shelling the CLI:

```jsonc
{ "mcpServers": { "puppetty": { "command": "npx", "args": ["-y", "puppetty", "mcp"] } } }
```

Tools: `puppetty_start_session`, `puppetty_send`, `puppetty_keys`,
`puppetty_read`, `puppetty_snap`, `puppetty_wait`, `puppetty_list`,
`puppetty_kill`. Text tools return the rendered screen; `puppetty_snap`
returns a PNG image (`last: true` renders from the recording) — use it when
visual layout matters: TUIs, menus, editors, selection highlights.
`puppetty_wait` returns the prompt classification, so check for `forbid`
before answering.

## JSON control protocol

Each session listens on a named pipe (`\\.\pipe\puppetty-<name>`; Unix
socket `~/.puppetty/run/<name>.sock` elsewhere). One JSON object per line
in, one out — drive it from any language without the CLI:

```text
{"op":"send","data":"hello","enter":true}
{"op":"keys","keys":["down","enter"]}
{"op":"read","scrollback":false}   → {"ok":true,"alive":true,"exitCode":null,"lines":[...],"cursor":{"x":0,"y":3}}
{"op":"read","restore":true}       → adds "restore" (ANSI repaint incl. colors), "cols", "rows"
{"op":"wait","pattern":"● ","flags":"i","idleMs":3000,"timeoutMs":60000}
                                   → {"ok":true,"reason":"pattern","waitedMs":4137,"lines":[...]}
{"op":"resize","cols":140,"rows":40}
{"op":"info"} / {"op":"kill"}
```

Live sessions are registered in `~/.puppetty/sessions/*.json` (includes the
pipe path). Failed requests return `{"ok":false,"error":"..."}`.

## Auto-answering (optional)

`--auto` answers prompts by layered JSONC policy (built-in defaults ←
`~/.puppetty/config.json` ← `<cwd>/.puppetty/config.json`; first matching
rule wins). `--decider "<cmd>"` pipes unrecognized prompts to a command that
reads the screen on stdin and replies one line: `SEND:<text>` / `ENTER` /
`CANCEL` / `WAIT`.

```jsonc
// .puppetty/config.json
{
  "rules": [
    // action: send | enter | forbid | decider | credential | ignore
    { "name": "project-name", "match": "Project name:\\s*$", "action": "send", "text": "my-app" }
  ],
  "dangerWords": ["\\boverwrite\\b", "git push"],       // escalate to confirm
  "onDanger": "human",                                  // or "decider"
  "onUnanswered": { "afterSec": 30, "do": "cancel" }
}
```

`puppetty config show` prints the effective merged policy;
`puppetty config validate` checks a policy piped to stdin. A loop guard
aborts after answering the same prompt 3 times.

## Exit codes and errors

| Code | Meaning |
| --- | --- |
| 0 | success (attached `run`: the child's own exit code) |
| 1 | `wait` ended by timeout; `config validate` rejected the input |
| 2 | error — message on stderr as `puppetty: ...` |
| 130 | autopilot gave up on an unanswered/cancelled prompt |

Common errors: `session "x" already exists` → pick another `--name` or
`kill` it. `cannot reach session "x"` → it exited and expired; use
`read x --last` (the error hints this when a recording exists).
`detached session failed to start` → the message includes the host's actual
spawn error.
