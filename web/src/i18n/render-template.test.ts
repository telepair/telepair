// web/src/i18n/render-template.test.ts
//
// `renderTemplate` is the only place in the i18n module that
// concatenates literal text and JSX, so the segment ordering and the
// "unknown slot stays literal" behaviour matter for production output.

import { describe, it, expect } from 'vitest';
import { renderTemplate } from './render-template';

describe('renderTemplate', () => {
  it('returns the template unchanged when there are no slots', () => {
    expect(renderTemplate('hello world', {})).toEqual(['hello world']);
  });

  it('returns an empty array for an empty template', () => {
    expect(renderTemplate('', {})).toEqual([]);
  });

  it('inserts a JSX node where a known slot appears', () => {
    const node = { type: 'code' } as unknown as Element;
    const out = renderTemplate('saved to {{ path }}.', { path: node });
    expect(out).toEqual(['saved to ', node, '.']);
  });

  it('handles multiple distinct slots in a single template', () => {
    const a = { id: 'a' } as unknown as Element;
    const b = { id: 'b' } as unknown as Element;
    const out = renderTemplate('{{ x }} then {{ y }}', { x: a, y: b });
    expect(out).toEqual([a, ' then ', b]);
  });

  it('preserves the literal slot text when the slot key is missing', () => {
    // A typo in either the dictionary or the call site should be
    // visible in the rendered UI rather than silently dropping the
    // slot. Asserting against the verbatim `{{ ghost }}` keeps that
    // contract honest.
    const out = renderTemplate('a {{ ghost }} b', {});
    expect(out).toEqual(['a ', '{{ ghost }}', ' b']);
  });

  it('treats whitespace inside the slot delimiters as optional', () => {
    const node = { id: 'tight' } as unknown as Element;
    const out = renderTemplate('a{{path}}b', { path: node });
    expect(out).toEqual(['a', node, 'b']);
  });

  it('does not match malformed slots that contain non-word characters', () => {
    // `{{ path-name }}` is not a valid slot — the regex requires \w+
    // (word characters only). It should be left in the literal text
    // unchanged so the failure is obvious to the author.
    const out = renderTemplate('a {{ bad-name }} b', {});
    expect(out).toEqual(['a {{ bad-name }} b']);
  });
});
