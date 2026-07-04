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

Works (verified against the Node CLI driving the same live session):

- `run -d` detached sessions, `run` attached (raw stdin/stdout bridge)
- Control ops: `send` (with Enter delay), `keys`, `read` (+ scrollback),
  `wait` (`pattern`/`gone`/`stable`/`prompt`/`idle`, `sinceStart`, timeout,
  exit), `resize`, `info`, `kill`, `attach` (input/resize/detach + data/exit
  events)
- Session registry entries compatible with the Node engine's `list`
- ConPTY cursor-query handshake: the engine answers `ESC[6n` cursor reports
  (ConPTY blocks the child until the first reply; TUIs also query at will)

Not yet ported (the Node engine remains the reference for these):

- Autopilot & policy (rules, severity classes, danger words, decider)
- Prompt classification in `wait` responses (`promptClass` etc.)
- Credentials (OS keyring), session logs (.cast/.jsonl), MCP server
- Attach replay preserves text only (no colors/attributes yet)
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
