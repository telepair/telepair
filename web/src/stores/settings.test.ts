// web/src/stores/settings.test.ts
import { describe, it, expect, beforeEach, vi } from 'vitest';

const store: Record<string, string> = {};
let storageWritesThrow = false;
vi.stubGlobal('localStorage', {
  getItem: (key: string) => store[key] ?? null,
  setItem: (key: string, value: string) => {
    if (storageWritesThrow) throw new Error('QuotaExceededError');
    store[key] = value;
  },
  removeItem: (key: string) => {
    delete store[key];
  },
});

const { terminalSettings, setTheme, setFontSize, setFontFamily, setCursorStyle, setCursorBlink, resetSettings, setNotificationsEnabled, SETTINGS_KEY } = await import('./settings');

beforeEach(() => {
  for (const key of Object.keys(store)) delete store[key];
  storageWritesThrow = false;
  resetSettings();
});

describe('terminalSettings', () => {
  it('returns defaults when localStorage is empty', () => {
    const s = terminalSettings();
    expect(s.theme).toBe('github-dark');
    expect(s.fontSize).toBe(14);
    expect(s.fontFamily).toBe('jetbrains-mono');
    expect(s.cursorStyle).toBe('block');
    expect(s.cursorBlink).toBe(true);
  });

  it('persists theme change to localStorage', () => {
    setTheme('dracula');
    expect(terminalSettings().theme).toBe('dracula');
    const saved = JSON.parse(store[SETTINGS_KEY]);
    expect(saved.theme).toBe('dracula');
  });

  it('persists fontSize change to localStorage', () => {
    setFontSize(18);
    expect(terminalSettings().fontSize).toBe(18);
    const saved = JSON.parse(store[SETTINGS_KEY]);
    expect(saved.fontSize).toBe(18);
  });

  it('clamps fontSize to 10-24 range', () => {
    setFontSize(5);
    expect(terminalSettings().fontSize).toBe(10);
    setFontSize(30);
    expect(terminalSettings().fontSize).toBe(24);
  });

  it('persists fontFamily change', () => {
    setFontFamily('fira-code');
    expect(terminalSettings().fontFamily).toBe('fira-code');
  });

  it('persists cursorStyle change', () => {
    setCursorStyle('underline');
    expect(terminalSettings().cursorStyle).toBe('underline');
  });

  it('persists cursorBlink change', () => {
    setCursorBlink(false);
    expect(terminalSettings().cursorBlink).toBe(false);
  });

  it('resetSettings restores defaults and clears storage', () => {
    setTheme('monokai');
    setFontSize(20);
    resetSettings();
    expect(terminalSettings().theme).toBe('github-dark');
    expect(terminalSettings().fontSize).toBe(14);
    expect(store[SETTINGS_KEY]).toBeUndefined();
  });

  it('merges partial saved data with defaults', async () => {
    store[SETTINGS_KEY] = JSON.stringify({ fontSize: 20 });
    const { loadSettings } = await import('./settings');
    const loaded = loadSettings();
    expect(loaded.fontSize).toBe(20);
    expect(loaded.theme).toBe('github-dark');
  });

  it('survives corrupted localStorage gracefully', async () => {
    store[SETTINGS_KEY] = 'not-valid-json{{{';
    const { loadSettings } = await import('./settings');
    const loaded = loadSettings();
    expect(loaded.theme).toBe('github-dark');
  });

  it('survives localStorage write failures', () => {
    storageWritesThrow = true;
    setTheme('dracula');
    expect(terminalSettings().theme).toBe('dracula');
  });

  it('notificationsEnabled defaults to false', () => {
    expect(terminalSettings().notificationsEnabled).toBe(false);
  });

  it('persists notificationsEnabled change', () => {
    setNotificationsEnabled(true);
    expect(terminalSettings().notificationsEnabled).toBe(true);
    const saved = JSON.parse(store[SETTINGS_KEY]);
    expect(saved.notificationsEnabled).toBe(true);
  });

  it('resetSettings restores notificationsEnabled to false', () => {
    setNotificationsEnabled(true);
    resetSettings();
    expect(terminalSettings().notificationsEnabled).toBe(false);
  });
});
