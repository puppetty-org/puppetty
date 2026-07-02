# puppetty-gui

Tauri 2 desktop shell over the puppetty session engine (see `../DESIGN.md`
§4.2). The engine stays the single source of truth: this app is a named-pipe
*client* — the same sessions remain fully drivable by the CLI and agents
while they're shown here.

## Features (M2 alpha)

- xterm.js terminal tabs live-attached to puppetty sessions (input + output;
  replay of recent history on attach)
- ＋ session — start any command as a new detached session
- ⌗ companion — open a `pwsh` tab in the active session's working directory
- Decision feed — the session's `.jsonl` event log, rendered live (who typed
  what, which rules fired, what was cancelled and why)
- "Waiting for input" banner + amber tab when a session is blocked on a
  prompt (`wait --prompt` polling)
- **Settings panel** (⚙) — four tabs: *Effective rules* (the merged policy
  with color-coded severity badges, read-only), *Edit config* (a structured
  add/edit/remove editor for your user rules — a form per rule with
  action/severity/match/answer, plus credential-ref picker — backed by a
  collapsible raw-JSONC editor for advanced keys; every save is validated by
  the engine before it's persisted), *Credentials* (add/list/remove secrets in
  the OS keyring; add takes a ref + value, the value goes straight to the
  keyring), and *Appearance*.
- **OS notifications** — a desktop toast fires on the rising edge of a
  session needing attention, so you can leave the app and get pulled back.
- **Appearance preferences** (Settings ▸ Appearance, persisted in
  localStorage) — UI language (English / 日本語), tab position (top
  horizontal / left vertical), show/hide the decision feed (also a top-bar
  toggle), color theme (Midnight / Slate / Light), terminal font size, and
  font family (a dropdown of the monospace fonts detected on your system,
  each previewed in its own typeface). Changes apply live to all open
  terminals.
- **Ask-human dialog** — when a session blocks on a `forbid` prompt
  (password/passphrase/token) or a `confirm` prompt (danger words like
  delete/overwrite/`git push`), a modal pops: masked secure input for
  secrets (with a reveal toggle), a prefilled confirmation for danger words.
  What you type goes straight to the program and is never logged (the event
  log records only a byte count). Cancel sends Ctrl+C. Escape is disabled so
  the choice is always explicit.

## Dev

```powershell
cd gui
npm install
npm run vendor   # copies xterm.js into ui/vendor
npm run dev      # tauri dev
```

Requires Node (the engine is `../bin/puppetty.js`, resolved relative to this
crate — dev mode only) and the Rust/Tauri toolchain.

The icon set is generated from `src-tauri/icon-source.png` (drawn by
`scripts/make-icon.ps1`: a marionette crossbar puppeteering a terminal
chevron). To change it, edit the script, rerun it, then
`npx tauri icon src-tauri/icon-source.png`. (`bundle.active` stays `false`
until release prep.)
