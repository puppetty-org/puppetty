// puppetty-gui frontend: tabs of xterm.js terminals attached to puppetty
// sessions, a decision feed from the session event log, and a banner when
// a session needs attention (blocked on a prompt after autonomous activity).
import { setLang, t, applyI18n } from './i18n.js';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const tabsEl = document.getElementById('tabs');
const termsEl = document.getElementById('terms');
const feedEl = document.getElementById('feed-items');
const bannerEl = document.getElementById('prompt-banner');
const layoutEl = document.getElementById('layout');
const topbarEl = document.getElementById('topbar');
const actionsEl = document.getElementById('actions');
const topbarRightEl = document.getElementById('topbar-right');

// ---------------------------------------------------------------- preferences

const PREF_KEY = 'puppetty.prefs';
const DEFAULT_PREFS = {
  lang: 'en',
  tabPos: 'top',        // 'top' (horizontal) | 'left' (vertical)
  showFeed: false,
  theme: 'midnight',    // midnight | slate | light
  fontFamily: 'Cascadia Mono, Consolas, monospace',
  fontSize: 14,
  opacity: 1,           // window/background opacity (0.5–1.0)
  onLastTab: 'newShell', // when the last tab closes: 'quit' the app | 'newShell'
  aiCommand: 'claude -p', // CLI the rule editor pipes a prompt to for regex suggestions
  autoAnswer: false,    // default for the New-command "auto-answer prompts" toggle
  confirmKillTab: true, // ask before the tab ✕ kills a live session
};

function hexToRgba(hex, a) {
  const m = hex.replace('#', '');
  const r = parseInt(m.slice(0, 2), 16), g = parseInt(m.slice(2, 4), 16), b = parseInt(m.slice(4, 6), 16);
  return `rgba(${r}, ${g}, ${b}, ${a})`;
}

const THEMES = {
  midnight: {
    vars: { '--bg': '#16181d', '--bg-alt': '#1e2128', '--fg': '#d8dee9', '--dim': '#6b7280', '--accent': '#7aa2f7', '--warn': '#e0af68', '--border': '#2a2e37', '--term-bg': '#101216' },
    xterm: { background: '#101216', foreground: '#d8dee9', cursor: '#7aa2f7' },
  },
  slate: {
    vars: { '--bg': '#1b1e26', '--bg-alt': '#232733', '--fg': '#c8d0e0', '--dim': '#7a8394', '--accent': '#8ab4f8', '--warn': '#e5c07b', '--border': '#333a48', '--term-bg': '#12151c' },
    xterm: { background: '#12151c', foreground: '#c8d0e0', cursor: '#8ab4f8' },
  },
  light: {
    vars: { '--bg': '#ffffff', '--bg-alt': '#f2f3f5', '--fg': '#1f2430', '--dim': '#6b7280', '--accent': '#3b6ea5', '--warn': '#b7791f', '--border': '#d7dbe0', '--term-bg': '#fbfbfd' },
    xterm: { background: '#fbfbfd', foreground: '#1f2430', cursor: '#3b6ea5' },
  },
};

// Common monospace/coding fonts to probe for; only the installed ones are
// offered in the Appearance dropdown.
const FONT_CANDIDATES = [
  'Cascadia Mono', 'Cascadia Code', 'Cascadia Mono PL', 'Cascadia Code PL',
  'Consolas', 'Courier New', 'Lucida Console', 'Fira Code', 'JetBrains Mono',
  'Source Code Pro', 'Roboto Mono', 'IBM Plex Mono', 'Hack', 'Inconsolata',
  'DejaVu Sans Mono', 'Liberation Mono', 'Noto Sans Mono', 'Anonymous Pro',
  'PT Mono', 'Iosevka', 'Victor Mono', 'MS Gothic', 'Meiryo',
  // macOS staples.
  'Menlo', 'Monaco', 'SF Mono', 'Andale Mono', 'Osaka-Mono',
  // Ubuntu Mono + common Nerd Font family-name variants (probed by exact
  // name; Homebrew casks and manual installs differ in NF / NFM / spelled-out
  // suffixes). The Custom… entry covers anything not listed here.
  'Ubuntu Mono', 'UbuntuMono NF', 'UbuntuMono NFM', 'UbuntuMono Nerd Font',
  'UbuntuMono Nerd Font Mono', 'CaskaydiaCove NF', 'CaskaydiaCove Nerd Font',
  'CaskaydiaCove NFM', 'CaskaydiaMono NF',
  'JetBrainsMono NF', 'JetBrainsMono NFM', 'JetBrainsMono Nerd Font',
  'JetBrainsMono Nerd Font Mono', 'FiraCode NF', 'FiraCode NFM',
  'FiraCode Nerd Font', 'FiraCode Nerd Font Mono',
  'Hack NF', 'Hack NFM', 'Hack Nerd Font', 'Hack Nerd Font Mono',
  'MesloLGS NF', 'MesloLGS Nerd Font', 'MesloLGS Nerd Font Mono',
  'MesloLGM Nerd Font', 'MesloLGL Nerd Font',
  'SauceCodePro NF', 'SauceCodePro Nerd Font',
  'RobotoMono Nerd Font', 'DejaVuSansM Nerd Font', 'Symbols Nerd Font Mono',
];

