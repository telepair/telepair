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
  | 'auth.error_email_taken'
  | 'auth.error_invalid_otp'
  | 'auth.error_otp_locked'
  | 'auth.error_invalid_credentials'
  | 'auth.error_not_verified'
  | 'auth.error_rate_limited'
  | 'auth.error_smtp_unavailable'
  | null;

const [token, setTokenSignal] = createSignal(readInitialToken());
const [validating, setValidating] = createSignal(false);
const [errorKey, setErrorKey] = createSignal<AuthErrorKey>(null);
// Cached caller identity from `/api/auth/whoami`. `null` while
// unknown (before the first successful load, or after logout),
// populated atomically by `loadIdentity()` so the `id`, `isAdmin`,
// and `isGuest` fields can never disagree. The dashboard's owner
// gate reads `id` to match against `session.owner_id`; route guards
// read `isAdmin` to gate admin pages; the session-page back button
// reads `isGuest` to decide between "return to dashboard" (real
// user keeps their identity) and "log out" (scoped guest has
// nowhere else to go — their token is only valid for this one
// session). These were previously three independent signals that
// always had to be set or cleared together — collapsing them
// removes the drift risk and the duplicate boilerplate.
interface CurrentUser {
  id: string;
  isAdmin: boolean;
  isGuest: boolean;
  sessionEnabled: boolean;
}
const [currentUser, setCurrentUser] = createSignal<CurrentUser | null>(null);
const currentUserId = () => currentUser()?.id ?? '';
const currentUserIsAdmin = () => currentUser()?.isAdmin ?? null;
const currentUserIsGuest = () => currentUser()?.isGuest ?? null;
const currentUserSessionEnabled = () => currentUser()?.sessionEnabled ?? null;

// In-flight `loadIdentity` promise, memoized so two concurrent
// mounts (e.g. Dashboard + AdminGuard on a deep link) share one
// whoami round-trip instead of racing. Cleared on success or
// failure so the next caller starts fresh.
let identityInFlight: Promise<void> | null = null;

// Flips to `true` when the FIRST `loadIdentity()` call settles —
// either the whoami succeeded and `currentUser` was populated, or
// it failed and `currentUser` stays null. AdminGuard watches this
// instead of `currentUserIsAdmin() !== null` so it can distinguish
// "still in flight" (show spinner placeholder) from "request
// finished but returned no usable identity" (redirect home instead
// of staying blank forever). Reset to `false` on logout so the
// next open of the admin page re-runs the check.
const [identityChecked, setIdentityChecked] = createSignal(false);

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
  const previous = token();
  if (value) {
    safeSet(sessionStore(), STORAGE_KEY, value);
    if (options.persist) {
      safeSet(localStore(), STORAGE_KEY, value);
    }
    // Identity is bound to the credential — a token swap (admin
    // logged in, then same tab walks a /join/:token invite link and
    // picks up a guest token) must invalidate the cached whoami, or
    // AdminGuard / dashboard owner gate / Session back-button all
    // keep running against the previous user's id and flags. Only
    // invalidate when the token actually changes so back-to-back
    // writes of the same value (persist upgrade, retry) don't churn
    // an in-flight whoami.
    if (value !== previous) {
      setCurrentUser(null);
      setIdentityChecked(false);
      identityInFlight = null;
    }
  } else {
    // Logout: clear both tiers. A shared-admin-then-logout flow should
    // force every future tab to re-login, not inherit the stale token
    // from localStorage.
    safeRemove(sessionStore(), STORAGE_KEY);
    safeRemove(localStore(), STORAGE_KEY);
    // Identity is bound to the credential — when the credential goes
    // away, the cached user must too. Otherwise the next login (e.g.
    // admin → guest invite → admin) would observe a stale id from
    // the previous session and mis-gate the dashboard's owner check,
    // or leak the admin gear icon to a subsequent guest session.
    setCurrentUser(null);
    setIdentityChecked(false);
    identityInFlight = null;
  }
  setTokenSignal(value);
  setErrorKey(null);
}

/**
 * Fetch and cache the caller's identity from `/api/auth/whoami`.
 * Idempotent AND de-duplicated: concurrent callers (Dashboard and
 * AdminGuard mounting on the same tick of a deep link) share one
 * HTTP round-trip via `identityInFlight`. Returns immediately if
 * the identity is already cached or no token is in scope. Failures
 * are swallowed — the next protected request will surface the real
 * error through the global 401 interceptor, and the dashboard's
 * owner-gate falls back to "no rows are owned" which is the safer
 * side of the call.
 */
