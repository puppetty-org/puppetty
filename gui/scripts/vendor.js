// Copies xterm.js runtime files from node_modules into ui/vendor so the
// Tauri webview can load them without a bundler.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const vendor = path.join(root, 'ui', 'vendor');
fs.mkdirSync(vendor, { recursive: true });

const files = [
  ['node_modules/@xterm/xterm/lib/xterm.js', 'xterm.js'],
  ['node_modules/@xterm/xterm/css/xterm.css', 'xterm.css'],
  ['node_modules/@xterm/addon-fit/lib/addon-fit.js', 'addon-fit.js'],
  // KaTeX for the math-overlay feature; its CSS references fonts/ relatively,
  // so the fonts directory is copied alongside below.
  ['node_modules/katex/dist/katex.min.js', 'katex/katex.min.js'],
  ['node_modules/katex/dist/katex.min.css', 'katex/katex.min.css'],
];
for (const [src, dst] of files) {
  fs.mkdirSync(path.dirname(path.join(vendor, dst)), { recursive: true });
  fs.copyFileSync(path.join(root, src), path.join(vendor, dst));
  console.log(`vendored ${dst}`);
}
// woff2 only: every supported webview (WebView2, WKWebView, WebKitGTK) loads
// woff2 first, so the ttf/woff fallbacks would be dead weight in the repo.
fs.cpSync(
  path.join(root, 'node_modules/katex/dist/fonts'),
  path.join(vendor, 'katex/fonts'),
  { recursive: true, filter: (src) => !/\.(ttf|woff)$/.test(src) }
);
console.log('vendored katex/fonts (woff2)');

// Build the Rust engine and place it as the Tauri sidecar (externalBin needs
// a target-triple suffixed file). Skip with PUPPETTY_SKIP_ENGINE=1; CI uses
// PUPPETTY_ENGINE_PROFILE=debug for a faster compile-only check.
if (!process.env.PUPPETTY_SKIP_ENGINE) {
  const { execSync } = await import('node:child_process');
  const repo = path.dirname(root);
  const profile = process.env.PUPPETTY_ENGINE_PROFILE === 'debug' ? 'debug' : 'release';
  const flag = profile === 'release' ? ' --release' : '';
  execSync(`cargo build${flag}`, { cwd: path.join(repo, 'engine-rs'), stdio: 'inherit' });
  const triple = execSync('rustc -vV').toString().match(/host: (\S+)/)[1];
  const ext = process.platform === 'win32' ? '.exe' : '';
  const binDir = path.join(root, 'src-tauri', 'binaries');
  fs.mkdirSync(binDir, { recursive: true });
  fs.copyFileSync(
    path.join(repo, 'engine-rs', 'target', profile, `puppetty-engine${ext}`),
    path.join(binDir, `puppetty-engine-${triple}${ext}`)
  );
  console.log(`sidecar: puppetty-engine-${triple}${ext} (${profile})`);
}
