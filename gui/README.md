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

## Install

Windows:

```powershell
iwr -useb https://puppetty-org.github.io/puppetty/gui/install.ps1 | iex
```

Linux:

```sh
curl -fsSL https://puppetty-org.github.io/puppetty/gui/install.sh | sh
```

The script installs the GUI and the Rust engine sidecar, creates the
platform shortcut/link, and writes an uninstall script. On Windows it
installs the WebView2 runtime if missing; on Linux the app needs the
WebKitGTK 4.1 runtime (`libwebkit2gtk-4.1-0` on Debian/Ubuntu). No separate
Node.js or npm install is required for the desktop app. The CLI/agent
toolchain (`puppetty` command, MCP server) can additionally be installed
with `npm install -g puppetty`. macOS packages are not published yet.

Uninstall from Windows **Settings ▸ Apps ▸ Installed apps** by selecting
**puppetty-gui** and choosing **Uninstall**. On Linux, run
`~/.local/share/puppetty-gui/uninstall.sh`.

## Dev

```powershell
cd gui
npm install
npm run vendor   # copies xterm.js into ui/vendor
npm run dev      # tauri dev
```

Requires Node (the engine is `../bin/puppetty.js`, resolved relative to this
crate) and the Rust/Tauri toolchain.

## Release

Pushing a tag like `gui-v0.1.0` runs the `GUI installer` GitHub Actions
workflow: it builds the Tauri app on Windows and Linux, zips the app
binary with the engine sidecar for each platform, and deploys the static
install endpoint to GitHub Pages:

```powershell
iwr -useb https://puppetty-org.github.io/puppetty/gui/install.ps1 | iex
curl -fsSL https://puppetty-org.github.io/puppetty/gui/install.sh | sh
```

The Pages payload contains `install.ps1`, `install.sh`, `latest.json`, and
`latest/puppetty-gui-<platform>.zip` packages plus `.sha256` files. Bump the version in
`src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml` before tagging. A
manual run from the Actions tab uploads the app package as a build artifact
instead of publishing.

The workflow also files a **draft GitHub Release** for the tag with the
same zip + `.sha256` assets and auto-generated notes. Publish it after
checking: published release assets are immutable, so they are the
tamper-evident archive that the mutable Pages endpoint can be audited
against.

The repository must have GitHub Pages enabled with **GitHub Actions** as
the source before the public `puppetty-org.github.io/puppetty/gui/` URL can
serve the installer. No separate Pages branch is needed.

The icon set is generated from `src-tauri/icon-source.png` (drawn by
`scripts/make-icon.ps1`: a marionette crossbar puppeteering a terminal
chevron). To change it, edit the script, rerun it, then
`npx tauri icon src-tauri/icon-source.png`.