// Detect an installed font by comparing rendered text width against the three
// generic base families — if it differs from all of them, a real face exists.
const _fontCanvas = document.createElement('canvas').getContext('2d');
function fontAvailable(name) {
  const probe = 'mmmmmmmmmmlliWQ0123';
  for (const base of ['monospace', 'sans-serif', 'serif']) {
    _fontCanvas.font = `72px ${base}`;
    const baseW = _fontCanvas.measureText(probe).width;
    _fontCanvas.font = `72px "${name}", ${base}`;
    if (_fontCanvas.measureText(probe).width !== baseW) return true;
  }
  return false;
}
// A font is fixed-width if a narrow and a wide glyph render at the same width.
function isMonospace(name) {
  _fontCanvas.font = `72px "${name}"`;
  const narrow = _fontCanvas.measureText('i').width;
  const wide = _fontCanvas.measureText('W').width;
  return narrow > 0 && Math.abs(narrow - wide) < 0.5;
}
function primaryFont(stack) {
  return (stack || '').split(',')[0].trim().replace(/^["']|["']$/g, '');
}

const prefs = { ...DEFAULT_PREFS, ...loadPrefs() };
function loadPrefs() {
  try { return JSON.parse(localStorage.getItem(PREF_KEY)) || {}; } catch { return {}; }
}
function savePrefs() {
  localStorage.setItem(PREF_KEY, JSON.stringify(prefs));
}
function currentXtermTheme() {
  const theme = THEMES[prefs.theme] || THEMES.midnight;
  // Fully transparent xterm background: #terms carries the single tinted
  // rgba layer — two stacked alpha layers compound to near-opaque.
  return { ...theme.xterm, background: hexToRgba(theme.xterm.background, 0) };
}

function refitActive() {
  const s = sessions.get(active);
  if (!s) return;
  requestAnimationFrame(() => {
    s.fit.fit();
    s.lastResizeAt = Date.now();
    invoke('resize_session', { name: active, cols: s.term.cols, rows: s.term.rows }).catch(() => {});
  });
}

function applyTheme(name) {
  const theme = THEMES[name] || THEMES.midnight;
  for (const [k, v] of Object.entries(theme.vars)) document.documentElement.style.setProperty(k, v);
  applyGlass();
}

// Composite the theme's background colors at the current window opacity onto the
// --*-a CSS vars and the terminals, so a transparent window shows through.
function applyGlass() {
  const theme = THEMES[prefs.theme] || THEMES.midnight;
  const a = prefs.opacity ?? 1;
  const root = document.documentElement.style;
  root.setProperty('--bg-a', hexToRgba(theme.vars['--bg'], a));
  root.setProperty('--bg-alt-a', hexToRgba(theme.vars['--bg-alt'], a));
  root.setProperty('--term-bg-a', hexToRgba(theme.vars['--term-bg'], a));
  // xterm itself stays fully transparent (see currentXtermTheme): #terms'
  // --term-bg-a is the one tinted layer, matching the feed panel's look.
  const xbg = hexToRgba(theme.xterm.background, 0);
  for (const s of sessions.values()) s.term.options.theme = { ...theme.xterm, background: xbg };
}
function applyFont() {
  for (const s of sessions.values()) {
    s.term.options.fontFamily = prefs.fontFamily;
    s.term.options.fontSize = prefs.fontSize;
  }
  refitActive();
}
function applyTabPos(pos) {
  if (pos === 'left') {
    document.body.classList.add('tabs-left');
    layoutEl.insertBefore(tabsEl, termsEl);
  } else {
    document.body.classList.remove('tabs-left');
    // Before the ＋ button, not #topbar-right — the button must end up on
    // the RIGHT of the last tab.
    topbarEl.insertBefore(tabsEl, document.getElementById('btn-companion'));
  }
  // Relocating the strip across DOM containers can leave WebView2's compositor
  // holding a stale (blank) layer for it until the next reflow — the tab is in
  // the DOM but not painted. Force a synchronous reflow + repaint so it always
  // shows immediately after the switch.
  tabsEl.style.display = 'none';
  void tabsEl.offsetHeight; // flush layout
  tabsEl.style.display = '';
  refitActive();
}
function applyFeed(show) {
  document.body.classList.toggle('feed-hidden', !show);
  document.getElementById('btn-feed').classList.toggle('off', !show);
  refitActive();
}
function applyLang(lang) {
  setLang(lang);
  document.documentElement.lang = lang;
  applyI18n(document);
}
function applyAllPrefs() {
  applyLang(prefs.lang);
  applyTheme(prefs.theme); // also applies glass/opacity
  applyTabPos(prefs.tabPos);
  applyFeed(prefs.showFeed);
  document.getElementById('btn-auto')?.classList.toggle('on', prefs.autoAnswer);
  // font is applied per-terminal at creation; applyFont() covers live changes
}

// ---- custom window titlebar (frameless): controls + resize grips ----
const appWindow = window.__TAURI__?.window?.getCurrentWindow?.();
if (appWindow) {
  document.getElementById('win-min').onclick = () => appWindow.minimize();
  document.getElementById('win-max').onclick = () => appWindow.toggleMaximize();
  document.getElementById('win-close').onclick = () => appWindow.close();
  document.querySelectorAll('.resize-grip').forEach((grip) => {
    grip.addEventListener('mousedown', (e) => {
      e.preventDefault();
      appWindow.startResizeDragging(grip.dataset.dir);
    });
  });
}

// name -> { term, fit, holder, tab, alive, blocked, lastUserInput, autonomousAt, killedByUser }
const sessions = new Map();
let active = null;
window.__sessions = sessions; // debug/inspection hook

// A session "needs attention" only when it is blocked on a prompt AND the
// blocking follows output the local user didn't cause (agent work, script
// output). An idle shell sitting at its prompt does not qualify.
const AUTONOMOUS_GAP_MS = 5_000; // output this long after last local keystroke = autonomous
const RESIZE_GAP_MS = 3_000; // output right after a resize is a ConPTY repaint, not activity
const ATTENTION_TTL_MS = 120_000;

function needsAttention(s) {
  if (!s.alive || !s.blocked) return false;
  // Password/danger prompts are meaningful by class — flag them even if the
  // GUI attached after the blocking output.
  if (s.promptClass === 'forbid' || s.promptClass === 'confirm') return true;
  // A session you launched to run a script (not a bare interactive shell/REPL)
  // that has settled on a prompt is genuinely waiting for you — surface it even
  // if its output all landed during the attach/resize window, which the
  // autonomous-output guard below would otherwise mask.
  if (s.surfacePrompts) return true;
  // Otherwise (unmatched prompt — e.g. an idle shell at its prompt) it only
  // counts if we actually saw autonomous output precede the block.
  return s.autonomousAt > 0 && Date.now() - s.autonomousAt < ATTENTION_TTL_MS;
}

// Did the GUI launch this command to run a script/one-off (vs. drop into an
// interactive shell or REPL)? Used to decide whether a prompt block is a real
// "waiting for you" moment.
function runsScript(command) {
  if (!Array.isArray(command)) return false;
  return command.slice(1).some((a) =>
    /^-(file|c|command)$/i.test(a) || /\.(ps1|bat|cmd|sh|py|js|rb|pl)$/i.test(a));
}

// ---------------------------------------------------------------- terminals

function makeTerminal(name, command, auto) {
  const holder = document.createElement('div');
  holder.className = 'term-holder hidden';
  termsEl.appendChild(holder);

  const term = new Terminal({
    fontFamily: prefs.fontFamily,
    fontSize: prefs.fontSize,
    theme: currentXtermTheme(),
    allowTransparency: true, // so a reduced window opacity shows through the terminal
    scrollback: 5000,
  });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  term.open(holder);

  const entry = {
    term, fit, holder,
    alive: true, blocked: false, auto: !!auto,
    surfacePrompts: runsScript(command),
    lastUserInput: 0, autonomousAt: 0, lastResizeAt: Date.now(), killedByUser: false,
  };

  term.onData((data) => {
    entry.lastUserInput = Date.now();
    invoke('write_session', { name, data }).catch(() => {});
  });

  const tab = document.createElement('div');
  tab.className = 'tab';
  const label = document.createElement('span');
  label.className = 'tab-label';
  label.textContent = name;
  const autoBadge = document.createElement('span');
  autoBadge.className = 'tab-auto';
  autoBadge.textContent = 'auto';
  autoBadge.title = 'Toggle auto-answer for this tab';
  autoBadge.onclick = (e) => { e.stopPropagation(); toggleTabAuto(name, entry); };
  tab.appendChild(autoBadge);
  entry.tabAuto = autoBadge;
  refreshTabAuto(entry);
  const close = document.createElement('button');
  close.className = 'tab-close';
  close.title = `kill session "${name}"`;
  close.textContent = '✕';
  close.onclick = (e) => {
    e.stopPropagation();
    closeSession(name);
  };
  tab.append(label, close);
  tab.onclick = () => activate(name);
  tabsEl.appendChild(tab);
  entry.tab = tab;
  entry.tabLabel = label;

  sessions.set(name, entry);
  return entry;
}

function activate(name) {
  active = name;
  // ＋ button stays enabled: no active session just means a plain shell.
  for (const [n, s] of sessions) {
    s.holder.classList.toggle('hidden', n !== name);
    s.tab.classList.toggle('active', n === name);
  }
  const s = sessions.get(name);
  if (s) {
    requestAnimationFrame(() => {
      s.fit.fit();
      s.lastResizeAt = Date.now();
      invoke('resize_session', { name, cols: s.term.cols, rows: s.term.rows }).catch(() => {});
      s.term.focus();
    });
  }
  renderBanner();
  refreshFeed();
}

async function openSession(name, command, auto) {
  if (sessions.has(name)) return activate(name);
  const s = makeTerminal(name, command, auto);
  activate(name);
  // Fit must happen while visible, BEFORE attach — the engine resizes the
  // session to these dimensions and sends a restore that exactly fits.
  await new Promise((r) => requestAnimationFrame(r));
  s.fit.fit();
  s.lastResizeAt = Date.now(); // attach resizes the session -> repaint incoming
  try {
    await invoke('attach_session', { name, cols: s.term.cols, rows: s.term.rows });
  } catch (err) {
    s.term.write(`\r\n\x1b[31mattach failed: ${err}\x1b[0m\r\n`);
  }
  sizeToGrid(s);
}

// First launch only: size the window so the terminal shows 120x28 cells.
// Afterwards the window-state plugin remembers whatever the user resizes to.
async function sizeToGrid(s) {
  // "First run" comes from the backend (no saved window state at boot):
  // localStorage is per-profile, so dev and installed builds would each
  // re-fire the sizing and clobber the restored window size.
  if (window.__sizedThisRun || !(await invoke('is_first_run').catch(() => false))) return;
  try {
    await new Promise((r) => requestAnimationFrame(r)); // ensure a real layout
    const r = s.holder.querySelector('.xterm-screen').getBoundingClientRect();
    if (!r.width || !r.height) return; // not rendered yet — retry on next open
    const cellW = r.width / s.term.cols;
    const cellH = r.height / s.term.rows;
    // Everything around the grid (top bar, paddings, feed if shown) stays
    // constant, so grow/shrink the window by the grid-size delta.
    const w = Math.round(window.innerWidth + (120 - s.term.cols) * cellW);
    const h = Math.round(window.innerHeight + (28 - s.term.rows) * cellH);
    const win = window.__TAURI__.window.getCurrentWindow();
    await win.setSize(new window.__TAURI__.dpi.LogicalSize(w, h));
    window.__sizedThisRun = true; // once per process; plugin owns it after
  } catch (err) {
    console.error('sizeToGrid failed:', err);
  }
}

// window.confirm and window.alert are not implemented in WKWebView (macOS)
// or WebKitGTK (Linux) — confirm() silently returns undefined — so all
// confirmations and error popups go through this in-app dialog.
const confirmDialog = document.getElementById('confirm-dialog');
// opts.suppressKey names a boolean pref: when the user checks "don't ask
// again" the pref flips to false and the dialog auto-confirms from then on
// (re-enable from Settings).
function uiConfirm(message, { suppressKey } = {}) {
  if (suppressKey && prefs[suppressKey] === false) return Promise.resolve(true);
  return new Promise((resolve) => {
    document.getElementById('confirm-dialog-msg').textContent = message;
    document.getElementById('confirm-dialog-cancel').hidden = false;
    const row = document.getElementById('confirm-dialog-suppress');
    const box = document.getElementById('confirm-dialog-suppress-box');
    row.hidden = !suppressKey;
    box.checked = false;
    confirmDialog.addEventListener('close', () => {
      const ok = confirmDialog.returnValue === 'ok';
      if (ok && suppressKey && box.checked) {
        prefs[suppressKey] = false;
        savePrefs();
      }
      resolve(ok);
    }, { once: true });
    confirmDialog.showModal();
  });
}
function uiAlert(message) {
  return new Promise((resolve) => {
    document.getElementById('confirm-dialog-msg').textContent = message;
    document.getElementById('confirm-dialog-cancel').hidden = true;
    document.getElementById('confirm-dialog-suppress').hidden = true;
    confirmDialog.addEventListener('close', () => resolve(), { once: true });
    confirmDialog.showModal();
  });
}

async function closeSession(name) {
  const s = sessions.get(name);
  if (!s) return;
  if (s.alive) {
    if (!(await uiConfirm(t('tab.confirmKill').replace('{name}', name), { suppressKey: 'confirmKillTab' }))) return;
    await invoke('kill_session', { name }).catch(() => {});
    // tab is removed when the exit event arrives
  } else {
    removeTab(name);
  }
}

function removeTab(name) {
  const s = sessions.get(name);
  if (!s) return;
  s.term.dispose();
  s.holder.remove();
  s.tab.remove();
  sessions.delete(name);
  if (active === name) {
    active = null;
    const next = sessions.keys().next().value;
    if (next) activate(next);
    else {
      renderBanner();
      feedEl.innerHTML = '';
      if (prefs.onLastTab === 'newShell') {
        // Keep the app alive with a fresh shell. Guarded against a respawn storm
        // if the shell itself dies immediately.
        if (Date.now() - lastShellSpawnAt > 3000) startShell();
      } else {
        appWindow?.close(); // no tabs left — close the whole app
      }
    }
  }
}

// ---- auto-answer (autopilot) toggles ----
function refreshTabAuto(entry) {
  if (entry.tabAuto) entry.tabAuto.classList.toggle('off', !entry.auto);
}

// Toggle autopilot for one live session via the daemon.
async function toggleTabAuto(name, entry) {
  try {
    entry.auto = await invoke('set_auto', { name, enabled: !entry.auto });
    refreshTabAuto(entry);
  } catch (err) { /* session may be gone */ }
}

// Global toggle: set the default for new sessions and apply to every live one.
async function setAutoAnswer(on) {
  prefs.autoAnswer = on;
  savePrefs();
  document.getElementById('btn-auto')?.classList.toggle('on', on);
  const cb = document.getElementById('pref-autoanswer');
  if (cb) cb.checked = on;
  for (const [name, s] of sessions) {
    if (!s.alive) continue;
    try {
      s.auto = await invoke('set_auto', { name, enabled: on });
      refreshTabAuto(s);
    } catch { /* ignore */ }
  }
}
// The global auto toggle lives in Settings (pref-autoanswer) — no top-bar
// button; per-tab AUTO badges toggle individual sessions.

// Platform default shell, resolved by the backend (pwsh on Windows,
// $SHELL elsewhere) and cached.
let defaultShellCmd = null;
async function shellCommand() {
  if (!defaultShellCmd) defaultShellCmd = await invoke('default_shell');
  return defaultShellCmd;
}

// Placeholder tab shown while start_session runs (it can take seconds).
// On failure it becomes a dismissable error tab — errors must live in the
// UI because WKWebView does not implement window.alert.
function pendingTab() {
  const tab = document.createElement('div');
  tab.className = 'tab pending';
  const label = document.createElement('span');
  label.className = 'tab-label';
  label.textContent = 'starting shell…';
  tab.appendChild(label);
  tabsEl.appendChild(tab);
  return {
    fail(message) {
      tab.classList.remove('pending');
      tab.classList.add('error');
      label.textContent = message;
      label.title = message;
      const close = document.createElement('button');
      close.className = 'tab-close';
      close.textContent = '✕';
      close.onclick = () => tab.remove();
      tab.appendChild(close);
    },
    remove() {
      tab.remove();
    },
  };
}

let lastShellSpawnAt = 0;
async function startShell() {
  lastShellSpawnAt = Date.now();
  const pending = pendingTab();
  try {
    const auto = prefs.autoAnswer;
    const cmd = await shellCommand();
    const created = await invoke('start_session', { command: cmd, name: null, cwdOf: null, auto });
    pending.remove();
    await openSession(created, cmd, auto);
  } catch (err) {
    console.error('startShell failed:', err);
    pending.fail(`shell failed: ${err?.message ?? err}`);
  }
}

// One event channel for all sessions; route by name.
await listen('session-msg', ({ payload }) => {
  const s = sessions.get(payload.name);
  if (!s) return;
  const msg = payload.msg;
  if (msg.event === 'data') {
    s.term.write(msg.data);
    // Restore replays don't count as activity; neither does output right
    // after local typing (interactive echo) or a resize (ConPTY repaint).
    const now = Date.now();
    if (
      !msg.replay &&
      now - s.lastUserInput > AUTONOMOUS_GAP_MS &&
      now - s.lastResizeAt > RESIZE_GAP_MS
    ) {
      s.autonomousAt = now;
    }
  } else if (msg.event === 'exit') {
    s.alive = false;
    s.blocked = false;
    removeTab(payload.name); // the process ended — close its tab
  } else if (msg.event === 'disconnected' && s.alive) {
    s.tabLabel.innerHTML = `${payload.name}<span class="status">disconnected</span>`;
  }
});

window.addEventListener('resize', () => {
  const s = sessions.get(active);
  if (!s) return;
  s.fit.fit();
  s.lastResizeAt = Date.now();
  invoke('resize_session', { name: active, cols: s.term.cols, rows: s.term.rows }).catch(() => {});
});

// ---------------------------------------------------------------- actions

document.getElementById('btn-companion').onclick = async () => {
  if (!active) return startShell(); // no session yet: plain shell instead
  const pending = pendingTab();
  try {
    const auto = sessions.get(active)?.auto ?? prefs.autoAnswer;
    const created = await invoke('start_session', {
      command: await shellCommand(),
      name: null,
      cwdOf: active,
      auto,
    });
    pending.remove();
    await openSession(created, undefined, auto);
  } catch (err) {
    console.error('companion failed:', err);
    pending.fail(`shell failed: ${err?.message ?? err}`);
  }
};

// ---------------------------------------------------------------- settings

const settingsDialog = document.getElementById('settings-dialog');

document.getElementById('btn-settings').onclick = () => {
  showPane('pane-rules');
  loadRules();
  loadUserConfig();
  loadCreds();
  loadAppearanceControls();
  // Clear any drag offset from a previous open so it re-centers each time.
  settingsDialog.style.left = '';
  settingsDialog.style.top = '';
  settingsDialog.style.margin = '';
  settingsDialog.style.position = '';
  settingsDialog.showModal();
};

// Let the settings dialog be dragged by its header (grab-anywhere except the
// Close button). Modal dialogs default to centered via margin:auto; on first
// drag we switch to explicit fixed left/top and follow the pointer.
makeDraggable(settingsDialog, settingsDialog.querySelector('.settings-head'));
function makeDraggable(dialog, handle) {
  let dragging = false, startX = 0, startY = 0, originX = 0, originY = 0;
  handle.addEventListener('mousedown', (e) => {
    if (e.target.closest('button')) return; // don't hijack the Close button
    const r = dialog.getBoundingClientRect();
    dialog.style.position = 'fixed';
    dialog.style.margin = '0';
    dialog.style.left = `${r.left}px`;
    dialog.style.top = `${r.top}px`;
    dragging = true; startX = e.clientX; startY = e.clientY; originX = r.left; originY = r.top;
    e.preventDefault();
  });
  window.addEventListener('mousemove', (e) => {
    if (!dragging) return;
    dialog.style.left = `${originX + e.clientX - startX}px`;
    dialog.style.top = `${originY + e.clientY - startY}px`;
  });
  window.addEventListener('mouseup', () => { dragging = false; });
}

// ---- Appearance preferences wiring ----

document.getElementById('btn-feed').onclick = () => {
  prefs.showFeed = !prefs.showFeed;
  applyFeed(prefs.showFeed);
  savePrefs();
  const cb = document.getElementById('pref-feed');
  if (cb) cb.checked = prefs.showFeed;
};

function loadAppearanceControls() {
  document.getElementById('pref-lang').value = prefs.lang;
  document.getElementById('pref-tabpos').value = prefs.tabPos;
  document.getElementById('pref-lasttab').value = prefs.onLastTab;
  document.getElementById('pref-aicommand').value = prefs.aiCommand ?? '';
  document.getElementById('pref-autoanswer').checked = prefs.autoAnswer;
  document.getElementById('pref-feed').checked = prefs.showFeed;
  document.getElementById('pref-confirmkill').checked = prefs.confirmKillTab !== false;
  invoke('get_remote_debug')
    .then((on) => { document.getElementById('pref-remotedebug').checked = !!on; })
    .catch(() => {});
  document.getElementById('pref-theme').value = prefs.theme;
  populateFontSelect();
  document.getElementById('pref-fontsize').value = prefs.fontSize;
  const op = Math.round((prefs.opacity ?? 1) * 100);
  document.getElementById('pref-opacity').value = op;
  document.getElementById('pref-opacity-val').textContent = `${op}%`;
}

let _installedFonts = null; // cached backend enumeration
async function populateFontSelect() {
  const sel = document.getElementById('pref-font');
  const cur = primaryFont(prefs.fontFamily);
  const render = (fonts) => {
    const list = fonts.includes(cur) ? fonts.slice() : [cur, ...fonts];
    sel.innerHTML =
      list
        .map((f) => `<option value="${f}" style="font-family:'${f}',monospace">${f}</option>`)
        .join('') + `<option value="__custom__">${t('appearance.customFont')}</option>`;
    sel.value = cur;
  };

  // Render an immediate list from the probe so the control is never empty,
  // then upgrade to the backend's full enumeration (the webview's Local
  // Font Access API is Chromium-only, so the backend does the listing).
  render(_installedFonts || FONT_CANDIDATES.filter(fontAvailable).sort((a, b) => a.localeCompare(b)));
  if (_installedFonts) return;
  try {
    const fams = await invoke('list_mono_fonts');
    // Some Nerd Font variants are not flagged fixed-pitch in their tables;
    // keep any probed candidates the enumeration missed.
    const merged = [...new Set([...fams, ...FONT_CANDIDATES.filter(fontAvailable)])]
      .sort((a, b) => a.localeCompare(b));
    if (merged.length) {
      _installedFonts = merged;
      render(merged);
    }
  } catch {
    /* enumeration unavailable — the probe list stays */
  }
}

document.getElementById('pref-lang').onchange = (e) => {
  prefs.lang = e.target.value; applyLang(prefs.lang); savePrefs();
};
document.getElementById('pref-tabpos').onchange = (e) => {
  prefs.tabPos = e.target.value; applyTabPos(prefs.tabPos); savePrefs();
};
document.getElementById('pref-lasttab').onchange = (e) => {
  prefs.onLastTab = e.target.value; savePrefs();
};
document.getElementById('pref-aicommand').onchange = (e) => {
  prefs.aiCommand = e.target.value.trim(); savePrefs();
};
document.getElementById('pref-autoanswer').onchange = (e) => {
  setAutoAnswer(e.target.checked);
};
document.getElementById('pref-feed').onchange = (e) => {
  prefs.showFeed = e.target.checked; applyFeed(prefs.showFeed); savePrefs();
};
document.getElementById('pref-confirmkill').onchange = (e) => {
  prefs.confirmKillTab = e.target.checked; savePrefs();
};
document.getElementById('pref-remotedebug').onchange = (e) => {
  invoke('set_remote_debug', { enabled: e.target.checked }).catch((err) => uiAlert(String(err)));
};
document.getElementById('pref-theme').onchange = (e) => {
  prefs.theme = e.target.value; applyTheme(prefs.theme); savePrefs();
};
document.getElementById('pref-font').onchange = (e) => {
  const custom = document.getElementById('pref-font-custom');
  if (e.target.value === '__custom__') {
    // Free-text entry: the probe list can never cover every family name
    // (and queryLocalFonts is Chromium-only, so WKWebView/WebKitGTK users
    // have no enumeration).
    custom.hidden = false;
    custom.value = primaryFont(prefs.fontFamily);
    custom.focus();
    return;
  }
  custom.hidden = true;
  prefs.fontFamily = `"${e.target.value}", monospace`; applyFont(); savePrefs();
};
document.getElementById('pref-font-custom').onchange = (e) => {
  const name = e.target.value.trim();
  if (!name) return;
  prefs.fontFamily = `"${name}", monospace`; applyFont(); savePrefs();
  e.target.hidden = true;
  populateFontSelect(); // re-render so the custom family shows as selected
};
document.getElementById('pref-fontsize').onchange = (e) => {
  const n = Math.min(32, Math.max(8, Number(e.target.value) || DEFAULT_PREFS.fontSize));
  prefs.fontSize = n; e.target.value = n; applyFont(); savePrefs();
};
document.getElementById('pref-opacity').oninput = (e) => {
  const pct = Math.min(100, Math.max(50, Number(e.target.value) || 100));
  prefs.opacity = pct / 100;
  document.getElementById('pref-opacity-val').textContent = `${pct}%`;
  applyGlass();
};
document.getElementById('pref-opacity').onchange = () => savePrefs();

document.querySelectorAll('.stab').forEach((btn) => {
  btn.onclick = () => showPane(btn.dataset.pane);
});
function showPane(id) {
  document.querySelectorAll('.stab').forEach((b) => b.classList.toggle('active', b.dataset.pane === id));
  document.querySelectorAll('.spane').forEach((p) => p.classList.toggle('hidden', p.id !== id));
}

async function loadRules() {
  const tbody = document.querySelector('#rules-table tbody');
  const addBtn = document.getElementById('rule-add-btn');
  tbody.innerHTML = '';
  let cfg;
  try {
    cfg = await invoke('config_effective');
  } catch (e) {
    tbody.innerHTML = `<tr><td>failed to load: ${escapeHtml(String(e))}</td></tr>`;
    return;
  }
  document.getElementById('cfg-path').textContent = cfg.userConfigPath || '';
  // Parse the user layer so we know which effective rules are yours (directly
  // editable) vs. built-in defaults (which you override or disable).
  let userText = '';
  try { userText = await invoke('config_read_user'); } catch { /* no user config yet */ }
  syncModelFromText(userText);
  addBtn.disabled = userConfigObj === null; // unparseable config → editing disabled
  for (const r of cfg.rules || []) tbody.appendChild(buildRuleRow(r));
}

// Build one effective-rules row. `userNames` is recomputed here so a single row
// can be rebuilt in place (e.g. after a checkbox toggle) without a full reload.
function buildRuleRow(r) {
  const userNames = new Set((userConfigObj?.rules || []).map((u) => u.name));
  const cls = ruleClassOf(r);
  const inUser = userNames.has(r.name);
  const src = inUser ? 'user' : 'default';
  const enabled = !r.disabled;
  const tr = document.createElement('tr');
  tr.classList.toggle('rule-off', !enabled);
  tr.innerHTML =
    `<td class="rule-toggle"></td>` +
    `<td class="rule-name">${escapeHtml(r.name || '')}</td>` +
    `<td class="rule-match">${escapeHtml(r.match || '')}</td>` +
    `<td><span class="rule-badge ${cls}">${cls}</span></td>` +
    `<td>${escapeHtml(ruleAnswerOf(r))}</td>` +
    `<td><span class="src-badge ${src}">${escapeHtml(t('rule.src.' + src))}</span></td>` +
    `<td class="rule-ops"></td>`;
  // Enable/disable checkbox
  const cb = document.createElement('input');
  cb.type = 'checkbox';
  cb.checked = enabled;
  cb.title = t('rule.enabled');
  cb.disabled = userConfigObj === null;
  cb.onchange = () => setRuleEnabled(r, cb.checked, tr);
  tr.querySelector('.rule-toggle').appendChild(cb);
  // Actions
  const ops = tr.querySelector('.rule-ops');
  if (userConfigObj !== null) {
    if (inUser) {
      const idx = userConfigObj.rules.findIndex((u) => u.name === r.name);
      ops.append(mkRuleOp(t('common.edit'), () => openRuleDialog(idx)));
      ops.append(mkRuleOp(t('common.remove'), () => removeRule(idx), true));
    } else {
      ops.append(mkRuleOp(t('rule.override'), () => openRuleDialog(null, r)));
    }
  }
  return tr;
}

// Copy an effective rule's fields into the shape stored in the user config.
function ruleToUserFields(r) {
  const o = { name: r.name, match: r.match, action: r.action };
  if (r.flags) o.flags = r.flags;
  if (r.class) o.class = r.class;
  if (r.text != null && r.action === 'send') o.text = r.text;
  if (r.ref) o.ref = r.ref;
  if (r.scope) o.scope = r.scope;
  if (r.ai) o.ai = r.ai;
  if (r.describe) o.describe = r.describe;
  if (r.enter === false) o.enter = false;
  return o;
}

// Enable/disable a rule. User rules flip their `disabled` field; a built-in is
// disabled by shadowing it with a disabled copy in the user layer.
async function setRuleEnabled(rule, enable, tr) {
  if (userConfigObj === null) return;
  if (!Array.isArray(userConfigObj.rules)) userConfigObj.rules = [];
  const idx = userConfigObj.rules.findIndex((u) => u.name === rule.name);
  if (idx >= 0) {
    if (enable) delete userConfigObj.rules[idx].disabled;
    else userConfigObj.rules[idx].disabled = true;
  } else if (!enable) {
    userConfigObj.rules.push({ ...ruleToUserFields(rule), disabled: true });
  }
  // Persist without a full reload, then patch just this row so the dialog
  // doesn't flash/scroll on every toggle.
  const ok = await saveUserConfig({ rerender: false });
  if (!ok) { await loadRules(); return; } // validation failed → resync from disk
  rule.disabled = !enable;
  tr.replaceWith(buildRuleRow(rule));
}

function mkRuleOp(label, onclick, danger) {
  const b = document.createElement('button');
  b.type = 'button';
  b.className = 'rule-op' + (danger ? ' danger' : '');
  b.textContent = label;
  b.onclick = onclick;
  return b;
}

// Parsed user config backing the structured rules editor. `null` means the
// on-disk config couldn't be parsed here (e.g. exotic JSONC) → editor disabled,
// raw editor still available.
let userConfigObj = { rules: [] };

// Minimal JSONC → object: strip //-and-/* */ comments (string-aware) and
// trailing commas, then JSON.parse. Mirrors the engine's parseJsonc closely
// enough for configs this app produces.
function parseJsonc(text) {
  let out = '';
  let inStr = false, esc = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (inStr) {
      out += c;
      if (esc) esc = false;
      else if (c === '\\') esc = true;
      else if (c === '"') inStr = false;
      continue;
    }
    if (c === '"') { inStr = true; out += c; continue; }
    if (c === '/' && text[i + 1] === '/') { while (i < text.length && text[i] !== '\n') i++; out += '\n'; continue; }
    if (c === '/' && text[i + 1] === '*') { i += 2; while (i < text.length && !(text[i] === '*' && text[i + 1] === '/')) i++; i++; continue; }
    out += c;
  }
  out = out.replace(/,(\s*[}\]])/g, '$1');
  return JSON.parse(out);
}

