// web/src/i18n/types.ts
//
// Locale identifiers and dictionary type contract.
//
// `Locale` is the union of language codes the app supports. The `Dict`
// type is *not* defined here — it is inferred from `locales/en.ts` so
// that adding a key to English automatically forces every other locale
// to add the same key (TS error). See `locales/zh.ts` for the
// `: typeof en` annotation that enforces this.

export type Locale = 'en' | 'zh';

/** Default fallback when nothing else matches (no `localStorage`, no
 *  `navigator`, unrecognised `navigator.language`). */
export const FALLBACK_LOCALE: Locale = 'en';

/** localStorage key used to persist the user's explicit choice. Lives
 *  in `localStorage` (not `sessionStorage`) because language is a
 *  cross-tab preference, unlike auth tokens which are tab-scoped. */
export const STORAGE_KEY = 'telepair_locale';
