import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';

// Prompt policy: which prompts get answered, by whom, and what happens when
// nobody may answer. Layered JSONC config (DESIGN.md Q1/Q2):
//   defaults (below)  <  ~/.puppetty/config.json  <  <cwd>/.puppetty/config.json
//
// Rule fields: { name, match, flags?, action, text?, class?, disabled? }
//   action: send | enter | forbid | decider
//   class:  auto | confirm | forbid   (default: auto; forbid action => forbid)
// First matching rule wins; a rule earlier in the layering order shadows a
// later rule with the same name (set disabled:true to tombstone a default).
// Danger words escalate any non-forbid match to `confirm` (Q2).

export function isPromptish(line) {
  // Progress/status output is not a prompt even though it often ends in ')'
  // or a digit: git "Writing objects: 15% (353/2346)", download bars, "(3/10)",
  // "12.3 MiB | 4.5 MiB/s". Excluding it stops the autopilot from flagging
  // ordinary command output as an unanswered prompt (a real "N/M" prompt like
  // "Apply 3/5? [y/n]" is still caught by a rule before isPromptish is used).
  if (/\d\s*%|\(\d+\/\d+\)|\bMiB\b|\bKiB\b|\bGiB\b/.test(line)) return false;
  return /[:?>\])]\s*$/.test(line) || line.includes('?');
}

const DEFAULT_POLICY = {
  rules: [
    {
      name: 'secrets',
      match: '(password|passphrase|passcode|secret|api[ _-]?key|token)\\s*[:：]?\\s*$',
      flags: 'i',
      action: 'forbid',
    },
    {
      name: 'yes-no-bracket',
      match: '[\\[(](y/n|yes/no|y/n/a)[\\])]\\s*[:：?？]?\\s*$',
      flags: 'i',
      action: 'send',
      text: 'y',
    },
    {
      name: 'yes-no-default',
      match: '[\\[(](y/N|Y/n|yes/NO|YES/no)[\\])]\\s*[:：?？]?\\s*$',
      action: 'send',
      text: 'y',
    },
    {
      name: 'continue-question',
      match: '\\b(continue|proceed|install|ok to proceed)\\s*\\??\\s*$',
      flags: 'i',
      action: 'send',
      text: 'y',
    },
    {
      name: 'press-enter',
      match: 'press\\s+(enter|return|any key)',
      flags: 'i',
      action: 'enter',
    },
    {
      name: 'confirm-word',
      match: 'type\\s+[\'"]?(y|yes)[\'"]?\\s+to\\s+(confirm|continue|proceed)',
      flags: 'i',
      action: 'send',
      text: 'yes',
      class: 'confirm',
    },
  ],
  dangerWords: [
    '\\bdelete\\b', '\\bremove\\b', '\\boverwrite\\b', '\\bforce\\b',
    'rm -rf', 'reset --hard', 'git push', 'irreversible',
    'cannot be undone', '\\bpermanently\\b',
  ],
  onUnanswered: { afterSec: 30, do: 'cancel' },
  deciders: {},
  logging: { enabled: true, retentionDays: 30, maxTotalMB: 200 },
};

// String-aware JSONC: strips // and /* */ comments and trailing commas.
export function parseJsonc(text) {
  let out = '';
  let inStr = false;
  let esc = false;
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
    if (c === '/' && text[i + 1] === '/') {
      while (i < text.length && text[i] !== '\n') i++;
      out += '\n';
      continue;
    }
    if (c === '/' && text[i + 1] === '*') {
      i += 2;
      while (i < text.length && !(text[i] === '*' && text[i + 1] === '/')) i++;
      i++;
      continue;
    }
    if (c === ',') {
      // trailing comma? look ahead past whitespace
      let j = i + 1;
      while (j < text.length && /\s/.test(text[j])) j++;
      if (text[j] === '}' || text[j] === ']') continue;
    }
    out += c;
  }
  return JSON.parse(out);
}

function readConfigFile(file) {
  if (!fs.existsSync(file)) return null;
  try {
    return parseJsonc(fs.readFileSync(file, 'utf8'));
  } catch (err) {
    throw new Error(`invalid config ${file}: ${err.message}`);
  }
}

function dedupeByName(rules) {
  const seen = new Set();
  return rules.filter((r) => {
    const key = r.name ?? r.match;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

export function userConfigPath() {
  return path.join(os.homedir(), '.puppetty', 'config.json');
}

export function loadPolicy(cwd = process.cwd()) {
  const user = readConfigFile(userConfigPath());
  const project = readConfigFile(path.join(cwd, '.puppetty', 'config.json'));

  const policy = {
    rules: dedupeByName([
      ...(project?.rules ?? []),
      ...(user?.rules ?? []),
      ...DEFAULT_POLICY.rules,
    ]),
    dangerWords: project?.dangerWords ?? user?.dangerWords ?? DEFAULT_POLICY.dangerWords,
    onUnanswered: { ...DEFAULT_POLICY.onUnanswered, ...user?.onUnanswered, ...project?.onUnanswered },
    deciders: { ...DEFAULT_POLICY.deciders, ...user?.deciders, ...project?.deciders },
    logging: { ...DEFAULT_POLICY.logging, ...user?.logging, ...project?.logging },
    sources: { user: !!user, project: !!project },
  };

  policy.compiled = policy.rules
    .filter((r) => !r.disabled)
    .map((r) => {
      try {
        return { ...r, regex: new RegExp(r.match, r.flags ?? '') };
      } catch (err) {
        throw new Error(`invalid rule "${r.name ?? r.match}": ${err.message}`);
      }
    });
  policy.dangerRe = policy.dangerWords.length
    ? new RegExp(policy.dangerWords.join('|'), 'i')
    : null;
  return policy;
}

// -> { rule, class } for the first matching rule, or null.
// class: 'auto' may be answered by rules; 'confirm' needs a human (headless:
// falls through to onUnanswered); 'forbid' is never automated; 'credential'
// is answered from the OS keyring by ref (the secret is never logged).
//
// By default a rule matches against the prompt `line` (the last non-empty line).
// A rule with `scope: 'screen'` matches against the whole visible `screen`
// instead — needed for multi-line TUI prompts (e.g. an agent's boxed menu)
// whose distinctive text isn't on the last line. The danger-word check always
// scans the whole visible screen (not just the matched target), so any auto
// match escalates to confirm when a danger word is visible anywhere.
export function evaluate(policy, line, screen) {
  const full = screen ?? line;
  for (const rule of policy.compiled) {
    const target = rule.scope === 'screen' ? full : line;
    if (!rule.regex.test(target)) continue;
    let cls;
    if (rule.action === 'credential') cls = 'credential';
    else cls = rule.class ?? (rule.action === 'forbid' ? 'forbid' : 'auto');
    // Danger words escalate an automatable class to human confirmation, but
    // never override an explicit forbid/credential. Always scan the whole
    // visible screen, not just the matched line: command-approval prompts show
    // the risky command on a different line than the "press enter" footer a
    // line-scoped rule matches, so a line check alone would miss it.
    if (cls === 'auto' && policy.dangerRe?.test(full)) cls = 'confirm';
    return { rule, class: cls };
  }
  return null;
}
