// web/src/i18n/provider.tsx
//
// Solid context provider for i18n. Wraps `<App />` so every component
// can call `useI18n()` to get the reactive translator and locale signal.
//
// Why a custom provider instead of using `@solid-primitives/context`:
// avoids a second dependency for a 30-line wrapper. Solid's built-in
// `createContext` is enough.
//
// Reactivity contract:
// - `t(key, params?)` is built from `i18n.translator(() => dict, …)`,
//   where `dict` is recomputed whenever `locale()` changes. Solid's
//   reactive system auto-tracks the read inside the getter, so calling
//   `setLocale('zh')` triggers re-render in every component that
//   reads any `t(...)` value.
// - `setLocale` writes to `localStorage` synchronously so the choice
//   survives a reload before the next render even fires.

import {
  createContext,
  createSignal,
  useContext,
  type Accessor,
  type JSX,
} from 'solid-js';
import * as i18n from '@solid-primitives/i18n';

import { en } from './locales/en';
import { zh } from './locales/zh';
import { detectInitialLocale, persistLocale } from './detect';
import type { Locale } from './types';

/** Flatten the nested dictionaries once at module load. The flat keys
 *  are what `i18n.translator` actually consumes ("login.connect" etc).
 *  Both dicts ship in the main bundle — no lazy loading by design (see
 *  spec §1: total < 10 KB gzipped). */
const dictionaries = {
  en: i18n.flatten(en),
  zh: i18n.flatten(zh),
} as const;

type FlatDict = (typeof dictionaries)['en'];

/** All valid translation keys, derived from the English dict's flat
 *  shape. Use this when a key is computed at runtime and stored in a
 *  data structure (e.g. preset arrays) so the union still gets
 *  enforced at the literal-assignment site. */
export type TranslationKey = keyof FlatDict;

/** Optional template parameters. The library accepts any string-keyed
 *  record; we narrow to `string` values because every callsite already
 *  stringifies numbers explicitly. */
export type TranslationParams = Record<string, string>;

/** Strict translator: returns `string` (never `undefined`). The
 *  upstream `i18n.translator` returns `string | undefined` because it
 *  is designed for `createResource` flows where the dict can be
 *  loading. We load both dicts statically at module init, so the dict
 *  is never missing — but a *missing key* would still return
 *  undefined. Returning the raw key in that case fails loudly in the
 *  UI ("invite.foo" appears verbatim) instead of crashing the render
 *  with a `null is not assignable to ReactNode` error. */
export type Translator = (
  key: TranslationKey,
  params?: TranslationParams,
) => string;

export interface I18nContextValue {
  /** Current locale signal. Read inside JSX to react to changes. */
  locale: Accessor<Locale>;
  /** Switch the active locale and persist the choice. */
  setLocale: (locale: Locale) => void;
  /** Translator. Reactive — re-evaluates on `setLocale`. */
  t: Translator;
}

const I18nContext = createContext<I18nContextValue | undefined>(undefined);

/** Wraps the application root and exposes the i18n context. */
export function I18nProvider(props: { children: JSX.Element }): JSX.Element {
  const [locale, setLocaleSignal] = createSignal<Locale>(detectInitialLocale());

  // Getter is reactive: every read of `dictionaries[locale()]` is
  // tracked by Solid, so the translator returns fresh strings on
  // every locale change without any manual subscribe/unsubscribe.
  const rawTranslator = i18n.translator(
    () => dictionaries[locale()],
    i18n.resolveTemplate,
  );

  const t: Translator = (key, params) => {
    const result = rawTranslator(key, params);
    // Surface missing translations as the raw key — visible but
    // non-fatal. Should never fire if the dict-symmetry test passes.
    return typeof result === 'string' ? result : key;
  };

  const setLocale = (next: Locale) => {
    persistLocale(next);
    setLocaleSignal(next);
  };

  const value: I18nContextValue = { locale, setLocale, t };

  return (
    <I18nContext.Provider value={value}>{props.children}</I18nContext.Provider>
  );
}

/** Consume the i18n context. Throws if called outside `<I18nProvider>` —
 *  that's a programming error, not a runtime condition. */
export function useI18n(): I18nContextValue {
  const ctx = useContext(I18nContext);
  if (!ctx) {
    throw new Error('useI18n must be called inside <I18nProvider>');
  }
  return ctx;
}
