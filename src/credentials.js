import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { Entry } = require('@napi-rs/keyring');

// Credential store (DESIGN.md Q4). Secrets live in the OS keyring (Windows
// Credential Manager / macOS Keychain / libsecret), never on disk in
// plaintext and never in any puppetty log. Only a *ref* (name) is ever
// stored by us, logged, or seen by a decider/agent — the secret is fetched
// at the last moment and written straight into the PTY.
//
// The keyring can't enumerate our entries, so we keep a names-only registry.

const SERVICE = 'puppetty';

function registryPath() {
  const dir = path.join(os.homedir(), '.puppetty');
  fs.mkdirSync(dir, { recursive: true });
  return path.join(dir, 'credentials.json');
}

function readRegistry() {
  try {
    const data = JSON.parse(fs.readFileSync(registryPath(), 'utf8'));
    return Array.isArray(data.refs) ? data.refs : [];
  } catch {
    return [];
  }
}

function writeRegistry(refs) {
  fs.writeFileSync(registryPath(), JSON.stringify({ refs: [...new Set(refs)].sort() }, null, 2));
}

export function listRefs() {
  // Reconcile: drop registry entries whose secret is gone from the keyring.
  const refs = readRegistry().filter((ref) => {
    try {
      return new Entry(SERVICE, ref).getPassword() != null;
    } catch {
      return false;
    }
  });
  if (refs.length !== readRegistry().length) writeRegistry(refs);
  return refs;
}

export function setCredential(ref, secret) {
  if (!ref || !/^[\w.-]+$/.test(ref)) throw new Error('ref must match [A-Za-z0-9_.-]+');
  new Entry(SERVICE, ref).setPassword(secret);
  writeRegistry([...readRegistry(), ref]);
}

// Returns the secret string, or null if the ref is unknown. Callers MUST NOT
// log the result and should write it directly to the PTY.
export function getCredential(ref) {
  try {
    return new Entry(SERVICE, ref).getPassword();
  } catch {
    return null;
  }
}

export function deleteCredential(ref) {
  let existed = false;
  try {
    existed = new Entry(SERVICE, ref).deletePassword();
  } catch {}
  writeRegistry(readRegistry().filter((r) => r !== ref));
  return existed;
}
