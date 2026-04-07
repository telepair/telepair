// web/src/test-setup.ts
//
// Vitest setup module — runs before every test file. Loaded via the
// `setupFiles` entry in `vite.config.ts`.
//
// Why this exists: the i18n provider auto-detects the user's locale at
// boot from `localStorage` and `navigator.language`. In CI or on a
// developer's machine that's set to Chinese, every test that touches
// the rendered UI would suddenly assert against Chinese strings. By
// pre-seeding `localStorage[telepair_locale] = 'en'` here, every test
// starts on English regardless of the host environment, and the
// existing English assertions stay valid.
//
// We do NOT touch `navigator.language` — letting jsdom default carries
// the secondary benefit that any test which clears localStorage and
// reloads the provider can still exercise the fallback path.

const LOCALE_KEY = 'telepair_locale';

try {
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem(LOCALE_KEY, 'en');
  }
} catch {
  // jsdom localStorage is normally writable; the only realistic
  // failure is a global stub installed by an individual test file
  // that throws on writes (see `auth.test.ts`). In that case the
  // test owns its own locale state anyway, so swallowing is safe.
}