async function loadUserConfig() {
  const ta = document.getElementById('config-text');
  const status = document.getElementById('config-status');
  status.textContent = '';
  status.className = '';
  let text = '';
  try {
    text = await invoke('config_read_user');
  } catch (e) {
    ta.value = '';
    status.textContent = String(e);
    status.className = 'err';
    userConfigObj = null;
    return;
  }
  ta.value = text || '{\n  "rules": [\n  ]\n}\n';
  syncModelFromText(text);
}

// Parse raw config text into userConfigObj (backing the rule editor). `null`
// means it couldn't be parsed here (exotic JSONC) → structured editing disabled.
function syncModelFromText(text) {
  try {
    userConfigObj = text && text.trim() ? parseJsonc(text) : { rules: [] };
    if (typeof userConfigObj !== 'object' || userConfigObj === null || Array.isArray(userConfigObj)) userConfigObj = { rules: [] };
    if (!Array.isArray(userConfigObj.rules)) userConfigObj.rules = [];
  } catch {
    userConfigObj = null;
  }
}

function ruleClassOf(r) {
  return r.action === 'credential' ? 'credential' : (r.class || (r.action === 'forbid' ? 'forbid' : 'auto'));
}
function ruleAnswerOf(r) {
  if (r.action === 'credential') return r.ai ? 'credential (AI-chosen)' : `ref: ${r.ref || ''}`;
  if (r.action === 'send') return JSON.stringify(r.text ?? '') + (r.enter === false ? ' (no Enter)' : '');
  return r.action || '';
}

