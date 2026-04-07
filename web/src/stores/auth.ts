// web/src/stores/auth.ts
import { createSignal } from 'solid-js';
import { api, ApiError } from '../lib/api';

export const STORAGE_KEY = 'telepair_token';

/**
 * Auth identity is scoped to a single browser tab.
 *
 * A single shared `localStorage` slot would make identity hijack
 * trivial: tab A is the admin, tab B opens a guest invite link, tab
 * B's redeem flow writes the guest token into the shared slot, and
 * tab A — on its very next request — quietly downgrades to that
 * guest. Splitting into two tiers fixes that:
 *   - `sessionStorage[STORAGE_KEY]` is the **authoritative** slot for
 *     this tab. Every read and write in this module goes through it.
 *     Browsers isolate sessionStorage per-tab, so tab B's guest token
 *     cannot reach tab A.
 *   - `localStorage[STORAGE_KEY]` is a **persistent fallback** kept
 *     only for the caller's long-lived identity (set via
 *     `setToken(value, { persist: true })` from the login flow). It
 *     seeds new tabs that have no sessionStorage entry so admins
 *     don't have to re-paste their token every time they open a tab.
 *
 * Guest tokens minted during invite redemption are NEVER persisted —
 * they expire with the tab, matching the "throwaway join" mental model.
 */

/**
 * Safe storage accessor. `localStorage` / `sessionStorage` can throw in
 * private browsing, sandboxed iframes, or when quota is exceeded. We
 * treat any failure as "no storage" and keep going rather than crashing
 * the app.
 */
function safeGet(storage: Storage | null, key: string): string | null {
  try {
    return storage?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

function safeSet(storage: Storage | null, key: string, value: string): void {
  try {
    storage?.setItem(key, value);
  } catch {
    // ignore
  }
}

function safeRemove(storage: Storage | null, key: string): void {
  try {
    storage?.removeItem(key);
  } catch {
    // ignore
  }
}

// Lazy globals: jsdom/tests may stub one before the other.
function sessionStore(): Storage | null {
  return typeof sessionStorage === 'undefined' ? null : sessionStorage;
}

function localStore(): Storage | null {
  return typeof localStorage === 'undefined' ? null : localStorage;
}

/**
 * Read the initial token for this tab. Prefers sessionStorage (already
 * bound to this tab); falls back to the persisted admin token and seeds
 * sessionStorage with it so subsequent writes stay tab-local.
 */
function readInitialToken(): string {
  const tabLocal = safeGet(sessionStore(), STORAGE_KEY);
  if (tabLocal) return tabLocal;
  const persistent = safeGet(localStore(), STORAGE_KEY);
  if (persistent) {
    safeSet(sessionStore(), STORAGE_KEY, persistent);
    return persistent;
  }
  return '';
}

/**
 * Source of truth for the API layer: callers outside this module (e.g.
 * `api.ts::request`) read the current token through this helper instead
 * of touching localStorage directly. Same sessionStorage-first fallback
 * as `readInitialToken` so a brand-new tab whose signal hasn't been
 * primed yet still gets the right identity on its first request.
 *
 * Last-resort fallback to the in-memory signal: `safeSet` swallows
 * `setItem` failures (private mode, quota exceeded, sandboxed iframe),
 * so storage may legitimately be empty even though the user is logged
 * in. Without this fallback, `validateToken()` and the guest invite
 * redeem flow would issue follow-up HTTP requests with no
 * `Authorization` header in those environments while the UI thinks
 * the user is authenticated, producing mysterious "I'm logged in but
 * everything 401s" reports.
 */
export function readCurrentToken(): string {
  const tabLocal = safeGet(sessionStore(), STORAGE_KEY);
  if (tabLocal) return tabLocal;
  const persistent = safeGet(localStore(), STORAGE_KEY);
  if (persistent) return persistent;
  return token();
}

/**
 * Auth error states are stored as **i18n keys**, not pre-translated
 * strings, so locale switches re-render the error live. The Login page
 * resolves the key through `useI18n().t()` at render time. `null` means
 * "no error". Adding a new error case requires:
 *   1. Add the key here
 *   2. Add the translation in `i18n/locales/en.ts` and `zh.ts`
 *   3. Use `setErrorKey('auth.error.<name>')` from this module
 */
export type AuthErrorKey =
  | 'auth.error_invalid_token'
  | 'auth.error_connection_failed'
  | null;

const [token, setTokenSignal] = createSignal(readInitialToken());
const [validating, setValidating] = createSignal(false);
const [errorKey, setErrorKey] = createSignal<AuthErrorKey>(null);

export interface SetTokenOptions {
  /**
   * If `true`, write to localStorage in addition to sessionStorage so
   * the token survives tab close. Used by the login flow for admin
   * tokens; invite redemption intentionally omits this so guest
   * identities stay tab-scoped and expire with the tab.
   */
  persist?: boolean;
}

function setToken(value: string, options: SetTokenOptions = {}) {
  if (value) {
    safeSet(sessionStore(), STORAGE_KEY, value);
    if (options.persist) {
      safeSet(localStore(), STORAGE_KEY, value);
    }
  } else {
    // Logout: clear both tiers. A shared-admin-then-logout flow should
    // force every future tab to re-login, not inherit the stale token
    // from localStorage.
    safeRemove(sessionStore(), STORAGE_KEY);
    safeRemove(localStore(), STORAGE_KEY);
  }
  setTokenSignal(value);
  setErrorKey(null);
}

async function validateToken(t: string): Promise<boolean> {
  setValidating(true);
  // Login is the "persistent" entry point — the admin wants their
  // primary token to survive a tab close. Guest redemption uses the
  // bare `setToken(value)` call instead.
  setToken(t, { persist: true });
  try {
    await api.listTargets();
    return true;
  } catch (e) {
    setToken('');
    if (e instanceof ApiError && e.status === 401) {
      setErrorKey('auth.error_invalid_token');
    } else {
      setErrorKey('auth.error_connection_failed');
    }
    return false;
  } finally {
    setValidating(false);
  }
}

function logout() {
  setToken('');
}

/**
 * Single source of truth for "drop credentials and bounce to /login".
 * Three call sites used to inline the same `auth.logout(); window
 * .location.assign('/login')` pattern (the api 401 interceptor, the
 * dashboard 403 path in `stores/session.ts`, and the non-owner exit in
 * `Session.tsx`). They drifted on the path-already-/login guard, which
 * meant the api interceptor would re-assign while already on /login
 * and pollute the back stack. Funnel everything here so the guard and
 * the order (clear token THEN navigate, so AuthGuard sees the empty
 * signal on the next route eval) stay consistent.
 */
function logoutAndRedirect() {
  setToken('');
  if (typeof window !== 'undefined' && window.location.pathname !== '/login') {
    window.location.assign('/login');
  }
}

function isAuthenticated(): boolean {
  return token().length > 0;
}

export const auth = {
  token,
  validating,
  errorKey,
  setToken,
  validateToken,
  logout,
  logoutAndRedirect,
  isAuthenticated,
};
