import { spawn } from 'node:child_process';

// Ask an external command (a script, or an LLM CLI like `claude -p`) what to do
// about a suspected prompt. The context is written to the command's stdin; the
// command must print exactly one directive on its first stdout line:
//
//   SEND:<text>   type <text> and press Enter
//   ENTER         just press Enter
//   CANCEL        abort the child process (Ctrl+C)
//   WAIT          not a prompt / still working — do nothing
//
// Anything else is treated as WAIT.

const INSTRUCTIONS = `You are supervising a terminal program that appears to be waiting for input.
Below is the tail of its output. Decide how to respond.
Reply with EXACTLY ONE line and nothing else, in one of these forms:
SEND:<text>   (type <text> and press Enter — e.g. SEND:y or SEND:my-project)
ENTER         (just press Enter)
CANCEL        (abort the program; use for password prompts or anything unsafe)
WAIT          (it is not actually waiting for input)

--- terminal output tail ---
`;

// Credential-choice mode: the decider is shown the available credential *names*
// (never the secret values) and picks which one fits the prompt.
const CRED_INSTRUCTIONS = (refs) => `You are supervising a terminal program that is asking for a credential (a password, passphrase, or token).
Below is the tail of its output. Choose which stored credential should be used.
Available credentials — NAMES ONLY, you never see their values: ${refs.join(', ')}
Reply with EXACTLY ONE line and nothing else:
CRED:<name>   (use this credential — <name> MUST be one of the list above)
CANCEL        (do not provide any credential; use if none clearly fits or it's unsafe)

--- terminal output tail ---
`;

function runDecider(deciderCmd, input, timeoutMs, parse) {
  return new Promise((resolve) => {
    const child = spawn(deciderCmd, {
      shell: true,
      stdio: ['pipe', 'pipe', 'inherit'],
      windowsHide: true,
    });

    let out = '';
    let done = false;
    const finish = (verdict) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      try { child.kill(); } catch {}
      resolve(verdict);
    };

    const timer = setTimeout(() => finish({ type: 'WAIT', raw: '(decider timeout)' }), timeoutMs);

    child.stdout.on('data', (d) => { out += d.toString(); });
    child.on('error', (err) => finish({ type: 'WAIT', raw: `(decider error: ${err.message})` }));
    child.on('close', () => finish(parse(out)));

    child.stdin.write(input);
    child.stdin.end();
  });
}

export function askDecider(deciderCmd, tail, timeoutMs = 60_000) {
  return runDecider(deciderCmd, INSTRUCTIONS + tail + '\n', timeoutMs, parseVerdict);
}

export function askCredentialChoice(deciderCmd, tail, refs, timeoutMs = 60_000) {
  return runDecider(deciderCmd, CRED_INSTRUCTIONS(refs) + tail + '\n', timeoutMs, parseCredVerdict);
}

export function parseCredVerdict(output) {
  const line = output
    .split('\n')
    .map((l) => l.trim())
    .find((l) => /^(CRED:|CANCEL$)/.test(l));
  if (!line) return { type: 'WAIT', raw: output.trim().slice(0, 200) };
  if (line.startsWith('CRED:')) return { type: 'CRED', ref: line.slice(5).trim(), raw: line };
  return { type: 'CANCEL', raw: line };
}

export function parseVerdict(output) {
  const line = output
    .split('\n')
    .map((l) => l.trim())
    .find((l) => /^(SEND:|ENTER$|CANCEL$|WAIT$)/.test(l));
  if (!line) return { type: 'WAIT', raw: output.trim().slice(0, 200) };
  if (line.startsWith('SEND:')) return { type: 'SEND', text: line.slice(5), raw: line };
  return { type: line, raw: line };
}