// Persist the model through the engine-validated write path, then refresh the
// effective-rules view.
async function saveUserConfig({ rerender = true } = {}) {
  const text = JSON.stringify(userConfigObj, null, 2) + '\n';
  try {
    await invoke('config_write_user', { text });
    document.getElementById('config-text').value = text;
    if (rerender) await loadRules(); // re-render effective view (also re-syncs the model)
    return true;
  } catch (e) {
    await uiAlert(String(e)); // validation error from the engine
    return false;
  }
}

async function removeRule(i) {
  const r = userConfigObj.rules[i];
  if (!(await uiConfirm(t('rule.confirmRemove').replace('{name}', r?.name || '')))) return;
  userConfigObj.rules.splice(i, 1);
  await saveUserConfig();
}

// ---- rule add/edit dialog ----
const ruleDialog = document.getElementById('rule-dialog');
let ruleEditIndex = null; // null = adding
let rulePending = null;

document.getElementById('rule-add-btn').onclick = () => openRuleDialog(null);
document.getElementById('rule-f-action').onchange = updateRuleFieldVisibility;
ruleDialog.addEventListener('cancel', () => { rulePending = null; });

function updateRuleFieldVisibility() {
  const a = document.getElementById('rule-f-action').value;
  const send = a === 'send';
  document.getElementById('rule-f-text-row').style.display = send ? '' : 'none';
  document.getElementById('rule-f-keyshint').style.display = send ? '' : 'none';
  document.getElementById('rule-f-enter-row').style.display = send ? '' : 'none';
  document.getElementById('rule-f-ref-row').style.display = a === 'credential' ? '' : 'none';
  const describe = document.getElementById('rule-f-matchmode').value === 'describe';
  document.getElementById('rule-f-describe-row').style.display = describe ? '' : 'none';
  document.getElementById('rule-f-match-hint').style.display = describe ? '' : 'none';
}

