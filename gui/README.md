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
- **Math typesetting** (Settings ▸ Appearance) — LaTeX on the
  visible screen (`$x^2$`, `$$…$$`, `\(…\)`, `\[…\]` — including multi-line
  display blocks) is typeset with KaTeX and drawn over the source cells.
  Presentation only: the terminal grid and the child process are untouched,
  the row being typed on is never covered, and heuristics keep shell `$PATH`
  / price noise as plain text. Handy when an AI CLI answers with formulas.
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
iwr -useb https://github.com/puppetty-org/puppetty/releases/latest/download/install-gui.ps1 | iex
```

Linux / macOS (Apple Silicon):

```sh
curl -fsSL https://github.com/puppetty-org/puppetty/releases/latest/download/install-gui.sh | sh
```

The script — itself an asset of the newest stable GitHub Release —
resolves that release, downloads and SHA-256-verifies your platform's
package, installs the GUI and the Rust engine sidecar, creates the
platform shortcut/link, and writes an uninstall script. On Windows it
installs the WebView2 runtime if missing; on Linux the app needs the
WebKitGTK 4.1 runtime (`libwebkit2gtk-4.1-0` on Debian/Ubuntu); on macOS
(Apple Silicon only) the `.app` bundle installs into `~/Applications`. No
separate Node.js or npm install is required for the desktop app. The
CLI/agent toolchain (`puppetty` command, MCP server) can additionally be
installed with `npm install -g puppetty`.

Uninstall from Windows **Settings ▸ Apps ▸ Installed apps** by selecting
**puppetty-gui** and choosing **Uninstall**. On Linux, run
`~/.local/share/puppetty-gui/uninstall.sh`. On macOS, move
`~/Applications/puppetty-gui.app` to the Trash.

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

Pushing a tag like `gui-v0.2.0` runs the `GUI installer` GitHub Actions
workflow: it builds the Tauri app on Windows, Linux, and macOS (Apple
Silicon) and packages the app binary with the engine sidecar for each
platform — a zip on Windows, a `.tar.gz` on Linux (so the installer needs
only `tar`), and on macOS the whole `.app` bundle zipped via `ditto` —
then files a **draft GitHub Release** with the packages, their `.sha256`
files, the install scripts themselves, and auto-generated notes.
Publishing the draft is the release act: assets become immutable and
installable at that moment, and nothing ships without that explicit
review step. The stable one-liners fetch the script via
`releases/latest/download/`, so even the script is release-gated.

The install scripts resolve releases through the GitHub API: by default
the newest published non-prerelease `gui-v*` release that carries the
platform's package; tags with a prerelease suffix (e.g.
`gui-v0.3.0-beta.1`, marked as prereleases) are only selected on explicit
opt-in. Beta installs use the development script from `main` (there may
be no stable release to serve one, and beta testers want the newest
logic). Installing a beta, or pinning an exact version:

```powershell
$env:PUPPETTY_CHANNEL = "beta"; iwr -useb https://raw.githubusercontent.com/puppetty-org/puppetty/main/gui/scripts/install-gui.ps1 | iex
curl -fsSL https://raw.githubusercontent.com/puppetty-org/puppetty/main/gui/scripts/install-gui.sh | CHANNEL=beta sh
curl -fsSL https://raw.githubusercontent.com/puppetty-org/puppetty/main/gui/scripts/install-gui.sh | TAG=gui-v0.2.0-beta.1 sh
```

Bump the version in `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml`
before tagging. A manual run from the Actions tab uploads the app package
as a build artifact instead of drafting a release.

The icon set is generated from `src-tauri/icon-source.png` (drawn by
`scripts/make-icon.ps1`: a marionette crossbar puppeteering a terminal
chevron). To change it, edit the script, rerun it, then
`npx tauri icon src-tauri/icon-source.png`.
