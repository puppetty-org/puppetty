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
- Colored attach replay: a built-in serializer (the xterm serialize-addon
  counterpart) repaints the buffer with SGR colors/attributes and up to
  1000 lines of styled history; guarded by cell-exact round-trip tests
- ConPTY cursor-query handshake: the engine answers `ESC[6n` cursor reports
  (ConPTY blocks the child until the first reply; TUIs also query at will)

## JS regex compatibility

User-config rule patterns are written in JS RegExp syntax (configs are shared
with the Node engine). The Rust engine compiles them with fancy-regex; the
contract below is enforced by the `js_regex_compatibility_spec` test — every
supported construct is verified to behave exactly as a JS RegExp would.

Supported: `i`/`m`/`s` flags; classes and anchors (`\d \w \s \b ^ $`);
alternation, optional/quantifiers; lookahead `(?=…)`/`(?!…)` and lookbehind
`(?<=…)`/`(?<!…)`; backreferences `\1`; named groups `(?<name>…)`; `\uXXXX`
escapes; unicode literals in classes (e.g. `[:：]`); unicode properties
`\p{L}` (JS `/u` semantics).

Unsupported (rejected at compile time by `config validate`, never silently
different): control escapes `\cX`, empty character class `[]`, legacy octal
escapes.

## Platform status

- Windows is the primary target (ConPTY); full test suite + demos verified.
- Unix: same code paths built and tested on Linux (WSL2 Ubuntu) — PTY spawn,
  Unix-socket control endpoint, `sh -c` decider. Linux keyring uses the
  kernel keyutils (`linux-native` feature): secrets are per-session, not
  persisted across reboots; switch to a secret-service feature if you need
  persistence. macOS builds (apple-native keyring) but is untested.

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
