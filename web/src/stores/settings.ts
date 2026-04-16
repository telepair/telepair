// web/src/stores/settings.ts
import { createSignal } from 'solid-js';
import type { ThemeId, FontFamilyId } from '../lib/terminal-themes';
import { DEFAULT_THEME, DEFAULT_FONT_FAMILY } from '../lib/terminal-themes';

export const SETTINGS_KEY = 'telepair_terminal_settings';

export type CursorStyle = 'block' | 'underline' | 'bar';

export interface TerminalSettings {
  theme: ThemeId;
  fontSize: number;
  fontFamily: FontFamilyId;
  cursorStyle: CursorStyle;
  cursorBlink: boolean;
}

const DEFAULTS: TerminalSettings = {
  theme: DEFAULT_THEME,
  fontSize: 14,
  fontFamily: DEFAULT_FONT_FAMILY,
  cursorStyle: 'block',
  cursorBlink: true,
};

function safeGet(key: string): string | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage.getItem(key);
  } catch {
    return null;
  }
}

function safeSet(key: string, value: string): void {
  try {
    if (typeof localStorage !== 'undefined') localStorage.setItem(key, value);
  } catch {
    // quota / private mode
  }
}

function safeRemove(key: string): void {
  try {
    if (typeof localStorage !== 'undefined') localStorage.removeItem(key);
  } catch {
    // ignore
  }
}

export function loadSettings(): TerminalSettings {
  const raw = safeGet(SETTINGS_KEY);
  if (!raw) return { ...DEFAULTS };
  try {
    const parsed = JSON.parse(raw);
    return {
      theme: parsed.theme ?? DEFAULTS.theme,
      fontSize: clampFontSize(parsed.fontSize ?? DEFAULTS.fontSize),
      fontFamily: parsed.fontFamily ?? DEFAULTS.fontFamily,
      cursorStyle: parsed.cursorStyle ?? DEFAULTS.cursorStyle,
      cursorBlink: typeof parsed.cursorBlink === 'boolean' ? parsed.cursorBlink : DEFAULTS.cursorBlink,
    };
  } catch {
    return { ...DEFAULTS };
  }
}

function clampFontSize(n: number): number {
  return Math.max(10, Math.min(24, Math.round(n)));
}

function persist(settings: TerminalSettings): void {
  safeSet(SETTINGS_KEY, JSON.stringify(settings));
}

const [settings, setSettings] = createSignal<TerminalSettings>(loadSettings());

export const terminalSettings = settings;

function update(patch: Partial<TerminalSettings>): void {
  const next = { ...settings(), ...patch };
  setSettings(next);
  persist(next);
}

export function setTheme(theme: ThemeId): void {
  update({ theme });
}

export function setFontSize(size: number): void {
  update({ fontSize: clampFontSize(size) });
}

export function setFontFamily(family: FontFamilyId): void {
  update({ fontFamily: family });
}

export function setCursorStyle(style: CursorStyle): void {
  update({ cursorStyle: style });
}

export function setCursorBlink(blink: boolean): void {
  update({ cursorBlink: blink });
}

export function resetSettings(): void {
  setSettings({ ...DEFAULTS });
  safeRemove(SETTINGS_KEY);
}
