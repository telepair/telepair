// web/src/i18n/detect.ts
//
// Initial-locale detection and persistence helpers.
//
// Detection order:
//   1. Explicit user choice from `localStorage[telepair_locale]`
//   2. Browser preference via `navigator.language` (any `zh*` → 'zh')
//   3. Fallback English
//
// Storage helpers mirror the safe-access pattern from `stores/auth.ts`:
// `localStorage` can throw in private mode, sandboxed iframes, or when
// the quota is exceeded. We treat any failure as "no storage" and keep
// going rather than crashing the app.

import { FALLBACK_LOCALE, STORAGE_KEY, type Locale } from './types';

function safeGet(key: string): string | null {
  try {
    return typeof localStorage === 'undefined'
      ? null
      : localStorage.getItem(key);
  } catch {
    return null;
  }
}

function safeSet(key: string, value: string): void {
  try {
    if (typeof localStorage !== 'undefined') {
      localStorage.setItem(key, value);
    }
  } catch {
    // ignore — quota / private mode / sandboxed iframe
  }
}

function isLocale(value: unknown): value is Locale {
  return value === 'en' || value === 'zh';
}

/** Resolve the locale to use when the app first boots. Pure function —
 *  no side effects, safe to call from tests with mocked globals. */
export function detectInitialLocale(): Locale {
  const saved = safeGet(STORAGE_KEY);
  if (isLocale(saved)) return saved;

  const nav =
    typeof navigator !== 'undefined' && typeof navigator.language === 'string'
      ? navigator.language
      : '';
  if (nav.toLowerCase().startsWith('zh')) return 'zh';

  return FALLBACK_LOCALE;
}

/** Persist the user's explicit choice so subsequent visits and other
 *  tabs see the same language. Best-effort — failures are swallowed. */
export function persistLocale(locale: Locale): void {
  safeSet(STORAGE_KEY, locale);
}
