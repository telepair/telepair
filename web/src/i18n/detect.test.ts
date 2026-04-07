// web/src/i18n/detect.test.ts
//
// Coverage for the locale-detection precedence rules. The provider
// only ever calls `detectInitialLocale` once at boot, so a regression
// here means every visitor lands on the wrong language. Worth pinning
// the full decision tree.

import { describe, it, expect, beforeEach, vi } from 'vitest';

const store: Record<string, string> = {};
vi.stubGlobal('localStorage', {
  getItem: (key: string) => store[key] ?? null,
  setItem: (key: string, value: string) => {
    store[key] = value;
  },
  removeItem: (key: string) => {
    delete store[key];
  },
});

const { detectInitialLocale, persistLocale } = await import('./detect');
const { STORAGE_KEY } = await import('./types');

beforeEach(() => {
  for (const key of Object.keys(store)) delete store[key];
  // jsdom defaults `navigator.language` to 'en-US'; tests below
  // override it explicitly when they need a different value.
  Object.defineProperty(navigator, 'language', {
    value: 'en-US',
    configurable: true,
  });
});

describe('detectInitialLocale', () => {
  it('returns the persisted choice when localStorage has a valid locale', () => {
    store[STORAGE_KEY] = 'zh';
    expect(detectInitialLocale()).toBe('zh');
  });

  it('ignores invalid persisted values and falls through to browser detection', () => {
    store[STORAGE_KEY] = 'klingon';
    Object.defineProperty(navigator, 'language', {
      value: 'zh-CN',
      configurable: true,
    });
    expect(detectInitialLocale()).toBe('zh');
  });

  it('uses navigator.language for any zh-* locale', () => {
    Object.defineProperty(navigator, 'language', {
      value: 'zh-TW',
      configurable: true,
    });
    expect(detectInitialLocale()).toBe('zh');
  });

  it('treats zh prefix case-insensitively', () => {
    Object.defineProperty(navigator, 'language', {
      value: 'ZH-Hant',
      configurable: true,
    });
    expect(detectInitialLocale()).toBe('zh');
  });

  it('falls back to English for non-Chinese browser locales', () => {
    Object.defineProperty(navigator, 'language', {
      value: 'fr-FR',
      configurable: true,
    });
    expect(detectInitialLocale()).toBe('en');
  });

  it('persisted choice wins over browser locale (user override)', () => {
    store[STORAGE_KEY] = 'en';
    Object.defineProperty(navigator, 'language', {
      value: 'zh-CN',
      configurable: true,
    });
    expect(detectInitialLocale()).toBe('en');
  });
});

describe('persistLocale', () => {
  it('writes to localStorage so subsequent visits inherit the choice', () => {
    persistLocale('zh');
    expect(store[STORAGE_KEY]).toBe('zh');
  });

  it('overwrites previously stored values', () => {
    store[STORAGE_KEY] = 'en';
    persistLocale('zh');
    expect(store[STORAGE_KEY]).toBe('zh');
  });
});