document.getElementById('rule-f-matchmode').onchange = () => {
  document.getElementById('rule-f-suggest-status').textContent = '';
  updateRuleFieldVisibility();
};

// Ask the configured AI CLI to turn a plain-language description into a regex,
// then drop it into the (still editable) Match field for review.
document.getElementById('rule-f-suggest').onclick = async () => {
  const status = document.getElementById('rule-f-suggest-status');
  const cmd = (prefs.aiCommand || '').trim();
  if (!cmd) { status.className = 'field-hint err'; status.textContent = t('rule.suggestNoCmd'); return; }
  const desc = document.getElementById('rule-f-describe').value.trim();
  if (!desc) { status.className = 'field-hint err'; status.textContent = t('rule.suggestEmpty'); return; }
  const btn = document.getElementById('rule-f-suggest');
  btn.disabled = true;
  status.className = 'field-hint';
  status.textContent = t('rule.suggesting');
  const prompt =
    'You convert a description of a terminal prompt into ONE JavaScript regular expression that matches such a prompt.\n' +
    'Output ONLY the regex on a single line: no delimiters, no quotes, no code fences, no explanation.\n' +
    'Keep it general enough to match variants but specific enough to avoid false positives.\n\n' +
    'Description: ' + desc + '\n';
  try {
    const raw = await invoke('ai_complete', { command: cmd, input: prompt });
    const regex = sanitizeRegex(raw);
    if (!regex) throw new Error('empty response');
    new RegExp(regex); // validate before accepting
    document.getElementById('rule-f-match').value = regex;
    status.className = 'field-hint ok';
    status.textContent = t('rule.suggestOk');
  } catch (e) {
    status.className = 'field-hint err';
    status.textContent = t('rule.suggestFail').replace('{err}', String(e).slice(0, 160));
  } finally {
    btn.disabled = false;
  }
};

