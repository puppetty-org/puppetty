import { evaluate, isPromptish } from './policy.js';
import { askDecider, askCredentialChoice } from './decider.js';
import { getCredential, listRefs } from './credentials.js';
import { expandInput } from './keyexpand.js';

const POLL_MS = 250;

// Optional prompt-answering layer on top of a Session, driven by the loaded
// policy (DESIGN.md Q2/Q9): class `auto` answers directly; `confirm` and
// `forbid` are never answered headless — they fall through to onUnanswered
// (a GUI will route them to ask-human instead). Unmatched prompt-looking
// lines go to the decider if one is configured.
export function attachAutopilot(session, opts) {
  const { policy, quietMs = 700, decider = null, log } = opts;
  const promptTimeout = opts.promptTimeout ?? policy.onUnanswered.afterSec;

  let lastData = Date.now();
  let cancelled = false;
  let handledState = null;
  let unansweredSince = null;
  let deciderBusy = false;
  const answerCounts = new Map();

  const notifyData = () => {
    lastData = Date.now();
    unansweredSince = null;
  };

  const answer = (text, why) => {
    log(`auto-answer (${why}): ${JSON.stringify(text)}`);
    session.logger?.event('answer', { text, by: why, source: 'autopilot' });
    session.write(text);
  };

  const cancel = (why) => {
    cancelled = true;
    log(`cancelling child (${why})`);
    session.logger?.event('cancel', { why, source: 'autopilot' });
    session.write('\x03');
    setTimeout(() => { if (!session.exited) session.kill(); }, 5_000).unref?.();
  };

  const markUnanswered = (reason) => {
    if (unansweredSince === null) {
      unansweredSince = Date.now();
      log(`waiting for input I won't answer (${reason}); ${policy.onUnanswered.do} in ${promptTimeout}s unless output resumes`);
      session.logger?.event('prompt-unanswerable', { reason });
    } else if (Date.now() - unansweredSince > promptTimeout * 1_000) {
      unansweredSince = null;
      if (policy.onUnanswered.do === 'cancel') cancel(reason);
    }
  };

  const poller = setInterval(async () => {
    if (session.exited || deciderBusy) return;
    if (Date.now() - lastData < quietMs) return;

    const snap = await session.screen.snapshot();
    const screenText = snap.lines.join('\n');
    const line = [...snap.lines].reverse().find((l) => l.trim().length > 0)?.trim() || '';
    if (!line) return;

    const stateKey = screenText.slice(-500);
    if (stateKey === handledState) {
      if (unansweredSince !== null) markUnanswered('prompt still pending');
      return;
    }

    const match = evaluate(policy, line, screenText);

    if (match && match.class === 'auto' && (match.rule.action === 'send' || match.rule.action === 'enter')) {
      const count = (answerCounts.get(line) || 0) + 1;
      answerCounts.set(line, count);
      if (count > 3) {
        cancel(`answered the same prompt ${count - 1} times — giving up`);
        return;
      }
      handledState = stateKey;
      // Text may embed {key} tokens; append Enter unless the rule opts out.
      answer(
        match.rule.action === 'enter'
          ? '\r'
          : expandInput(match.rule.text, { enter: match.rule.enter !== false }),
        `rule:${match.rule.name}`
      );
      return;
    }

    if (match && match.class === 'credential') {
      handledState = stateKey;
      let ref = match.rule.ref;
      // AI-decided credential: no fixed ref — ask the decider which stored
      // credential fits. The decider only ever sees ref *names*, never secrets.
      if (match.rule.ai && !ref) {
        const refs = listRefs();
        const deciderCmd = decider ?? (match.rule.decider ? policy.deciders[match.rule.decider]?.command : null);
        if (!deciderCmd || refs.length === 0) {
          session.logger?.event('prompt-detected', { line: line.slice(0, 200), class: 'credential', rule: match.rule.name });
          markUnanswered(`rule:${match.rule.name} (AI credential needs a --decider and stored credentials)`);
          return;
        }
        deciderBusy = true;
        log(`asking decider to pick a credential from: ${refs.join(', ')}`);
        session.logger?.event('decider-asked', { line: line.slice(0, 200), refs });
        const verdict = await askCredentialChoice(deciderCmd, screenText.slice(-2_000), refs);
        deciderBusy = false;
        if (session.exited) return;
        session.logger?.event('decider-said', { verdict: verdict.raw.slice(0, 200) });
        if (verdict.type === 'CRED' && refs.includes(verdict.ref)) {
          ref = verdict.ref;
        } else if (verdict.type === 'CANCEL') {
          cancel(`decider declined to provide a credential`);
          return;
        } else {
          markUnanswered(`rule:${match.rule.name} (AI did not pick a valid credential)`);
          return;
        }
      }
      const secret = ref ? getCredential(ref) : null;
      if (secret == null) {
        session.logger?.event('prompt-detected', { line: line.slice(0, 200), class: 'credential', rule: match.rule.name, ref });
        markUnanswered(`rule:${match.rule.name} (credential "${ref}" not found in store)`);
        return;
      }
      // Log the ref, NEVER the secret; write it straight to the PTY.
      log(`auto-answer (rule:${match.rule.name}): <credential:${ref}>`);
      session.logger?.event('answer', { by: `credential:${ref}`, source: 'autopilot', redacted: true });
      session.write(secret + '\r');
      return;
    }

    if (match && (match.class === 'forbid' || match.class === 'confirm')) {
      handledState = stateKey;
      session.logger?.event('prompt-detected', { line: line.slice(0, 200), class: match.class, rule: match.rule.name });
      markUnanswered(
        match.class === 'forbid'
          ? `rule:${match.rule.name} (class forbid — never automated)`
          : `rule:${match.rule.name} (class confirm — needs a human; no GUI attached)`
      );
      return;
    }

    if (!isPromptish(line)) {
      handledState = stateKey;
      return;
    }

    const deciderCmd =
      decider ??
      (match?.rule.action === 'decider' ? policy.deciders[match.rule.decider]?.command : null);

    if (deciderCmd) {
      deciderBusy = true;
      handledState = stateKey;
      log(`asking decider about: ${JSON.stringify(line.slice(0, 80))}`);
      session.logger?.event('decider-asked', { line: line.slice(0, 200) });
      const verdict = await askDecider(deciderCmd, screenText.slice(-2_000));
      deciderBusy = false;
      if (session.exited) return;
      log(`decider said: ${verdict.raw}`);
      session.logger?.event('decider-said', { verdict: verdict.raw.slice(0, 200) });
      switch (verdict.type) {
        case 'SEND': answer(verdict.text + '\r', 'decider'); break;
        case 'ENTER': answer('\r', 'decider'); break;
        case 'CANCEL': cancel('decider said CANCEL'); break;
        default: markUnanswered('decider said WAIT');
      }
      return;
    }

    handledState = stateKey;
    session.logger?.event('prompt-detected', { line: line.slice(0, 200), class: 'unmatched' });
    markUnanswered('unrecognized prompt and no decider configured');
  }, POLL_MS);

  return {
    notifyData,
    stop: () => clearInterval(poller),
    get cancelled() { return cancelled; },
  };
}
