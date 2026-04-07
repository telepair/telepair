// web/src/i18n/dict-symmetry.test.ts
//
// Structural symmetry guard for the en/zh dictionaries. The TS type
// `Dict = typeof en` already enforces "every English key exists in
// Chinese", but it can't catch:
//
//   1. **Placeholder drift** — if `en.invite.usable_singular` says
//      `{{ n }}` and the Chinese translation says `{{ count }}`, both
//      strings type-check but the rendered Chinese drops the number.
//
//   2. **Empty translations** — `zh.foo: ''` would slip past the type
//      check (empty string satisfies `string`) and render as a blank
//      label in production.
//
// This test catches both at CI time, before they reach the user.

import { describe, it, expect } from 'vitest';
import { en } from './locales/en';
import { zh } from './locales/zh';

type LeafEntries = Array<[string, string]>;

/** Walk a nested dictionary and yield `[dotted.key, value]` pairs.
 *  Mirrors what `i18n.flatten` does at runtime, but without pulling
 *  in the library — keeping the test isolated to its actual concern. */
function flatten(obj: Record<string, unknown>, prefix = ''): LeafEntries {
  const out: LeafEntries = [];
  for (const [key, value] of Object.entries(obj)) {
    const dotted = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === 'object') {
      out.push(...flatten(value as Record<string, unknown>, dotted));
    } else if (typeof value === 'string') {
      out.push([dotted, value]);
    }
  }
  return out;
}

const SLOT_RE = /\{\{\s*(\w+)\s*\}\}/g;

function placeholders(s: string): string[] {
  return Array.from(s.matchAll(SLOT_RE), (m) => m[1]).sort();
}

const enEntries = flatten(en as unknown as Record<string, unknown>);
const zhEntries = flatten(zh as unknown as Record<string, unknown>);
const zhMap = new Map(zhEntries);

describe('dictionary symmetry', () => {
  it('every English key has a Chinese translation', () => {
    const missing = enEntries
      .map(([k]) => k)
      .filter((k) => !zhMap.has(k));
    expect(missing).toEqual([]);
  });

  it('every Chinese key exists in English (no orphan translations)', () => {
    const enKeys = new Set(enEntries.map(([k]) => k));
    const orphans = zhEntries.map(([k]) => k).filter((k) => !enKeys.has(k));
    expect(orphans).toEqual([]);
  });

  it('no Chinese translation is empty', () => {
    const empties = zhEntries.filter(([, v]) => v.trim() === '').map(([k]) => k);
    expect(empties).toEqual([]);
  });

  it('placeholders match between English and Chinese for every key', () => {
    const mismatches: Array<{ key: string; en: string[]; zh: string[] }> = [];
    for (const [key, enValue] of enEntries) {
      const zhValue = zhMap.get(key);
      if (zhValue === undefined) continue; // covered by previous test
      const enSlots = placeholders(enValue);
      const zhSlots = placeholders(zhValue);
      if (enSlots.join('|') !== zhSlots.join('|')) {
        mismatches.push({ key, en: enSlots, zh: zhSlots });
      }
    }
    expect(mismatches).toEqual([]);
  });
});