// Strip common wrapping the model may add (code fences, /…/ delimiters, quotes)
// and keep the first non-empty line.
function sanitizeRegex(raw) {
  let s = (raw || '').trim();
  s = s.replace(/^```[a-z]*\s*/i, '').replace(/```$/,'').trim();
  s = s.split('\n').map((l) => l.trim()).find((l) => l.length) || '';
  if (s.length >= 2 && s.startsWith('/') && s.lastIndexOf('/') > 0) {
    s = s.slice(1, s.lastIndexOf('/')); // /pattern/flags -> pattern
  }
  if (s.length >= 2 && ((s[0] === '"' && s.endsWith('"')) || (s[0] === "'" && s.endsWith("'")))) {
    s = s.slice(1, -1);
  }
  return s.trim();
}

async function openRuleDialog(index, prefill) {
  ruleEditIndex = index;
  rulePending = null;
  const r = index != null ? (userConfigObj.rules[index] || {}) : (prefill || { action: 'send' });
  document.getElementById('rule-dialog-title').textContent = index == null ? t('rule.addTitle') : t('rule.editTitle');
  document.getElementById('rule-f-name').value = r.name || '';
  document.getElementById('rule-f-match').value = r.match || '';
  document.getElementById('rule-f-action').value = r.action || 'send';
  document.getElementById('rule-f-text').value = r.text || '';
  document.getElementById('rule-f-enter').checked = r.enter !== false; // default: append Enter
  document.getElementById('rule-f-class').value = r.class || '';
  // A saved `describe` means the regex was authored via the AI helper.
  document.getElementById('rule-f-describe').value = r.describe || '';
  document.getElementById('rule-f-matchmode').value = r.describe ? 'describe' : 'regex';
  document.getElementById('rule-f-scope').value = r.scope === 'screen' ? 'screen' : 'line';
  document.getElementById('rule-f-suggest-status').textContent = '';
  document.getElementById('rule-dialog-err').textContent = '';
  await populateRefSelect(r.ai ? '@ai' : r.ref);
  updateRuleFieldVisibility();
  ruleDialog.showModal();
}

