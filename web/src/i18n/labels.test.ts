// web/src/i18n/labels.test.ts
//
// Verifies that every Role / InputMode variant maps to a real
// dictionary entry in both shipped locales. Catches three classes of
// regression at CI time:
//   1. A new variant added to `Role` / `InputMode` without a matching
//      label entry — the lookup table in `labels.ts` would already be a
//      tsc error, but this test re-asserts at runtime against the dict.
//   2. The `roles.*` / `input_mode.*` group accidentally renamed in one
//      locale only — `dict-symmetry.test.ts` covers this for all keys,
//      but a focused test makes the failure point obvious.
//   3. Empty/whitespace label values, which would render as a blank
//      badge in the UI.

import { describe, expect, it } from 'vitest';
import * as i18n from '@solid-primitives/i18n';

import { en } from './locales/en';
import { zh } from './locales/zh';
import { inputModeLabel, roleLabel } from './labels';
import type { TranslationKey, Translator } from './provider';
import type { InputMode, Role } from '../lib/protocol';

const ROLES: Role[] = ['owner', 'operator', 'viewer'];
const INPUT_MODES: InputMode[] = ['multiplexed', 'serialized'];

function translatorFor(dict: typeof en): Translator {
  const flat = i18n.flatten(dict);
  const raw = i18n.translator(() => flat, i18n.resolveTemplate);
  return (key: TranslationKey, params) => {
    const result = raw(key, params);
    return typeof result === 'string' ? result : key;
  };
}

describe('roleLabel', () => {
  for (const locale of [
    { name: 'en', dict: en },
    { name: 'zh', dict: zh },
  ] as const) {
    describe(`locale=${locale.name}`, () => {
      const t = translatorFor(locale.dict);
      for (const role of ROLES) {
        it(`returns a non-empty label for "${role}"`, () => {
          const label = roleLabel(t, role);
          expect(label).toBeTruthy();
          expect(label.trim()).not.toBe('');
          // Must not leak the raw key — that would mean the dict entry
          // is missing and the translator fell back to returning the
          // key verbatim.
          expect(label).not.toBe(`roles.${role}`);
        });
      }
    });
  }

  it('uses Chinese strings under the zh locale', () => {
    const t = translatorFor(zh);
    expect(roleLabel(t, 'owner')).toBe('所有者');
    expect(roleLabel(t, 'operator')).toBe('操作者');
    expect(roleLabel(t, 'viewer')).toBe('观察者');
  });
});

describe('inputModeLabel', () => {
  for (const locale of [
    { name: 'en', dict: en },
    { name: 'zh', dict: zh },
  ] as const) {
    describe(`locale=${locale.name}`, () => {
      const t = translatorFor(locale.dict);
      for (const mode of INPUT_MODES) {
        it(`returns a non-empty label for "${mode}"`, () => {
          const label = inputModeLabel(t, mode);
          expect(label).toBeTruthy();
          expect(label.trim()).not.toBe('');
          expect(label).not.toBe(`input_mode.${mode}`);
        });
      }
    });
  }

  it('uses Chinese strings under the zh locale', () => {
    const t = translatorFor(zh);
    expect(inputModeLabel(t, 'multiplexed')).toBe('协作');
    expect(inputModeLabel(t, 'serialized')).toBe('独占');
  });
});