async function loadIdentity(): Promise<void> {
  if (currentUser() || !token()) {
    // Already loaded, or no token — mark as settled so AdminGuard
    // doesn't hang on a whoami that will never fire.
    setIdentityChecked(true);
    return;
  }
  if (identityInFlight) return identityInFlight;
  identityInFlight = (async () => {
    try {
      const me = await api.whoami();
      setCurrentUser({
        id: me.user_id,
        isAdmin: me.is_admin,
        isGuest: me.is_guest,
        sessionEnabled: me.session_enabled,
      });
    } catch {
      // Non-fatal: see comment above. Specifically NOT calling
      // `logoutAndRedirect` here — a transient network blip during
      // dashboard mount must not bounce the user back to /login.
      // AdminGuard will redirect to `/` on its next render
      // (identityChecked=true, currentUserIsAdmin=null → not true).
    } finally {
      identityInFlight = null;
      // Signal that the first attempt has settled. AdminGuard reads
      // this to know when to stop showing the loading placeholder and
      // either render the content or redirect away.
      setIdentityChecked(true);
    }
  })();
  return identityInFlight;
}

async function validateToken(t: string): Promise<boolean> {
  setValidating(true);
  // Login is the "persistent" entry point — the admin wants their
  // primary token to survive a tab close. Guest redemption uses the
  // bare `setToken(value)` call instead.
  setToken(t, { persist: true });
  try {
    await api.listTargets();
    // Prime the cached identity right after the credential check —
    // the dashboard's owner gate runs on first paint, so deferring
    // this to a separate `loadIdentity()` call would race with the
    // first session-row click. Best-effort: a transient whoami
    // failure must not turn a successful login into a failed one.
    await loadIdentity();
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

/**
 * Email-based registration. Sends the OTP to the user's inbox.
 * Callers should transition to an OTP-entry step on `true`.
 */
async function emailRegister(
  email: string,
  password: string,
  displayName: string,
): Promise<boolean> {
  setValidating(true);
  try {
    await api.register(email, password, displayName);
    setErrorKey(null);
    return true;
  } catch (e) {
    if (e instanceof ApiError) {
      if (e.status === 409) setErrorKey('auth.error_email_taken');
      else if (e.status === 503) setErrorKey('auth.error_smtp_unavailable');
      else setErrorKey('auth.error_connection_failed');
    } else {
      setErrorKey('auth.error_connection_failed');
    }
    return false;
  } finally {
    setValidating(false);
  }
}

/**
 * OTP verification step. On success, stores the returned token (persistent)
 * and loads the user identity — same behaviour as a successful `validateToken`.
 */
async function emailVerifyOtp(email: string, code: string): Promise<boolean> {
  setValidating(true);
  try {
    const { token: t } = await api.verifyOtp(email, code);
    setToken(t, { persist: true });
    await loadIdentity();
    setErrorKey(null);
    return true;
  } catch (e) {
    if (e instanceof ApiError) {
      if (e.status === 429) setErrorKey('auth.error_otp_locked');
      else if (e.status === 400 || e.status === 401) setErrorKey('auth.error_invalid_otp');
      else setErrorKey('auth.error_connection_failed');
    } else {
      setErrorKey('auth.error_connection_failed');
    }
    return false;
  } finally {
    setValidating(false);
  }
}

/**
 * Email+password login. Stores the token persistently and loads identity.
 */
async function emailLogin(email: string, password: string): Promise<boolean> {
  setValidating(true);
  try {
    const { token: t } = await api.loginWithPassword(email, password);
    setToken(t, { persist: true });
    await loadIdentity();
    setErrorKey(null);
    return true;
  } catch (e) {
    if (e instanceof ApiError) {
      if (e.status === 401) setErrorKey('auth.error_invalid_credentials');
      else if (e.status === 403) setErrorKey('auth.error_not_verified');
      else setErrorKey('auth.error_connection_failed');
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
  currentUserId,
  currentUserIsAdmin,
  currentUserIsGuest,
  currentUserSessionEnabled,
  identityChecked,
  setToken,
  validateToken,
  emailRegister,
  emailVerifyOtp,
  emailLogin,
  loadIdentity,
  logout,
  logoutAndRedirect,
  isAuthenticated,
};
