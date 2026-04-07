// web/src/i18n/render-template.tsx
//
// Helper for splicing JSX values into a translated template string.
//
// Why this exists: some help copy needs literal `<code>` markers around
// file paths and CLI commands. Putting raw HTML in a translated string
// would defeat the purpose of i18n (each locale would need to repeat
// the markup) AND force consumers to use `innerHTML`, which is an XSS
// vector. Instead, the dictionary keeps the sentence with named slots
// like "{{ path }}", and this helper walks the template, inserting
// real JSX nodes where the slots are.
//
// The helper deliberately accepts only `JSX.Element` for slot values —
// not strings — because string substitution is already handled by the
// translator's own `resolveTemplate`. The two systems are
// complementary: use `t('key', { name: 'Alice' })` for plain text,
// and use `renderTemplate(t('key'), { node: <code>x</code> })` when
// you need a real DOM node.

import type { JSX } from 'solid-js';

/** Match `{{ key }}` slots — whitespace optional, key is `\w+`.
 *  Capturing group is the key name so `split` returns alternating
 *  literal/key segments. */
const SLOT_RE = /\{\{\s*(\w+)\s*\}\}/g;

/** Splice JSX nodes into a translated template at named slots.
 *
 *  Example:
 *    renderTemplate(t('login.help_first_run'), {
 *      path: <code>~/.telepair/admin_token</code>,
 *    })
 *  with template "First run? telepair saves it to {{ path }}." returns
 *  the JSX array
 *    ['First run? telepair saves it to ', <code>…</code>, '.']
 *
 *  Behaviour notes:
 *  - Unknown slots (no matching key in `slots`) are kept as-is, so a
 *    typo in either the dictionary or the call site is visible in the
 *    rendered UI rather than silently dropped.
 *  - The function returns an array of `JSX.Element | string` segments;
 *    Solid renders that as siblings without needing a fragment.
 */
export function renderTemplate(
  template: string,
  slots: Record<string, JSX.Element>,
): Array<JSX.Element | string> {
  const out: Array<JSX.Element | string> = [];
  let lastIndex = 0;
  for (const match of template.matchAll(SLOT_RE)) {
    const [literal, key] = [match[0], match[1]];
    const start = match.index ?? 0;
    if (start > lastIndex) {
      out.push(template.slice(lastIndex, start));
    }
    if (key in slots) {
      out.push(slots[key]);
    } else {
      // Surface the missing slot verbatim — easier to spot in QA than
      // a silent gap in the sentence.
      out.push(literal);
    }
    lastIndex = start + literal.length;
  }
  if (lastIndex < template.length) {
    out.push(template.slice(lastIndex));
  }
  return out;
}