async function populateRefSelect(selected) {
  const el = document.getElementById('rule-f-ref');
  let refs = [];
  try { refs = await invoke('cred_list'); } catch { /* none */ }
  // "@ai" is a sentinel: no fixed credential — the decider picks one at runtime.
  const aiOpt = `<option value="@ai">${escapeHtml(t('rule.ref.ai'))}</option>`;
  el.innerHTML = aiOpt + (refs.length
    ? refs.map((r) => `<option value="${escapeHtml(r)}">${escapeHtml(r)}</option>`).join('')
    : `<option value="">—</option>`);
  if (selected) el.value = selected;
}

function buildRuleFromForm() {
  const action = document.getElementById('rule-f-action').value;
  const rule = {
    name: document.getElementById('rule-f-name').value.trim(),
    match: document.getElementById('rule-f-match').value,
    action,
  };
  if (action === 'send') {
    rule.text = document.getElementById('rule-f-text').value;
    // Only record enter:false — the default (append Enter) stays implicit.
    if (!document.getElementById('rule-f-enter').checked) rule.enter = false;
  }
  if (action === 'credential') {
    const ref = document.getElementById('rule-f-ref').value;
    if (ref === '@ai') rule.ai = true; // decider chooses the credential
    else rule.ref = ref;
  }
  const cls = document.getElementById('rule-f-class').value;
  if (cls) rule.class = cls;
  // Match the whole visible prompt instead of just its last line (for boxed
  // multi-line TUI prompts). Omit for the default line scope.
  if (document.getElementById('rule-f-scope').value === 'screen') rule.scope = 'screen';
  // Keep the description alongside the regex so re-editing stays in Describe
  // mode; the engine ignores unknown fields.
  if (document.getElementById('rule-f-matchmode').value === 'describe') {
    const d = document.getElementById('rule-f-describe').value.trim();
    if (d) rule.describe = d;
  }
  return rule;
}

function validateRule(r) {
  if (!r.name || !r.match) return t('rule.errName');
  try { new RegExp(r.match); } catch { return t('rule.errRegex'); }
  if (r.action === 'send' && !r.text) return t('rule.errText');
  if (r.action === 'credential' && !r.ref && !r.ai) return t('rule.errRef');
  return null;
}

document.getElementById('rule-dialog-save').onclick = (e) => {
  const rule = buildRuleFromForm();
  const err = validateRule(rule);
  if (err) { e.preventDefault(); document.getElementById('rule-dialog-err').textContent = err; return; }
  rulePending = rule; // applied in the close handler
};

ruleDialog.addEventListener('close', async () => {
  if (ruleDialog.returnValue !== 'save' || !rulePending) { rulePending = null; return; }
  const rule = rulePending; rulePending = null;
  if (!Array.isArray(userConfigObj.rules)) userConfigObj.rules = [];
  if (ruleEditIndex == null) userConfigObj.rules.push(rule);
  else userConfigObj.rules[ruleEditIndex] = rule;
  await saveUserConfig();
});

document.getElementById('config-save').onclick = async () => {
  const status = document.getElementById('config-status');
  const text = document.getElementById('config-text').value;
  try {
    await invoke('config_write_user', { text });
    status.textContent = t('config.saved');
    status.className = 'ok';
    syncModelFromText(text); // resync the structured editor
    loadRules(); // reflect changes in the effective view
  } catch (e) {
    status.textContent = String(e);
    status.className = 'err';
  }
};

async function loadCreds() {
  const ul = document.getElementById('cred-list');
  ul.innerHTML = '';
  let refs = [];
  try {
    refs = await invoke('cred_list');
  } catch (e) {
    ul.innerHTML = `<li class="empty">failed to load: ${escapeHtml(String(e))}</li>`;
    return;
  }
  if (refs.length === 0) {
    ul.innerHTML = '<li class="empty">No stored credentials.</li>';
    return;
  }
  for (const ref of refs) {
    const li = document.createElement('li');
    const name = document.createElement('span');
    name.className = 'cred-ref';
    name.textContent = ref;
    const del = document.createElement('button');
    del.textContent = 'remove';
    del.onclick = async () => {
      if (!(await uiConfirm(t('cred.confirmRemove').replace('{ref}', ref)))) return;
      await invoke('cred_rm', { reference: ref }).catch(() => {});
      loadCreds();
    };
    li.append(name, del);
    ul.appendChild(li);
  }
}

document.getElementById('cred-add-btn').onclick = async () => {
  const ref = document.getElementById('cred-ref').value.trim();
  const secret = document.getElementById('cred-secret').value;
  if (!ref || !secret) return;
  try {
    await invoke('cred_set', { reference: ref, secret });
    document.getElementById('cred-ref').value = '';
    document.getElementById('cred-secret').value = '';
    loadCreds();
  } catch (e) {
    await uiAlert(`failed to store: ${e}`);
  }
};

// ---------------------------------------------------------------- feed + banner

function renderBanner() {
  const s = sessions.get(active);
  bannerEl.classList.toggle('hidden', !(s && needsAttention(s)));
}

