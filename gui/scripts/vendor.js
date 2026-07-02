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
];
for (const [src, dst] of files) {
  fs.copyFileSync(path.join(root, src), path.join(vendor, dst));
  console.log(`vendored ${dst}`);
}
