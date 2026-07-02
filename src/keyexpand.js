import { KEYMAP } from './session.js';

// A few friendly aliases on top of KEYMAP (enter/tab/esc/arrows/…).
const EXTRA = { space: ' ', cr: '\r', lf: '\n', return: '\r', del: '\x7f' };

// Expand `{key}` tokens (case-insensitive) inside `text` into their control
// bytes — e.g. "y{enter}", "{down}{down}{enter}", "value{tab}more". Unknown
// tokens are left literal. Optionally append Enter at the very end.
export function expandInput(text, { enter = false } = {}) {
  let out = '';
  let last = 0;
  const re = /\{([a-z0-9-]+)\}/gi;
  let m;
  while ((m = re.exec(text ?? '')) !== null) {
    out += text.slice(last, m.index);
    const name = m[1].toLowerCase();
    const seq = KEYMAP[name] ?? EXTRA[name];
    out += seq != null ? seq : m[0]; // keep unknown tokens as typed
    last = re.lastIndex;
  }
  out += (text ?? '').slice(last);
  if (enter) out += '\r';
  return out;
}
