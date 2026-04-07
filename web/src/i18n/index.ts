// web/src/i18n/index.ts
//
// Public surface for the i18n module. Components import from this file
// only — never reach into `provider.tsx` / `detect.ts` / `locales/*`
// directly.

export {
  I18nProvider,
  useI18n,
  type I18nContextValue,
  type Translator,
  type TranslationKey,
  type TranslationParams,
} from './provider';
export { detectInitialLocale, persistLocale } from './detect';
export { FALLBACK_LOCALE, STORAGE_KEY, type Locale } from './types';
export { renderTemplate } from './render-template';
export { roleLabel, inputModeLabel } from './labels';
