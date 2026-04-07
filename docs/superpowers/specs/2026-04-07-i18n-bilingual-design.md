# Bilingual (Chinese / English) Web UI — Design

**Status:** Approved for planning
**Author:** Liys (with Claude)
**Date:** 2026-04-07
**Scope:** `web/` SolidJS frontend only — Rust crates unchanged

## Summary

Add runtime bilingual (Simplified Chinese / English) support to the Telepair
web UI. New visitors see a language matching their browser; users can switch
at any time and the choice persists across visits and tabs. Translation
covers all UI surfaces, client-side toasts/banners, and server errors that
arrive with a structured `ErrorCode`. Server-only fallback strings remain
English.

## Goals

- Every user-visible string in the four pages and the six string-bearing
  components is translated to both languages. (Skeleton and Terminal hold
  no user-facing text and are untouched.)
- Default locale comes from `navigator.language`; user choice is sticky in
  `localStorage`.
- Switching language is zero-flicker, does not break the WebSocket, and does
  not clear the xterm.js terminal buffer.
- Type-safe dictionary: a missing Chinese key fails `tsc`.
- All 67 existing vitest tests and 18 Playwright e2e tests keep passing,
  with minimal mechanical changes.
- Bundle increase ≤ 10 KB gzipped.

## Non-goals

- No traditional Chinese, no other languages — bilingual only.
- No translation of server-side `Error.message` fallback strings (those
  arrive without an `ErrorCode` and are intentionally raw).
- No translation of the xterm.js terminal byte stream (it's PTY output,
  not UI chrome).
- No SSR / static rendering — Telepair is a client-rendered SPA today.
- No lazy-loaded language packs — both dictionaries ship in the main bundle
  (total < 10 KB).

## Decisions made during brainstorming

| # | Question | Decision |
|---|----------|----------|
| Q1 | Translation scope | UI + client-side toasts/banners + ErrorCode-mapped server errors. Server fallback strings stay English. |
| Q2 | Default locale | Browser detect on first visit; persist user choice in `localStorage` thereafter. |
| Q3 | Switcher placement | Topbar slot in Dashboard / Session; small link at the bottom of the Login / Join cards. Uses `中文 \| English` toggle, not a dropdown. |
| Q4 | Library choice | `@solid-primitives/i18n` (Solid official, ~3 KB gzipped, type-safe). |
| Q5 | Test strategy | Lock locale to `en` in vitest setup + Playwright config; refactor `auth` store to expose error **keys** instead of translated strings so its tests stay decoupled from i18n. |

## Architecture

```
web/src/i18n/
├── index.ts            # Re-exports useI18n, locale signal, setLocale
├── provider.tsx        # <I18nProvider> wraps <App>; supplies context
├── detect.ts           # detectInitialLocale() + persistLocale()
├── locales/
│   ├── en.ts           # English dictionary; source of Dict type
│   └── zh.ts           # Chinese dictionary; typed `: Dict`
└── types.ts            # `Locale = 'en' | 'zh'`, `Dict` type
```

