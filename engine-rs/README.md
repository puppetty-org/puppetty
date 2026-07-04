# puppetty-engine (Rust port — alpha)

Rust reimplementation of the puppetty session engine (`../src`, Node.js).
Goal: a single static binary with no Node runtime requirement, so the GUI can
bundle it as a Tauri sidecar and the npm package can ship platform binaries
(the esbuild/Biome distribution model). The named-pipe JSON-lines control
protocol is identical — CLI clients, agents, and the GUI cannot tell which
engine hosts a session, and the two engines share the same session registry
(`~/.puppetty/sessions`).

Built on the crates the ecosystem already maintains instead of ports:
[`portable-pty`](https://crates.io/crates/portable-pty) (wezterm's PTY layer,
ConPTY on Windows), [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal)
(headless screen model, the xterm.js counterpart).

## Status

Ported and verified (including against the Node CLI driving the same live
session, and the repo's demo scripts running under the Rust autopilot):

- `run -d` detached sessions, `run` attached, implicit run (`puppetty <cmd>`),
  `attach` with Ctrl+] detach
- Control ops: `send` (with Enter delay), `keys`, `read` (+ scrollback),
  `wait` (`pattern`/`gone`/`stable`/`prompt`/`idle`, `sinceStart`, timeout,
  exit) with prompt classification (`promptClass`/`promptRule`/…), `resize`,
  `info`, `kill`, `set-auto`, `attach` event streams
- Autopilot & policy: layered JSONC config (defaults ← user ← project),
  rule severity classes, danger-word escalation, `{key}` expansion, loop
  guard (exit 130), onUnanswered cancel, decider (SEND/ENTER/CANCEL/WAIT),
  AI credential choice (CRED:<name>)
- Credentials in the OS keyring (`cred set/list/rm`, `--stdin`), names-only
  registry; `config show` / `config validate`
- Session logs: asciinema v2 `.cast` + structured `.jsonl` with source
  attribution and retention pruning
- MCP server over stdio (all 7 tools, hand-rolled JSON-RPC)
- ConPTY cursor-query handshake: the engine answers `ESC[6n` cursor reports
  (ConPTY blocks the child until the first reply; TUIs also query at will)

Known gaps vs the Node engine:

- Attach replay repaints text only (no colors/attributes yet — the Node
  engine uses xterm.js's serialize addon)
- User-config regexes run on fancy-regex: JS syntax largely works
  (lookaround, backrefs), but exotic JS-only constructs may differ
- Unix (code paths exist, only Windows is tested — same as the Node engine)

## Build & test

```powershell
cd engine-rs
cargo build --release   # target/release/puppetty-engine.exe
cargo test              # includes a live ConPTY round-trip test
```

## Parity check against the Node engine

```powershell
.\target\release\puppetty-engine.exe run -d --name x -- pwsh -NoLogo
node ..\bin\puppetty.js send x "echo hi"     # Node CLI drives the Rust host
node ..\bin\puppetty.js wait x --for hi
.\target\release\puppetty-engine.exe kill x
```
