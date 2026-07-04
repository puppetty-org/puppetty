#!/usr/bin/env node
// Thin launcher: npm only delivers the platform binary (the engine is Rust).
// Resolution: PUPPETTY_ENGINE env var → repo dev build → platform package.
import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import fs from 'node:fs';
import path from 'node:path';

const exe = process.platform === 'win32' ? 'puppetty-engine.exe' : 'puppetty-engine';

function resolveEngine() {
  if (process.env.PUPPETTY_ENGINE) return process.env.PUPPETTY_ENGINE;
  const repo = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
  for (const profile of ['release', 'debug']) {
    const dev = path.join(repo, 'engine-rs', 'target', profile, exe);
    if (fs.existsSync(dev)) return dev;
  }
  const pkg = {
    'win32-x64': '@puppetty/win32-x64',
    'linux-x64': '@puppetty/linux-x64-gnu',
    'linux-arm64': '@puppetty/linux-arm64-gnu',
    'darwin-x64': '@puppetty/darwin-x64',
    'darwin-arm64': '@puppetty/darwin-arm64',
  }[`${process.platform}-${process.arch}`];
  try {
    const require = createRequire(import.meta.url);
    return path.join(path.dirname(require.resolve(`${pkg}/package.json`)), exe);
  } catch {
    console.error(
      `puppetty: no engine binary for ${process.platform}-${process.arch}` +
        (pkg ? ` (optional dependency ${pkg} did not install)` : ' (unsupported platform)')
    );
    process.exit(1);
  }
}

const result = spawnSync(resolveEngine(), process.argv.slice(2), { stdio: 'inherit' });
process.exit(result.status ?? 1);