async function refreshFeed() {
  if (!active) return;
  let events = [];
  try {
    events = await invoke('read_events', { name: active });
  } catch {
    return;
  }
  feedEl.innerHTML = '';
  for (const ev of events) {
    if (ev.type === 'stdin' || ev.type === 'wait' || ev.type === 'attach' || ev.type === 'detach') continue;
    const div = document.createElement('div');
    div.className = `feed-item ${ev.type}`;
    const detail =
      ev.type === 'send' ? `→ ${JSON.stringify(ev.text)} (${ev.source})`
      : ev.type === 'keys' ? `→ keys: ${(ev.keys || []).join(' ')} (${ev.source})`
      : ev.type === 'answer' ? `✔ answered ${JSON.stringify(ev.text)} by ${ev.by}`
      : ev.type === 'prompt-detected' ? `⚠ prompt [${ev.class}]: ${ev.line}`
      : ev.type === 'prompt-unanswerable' ? `⛔ ${ev.reason}`
      : ev.type === 'cancel' ? `✕ cancelled: ${ev.why}`
      : ev.type === 'start' ? `session started: ${ev.command}`
      : ev.type === 'exit' ? `exited (${ev.exitCode})`
      : ev.type === 'kill' ? `kill requested (${ev.source})`
      : JSON.stringify(ev);
    div.innerHTML = `<div class="t">${ev.ts?.slice(11, 19) ?? ''} ${ev.type}</div>${escapeHtml(detail)}`;
    feedEl.appendChild(div);
  }
}

function escapeHtml(s) {
  return s.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c]));
}

setInterval(refreshFeed, 2500);

// Blocked-on-prompt polling; attention = blocked + recent autonomous output.
// For forbid/confirm-class prompts on an attention-worthy session, pop the
// ask-human dialog (secure input for secrets, confirmation for danger words).
let askOpenFor = null;
setInterval(async () => {
  for (const [name, s] of sessions) {
    if (!s.alive) continue;
    let res;
    try {
      res = await invoke('check_prompt', { name });
    } catch {
      s.blocked = false;
      continue;
    }
    const wasAttention = s.attentionActive || false;
    s.blocked = res.reason === 'prompt';
    s.promptClass = res.promptClass;
    s.promptLine = res.promptLine;
    const attention = needsAttention(s);
    s.attentionActive = attention;
    s.tab.classList.toggle('blocked', attention);

    // OS toast on the rising edge of attention (once per prompt), so you can
    // step away from the app and still get pulled back when input is needed.
    if (attention && !wasAttention && s.notifiedLine !== res.promptLine) {
      s.notifiedLine = res.promptLine;
      const kind = res.promptClass === 'forbid' ? 'needs a secret' : res.promptClass === 'confirm' ? 'needs confirmation' : 'is waiting for input';
      invoke('notify', { title: `puppetty: "${name}" ${kind}`, body: res.promptLine || '' }).catch(() => {});
    }
    if (!attention) s.notifiedLine = null;

    if (
      name === active &&
      attention &&
      askOpenFor === null &&
      (res.promptClass === 'forbid' || res.promptClass === 'confirm') &&
      s.askedLine !== res.promptLine // don't re-pop for a line we just answered
    ) {
      openAskDialog(name, res);
    }
  }
  renderBanner();
}, 4000);

// ---------------------------------------------------------------- ask-human

const askDialog = document.getElementById('ask-dialog');
const askInput = document.getElementById('ask-input');
const askReveal = document.getElementById('ask-reveal');
const askAppendEnter = document.getElementById('ask-append-enter');
let askIsSecret = false;

// Local mirror of the engine's key-token expander (src/keyexpand.js) so a human
// response can mix plain text with keys like {esc} {tab} {up}.
const GUI_KEYMAP = {
  enter: '\r', tab: '\t', esc: '\x1b', backspace: '\x7f', del: '\x7f',
  up: '\x1b[A', down: '\x1b[B', right: '\x1b[C', left: '\x1b[D',
  home: '\x1b[H', end: '\x1b[F', pageup: '\x1b[5~', pagedown: '\x1b[6~',
  'ctrl-c': '\x03', space: ' ', cr: '\r', lf: '\n', return: '\r',
};
function expandKeys(text, appendEnter) {
  let out = '', last = 0, m;
  const re = /\{([a-z0-9-]+)\}/gi;
  while ((m = re.exec(text)) !== null) {
    out += text.slice(last, m.index);
    const seq = GUI_KEYMAP[m[1].toLowerCase()];
    out += seq != null ? seq : m[0];
    last = re.lastIndex;
  }
  out += text.slice(last);
  if (appendEnter) out += '\r';
  return out;
}

askReveal.onchange = () => { askInput.type = askReveal.checked ? 'text' : 'password'; };

// Key chips insert a {token} at the cursor (confirm prompts only).
document.querySelectorAll('#ask-keys .key-chip').forEach((chip) => {
  chip.onclick = () => {
    const tok = chip.dataset.key;
    const s = askInput.selectionStart ?? askInput.value.length;
    const e = askInput.selectionEnd ?? askInput.value.length;
    askInput.value = askInput.value.slice(0, s) + tok + askInput.value.slice(e);
    askInput.focus();
    askInput.selectionStart = askInput.selectionEnd = s + tok.length;
  };
});

// Force an explicit Send/Cancel choice — no ambiguous Escape dismissal.
askDialog.addEventListener('cancel', (e) => e.preventDefault());

function openAskDialog(name, res) {
  askOpenFor = name;
  const s = sessions.get(name);
  if (s) s.askedLine = res.promptLine; // remember so we don't re-pop for it
  const secret = res.promptClass === 'forbid';
  askIsSecret = secret;
  document.getElementById('ask-title').textContent = secret ? t('ask.title.secret') : t('ask.title.confirm');
  document.getElementById('ask-line').textContent = res.promptLine || '(prompt)';
  const note = document.getElementById('ask-note');
  note.className = `ask-note ${res.promptClass}`;
  note.textContent = secret ? t('ask.note.secret') : t('ask.note.confirm');
  // Secrets: masked, empty, no reveal-by-default. Confirm: plain text, prefilled.
  askReveal.checked = !secret;
  askInput.type = secret ? 'password' : 'text';
  askInput.value = secret ? '' : (res.promptText || 'y');
  askAppendEnter.checked = true;
  document.getElementById('ask-reveal-label').style.display = secret ? 'flex' : 'none';
  // Key tokens would corrupt a password (it may contain braces) — hide for secrets.
  document.getElementById('ask-keys').style.display = secret ? 'none' : 'flex';
  askDialog.showModal();
  askInput.focus();
  askInput.select();
}

askDialog.addEventListener('close', async () => {
  const name = askOpenFor;
  askOpenFor = null;
  const value = askInput.value;
  askInput.value = ''; // never retain a secret in the DOM
  if (!name) return;
  if (askDialog.returnValue === 'send') {
    const enter = askAppendEnter.checked;
    // Secrets are sent verbatim (never token-expanded, so braces survive);
    // confirm responses expand {key} tokens.
    const data = askIsSecret ? value + (enter ? '\r' : '') : expandKeys(value, enter);
    await invoke('write_session', { name, data }).catch(() => {});
    const s = sessions.get(name);
    if (s) s.autonomousAt = 0; // answered — clear attention
  } else {
    await invoke('write_session', { name, data: '\x03' }).catch(() => {}); // Ctrl+C
  }
});

// ---------------------------------------------------------------- boot

applyAllPrefs();

const existing = await invoke('list_sessions').catch(() => []);
let opened = 0;
for (const info of existing) {
  if (info.alive) { await openSession(info.name); opened++; }
}
// Nothing to reconnect to → start a shell so the app doesn't open to an empty
// terminal area.
if (opened === 0) await startShell();