- Provider sits at the root of `App.tsx` (outside `<Router>` so route
  changes don't recreate the locale signal).
- Components consume via `const [t, { locale }] = useI18n();`.
- `t()` is a Solid-derived signal — locale changes trigger automatic
  re-render of every consumer.

## Default-locale detection & persistence

```ts
// detect.ts
const STORAGE_KEY = 'telepair_locale';

export function detectInitialLocale(): Locale {
  const saved = safeGet(STORAGE_KEY);
  if (saved === 'en' || saved === 'zh') return saved;
  const nav = typeof navigator !== 'undefined' ? navigator.language : '';
  if (nav.toLowerCase().startsWith('zh')) return 'zh';
  return 'en';
}

export function persistLocale(locale: Locale): void {
  safeSet(STORAGE_KEY, locale);
}
```

- `safeGet` / `safeSet` mirror the helpers in `auth.ts` (private mode and
  iframe sandbox can throw on `localStorage` access). Helpers are inlined,
  not extracted to a shared module — only two callsites, YAGNI.
- `localStorage`, **not** `sessionStorage`. Locale is a cross-tab user
  preference, unlike auth tokens which are per-tab.
- Only `navigator.language` is read; `navigator.languages[]` priority list
  is overkill for two languages.
- `zh-CN`, `zh-TW`, `zh-HK` all map to `zh` (we ship Simplified only this
  round).
- Test environments without `navigator` fall back to English.

## Dictionary structure

- Flat key + dot-namespace: `domain.subdomain.key` (e.g.
  `'invite.role.viewer_desc'`).
- ~76 keys total, organised by surface: `common`, `login`, `dashboard`,
  `create_session`, `session`, `invite`, `chat`, `participants`, `join`,
  `auth.error`, `toast`, `locale.switch`.
- Interpolation uses `{name}` placeholders, expanded by
  `@solid-primitives/i18n`'s `template()` helper.
- Plurals use sibling keys (`xxx.singular` / `xxx.plural`) — no ICU runtime.
  English distinguishes; Chinese keys are duplicated content but kept
  separate so adding plural-rich languages later doesn't require a
  refactor.
- Brand name `telepair` is **not** in the dictionary; CLI strings inside
  `<code>` tags are not translated.
- Type contract: `export type Dict = typeof en;` and `zh.ts` is annotated
  `export const zh: Dict = { … }` so any missing key is a `tsc` error.
- A vitest case asserts `Object.keys(en).sort() === Object.keys(zh).sort()`
  as a runtime backstop (in case someone bypasses the type check via
  `as any`).

## Component changes

### App-level

```tsx
// App.tsx
import { I18nProvider } from './i18n/provider';

export default function App() {
  return (
    <I18nProvider>
      <Router>...</Router>
      <ToastContainer />
    </I18nProvider>
  );
}
```

### New: `LocaleSwitcher` component

```tsx
<LocaleSwitcher variant="topbar" />   // Dashboard / Session topbar
<LocaleSwitcher variant="card" />     // Login / Join card footer
```

- `topbar`: compact `中文 | English` button pair, current locale is bold
  + accent colour, `aria-pressed` true on the active one.
- `card`: smaller, link-style, centred under the card content.
- Both variants share `setLocale` and the same `aria-label`
  (`'Switch language'`, itself a translated key).

### Files touched

| File | Change |
|------|--------|
| `App.tsx` | Wrap children in `<I18nProvider>` |
| `pages/Login.tsx` | All literals → `t(key)`; `<LocaleSwitcher variant="card" />` at bottom |
| `pages/Dashboard.tsx` | All literals → `t(key)`; `<LocaleSwitcher variant="topbar" />` left of Refresh |
| `pages/Session.tsx` | All literals → `t(key)`; `<LocaleSwitcher variant="topbar" />` left of Invite. (Does not read `auth.error` — see store refactor below.) |
| `pages/Join.tsx` | All literals → `t(key)`; `<LocaleSwitcher variant="card" />` at bottom |
| `components/Banner.tsx` | `aria-label="Dismiss notification"` → `t('common.dismiss')` |
| `components/CreateSessionDialog.tsx` | All literals → `t(key)` |
| `components/InviteDialog.tsx` | All literals → `t(key)`; `formatExpiry` accepts `t` and returns translated string |
| `components/ChatPanel.tsx` | All literals → `t(key)` |
| `components/ParticipantList.tsx` | `'Participants ({n})'` → `t('participants.heading', { count: n })` |
| `components/Toast.tsx` | `aria-label="Notifications"` → `t('toast.region_label')`; `aria-label="Dismiss notification"` → `t('common.dismiss')` |

### `auth` store refactor

Breaking change — no backwards compatibility shim:

```ts
// before
const [error, setError] = createSignal('');
setError('Invalid token');

// after
type AuthErrorKey =
  | 'auth.error.invalid_token'
  | 'auth.error.connection_failed'
  | null;
const [errorKey, setErrorKey] = createSignal<AuthErrorKey>(null);
setErrorKey('auth.error.invalid_token');
```

- Public surface changes: `auth.error` → `auth.errorKey`.
- Only consumer is `Login.tsx`, which renders `t(auth.errorKey() ?? '')`.
- `auth.test.ts` updates 5 assertions from English literals to keys.
- All other stores (`session.ts`, `toast.ts`) are unchanged: `toast` keeps
  receiving already-translated strings from callers, so the toast queue
  itself stays a pure presentation layer.

## Data flow

```
index.tsx (boot)
  └→ render(<App />)
       └→ <I18nProvider>
           ├─ detectInitialLocale()
           │    ├─ localStorage['telepair_locale']
           │    ├─ navigator.language.startsWith('zh')
           │    └─ fallback 'en'
           ├─ createSignal<Locale>(initial)
           └─ provide { locale, setLocale, t } via createContext

Component render
  const [t, { locale }] = useI18n();
  return <h1>{t('login.subtitle')}</h1>
  → t() is a derived signal; locale change re-evaluates every consumer

User clicks LocaleSwitcher
  onClick → setLocale('zh')
    ├─ localStorage.setItem('telepair_locale', 'zh')
    ├─ signal updates
    └─ Solid reactive system propagates → consumers reflow
```

### Runtime guarantees

- **Zero page reload**: locale switch is a pure signal update — WebSocket,
  xterm.js buffer, scroll position, and form drafts all survive.
- **Toast text frozen at creation**: a toast already on screen keeps the
  language it was created in. Acceptable: toast lifetime is ~3 s.
- **Banner / `auth.errorKey` retranslate live**: their text is computed
  inside `t()` on every render.

## Testing

| Layer | Change |
|-------|--------|
| Vitest setup | New / extended `web/src/test-setup.ts`: `beforeEach` writes `localStorage.setItem('telepair_locale', 'en')` |
| `auth.test.ts` | 5 assertions updated to keys |
| `i18n/detect.test.ts` (new) | localStorage priority, `zh-CN` detect, `en-US` detect, missing navigator fallback |
| `i18n/provider.test.tsx` (new) | `t()` returns correct string before/after `setLocale`; `localStorage` write verified |
| `i18n/dict.test.ts` (new) | `Object.keys(en).sort()` deep-equals `Object.keys(zh).sort()` |
| `LocaleSwitcher.test.tsx` (new) | Click flips `aria-pressed`; locale signal updates |
| Playwright | `playwright.config.ts` `use.storageState` seeds `telepair_locale=en` so e2e English assertions still match |

Net new vitest cases: ~8. Total: 75. Coverage stays ≥ 80 %.

## Risks & mitigations

| Risk | Mitigation |
|------|-----------|
| `auth.error → errorKey` rename is breaking | TS will catch any missed consumer at compile time; only `Login.tsx` reads it today |
| `@solid-primitives/i18n` peerDependencies | Verify it accepts `solid-js@^1.9` before installing; bail to a 60-line DIY module if not |
| Toast already on screen doesn't retranslate | Documented as accepted behaviour |
| Playwright e2e uses English text locators | Lock locale to `en` in `playwright.config.ts`; no e2e changes |
| jsdom defaults `navigator.language` to `en-US` | Default-fallback English path means existing tests stay green |
| Server-side error.message strings remain English | Out of scope per Q1 = B; only `ErrorCode`-mapped errors are translated |

## Bundle impact

- `@solid-primitives/i18n`: ~3 KB gzipped
- Two dictionaries: ~4.5 KB unminified, ~2 KB gzipped
- Total: < 10 KB gzipped vs current dist of several hundred KB → < 5 %
  increase.

## Implementation order (high-level)

The `writing-plans` skill will turn this into step-by-step tasks. The
intended sequence:

1. Install `@solid-primitives/i18n`; scaffold `web/src/i18n/` with empty
   dictionaries and Provider.
2. Fill `en.ts` (~75 keys) and `zh.ts` translations; add the dict-symmetry
   test.
3. Wrap `App.tsx` in `<I18nProvider>`.
4. Refactor `auth.ts` (`error → errorKey`); update `auth.test.ts`.
5. Translate components in order of complexity: Banner → ChatPanel →
   ParticipantList → CreateSessionDialog → InviteDialog → Login → Join →
   Dashboard → Session. Type-check after each.
6. Build `LocaleSwitcher` and place it in the four pages.
7. Update vitest setup and `playwright.config.ts` to lock `en`; add the new
   i18n tests.
8. Run `make all` until green; run `make e2e` for full regression.
