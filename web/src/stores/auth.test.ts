import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createEffect, createRoot } from 'solid-js';

const mockFetch = vi.fn();
vi.stubGlobal('fetch', mockFetch);

// Two independent backing stores so the test can assert *which tier*
// each write lands in. The sessionStorage stub is the per-tab
// identity slot; the localStorage stub is the persistent admin
// fallback. Cross-tab isolation depends on keeping these distinct, so
// testing them as a single `store` would mask a regression.
const tabStore: Record<string, string> = {};
const persistStore: Record<string, string> = {};
// Toggled by the storage-failure regression test to simulate private
// mode / quota exhaustion: when set, every `setItem` throws so the
// `safeSet` helper takes its swallow-and-continue path.
let storageWritesThrow = false;
vi.stubGlobal('sessionStorage', {
  getItem: (key: string) => tabStore[key] ?? null,
  setItem: (key: string, value: string) => {
    if (storageWritesThrow) throw new Error('QuotaExceededError');
    tabStore[key] = value;
  },
  removeItem: (key: string) => { delete tabStore[key]; },
});
vi.stubGlobal('localStorage', {
  getItem: (key: string) => persistStore[key] ?? null,
  setItem: (key: string, value: string) => {
    if (storageWritesThrow) throw new Error('QuotaExceededError');
    persistStore[key] = value;
  },
  removeItem: (key: string) => { delete persistStore[key]; },
});

const { auth, STORAGE_KEY, onTokenChange } = await import('./auth');
const { __setAuthExpiredHandler } = await import('../lib/api');
// Neutralise the stale-token redirect: validateToken intentionally
// probes with api.listTargets() and a 401 response now fires the
// interceptor, which tries window.location.assign('/login') under
// jsdom. Tests care about the signal/localStorage effects, not the
// navigation, so swap the handler for a no-op.
__setAuthExpiredHandler(() => {});

function jsonResponse(data: unknown, status = 200) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

beforeEach(() => {
  mockFetch.mockReset();
  storageWritesThrow = false;
  delete tabStore[STORAGE_KEY];
  delete persistStore[STORAGE_KEY];
  auth.logout();
});

describe('auth.isAuthenticated', () => {
  it('returns false when no token', () => {
    expect(auth.isAuthenticated()).toBe(false);
  });

  it('returns true after setting token', () => {
    auth.setToken('valid-token');
    expect(auth.isAuthenticated()).toBe(true);
  });
});

describe('auth.setToken', () => {
  it('writes to sessionStorage only by default (tab-scoped)', () => {
    auth.setToken('my-token');
    expect(tabStore[STORAGE_KEY]).toBe('my-token');
    expect(persistStore[STORAGE_KEY]).toBeUndefined();
    expect(auth.token()).toBe('my-token');
  });

  it('writes to both tiers when { persist: true }', () => {
    auth.setToken('admin-token', { persist: true });
    expect(tabStore[STORAGE_KEY]).toBe('admin-token');
    expect(persistStore[STORAGE_KEY]).toBe('admin-token');
    expect(auth.token()).toBe('admin-token');
  });

  it('clearing empties both tiers so future tabs do not inherit the token', () => {
    auth.setToken('admin-token', { persist: true });
    auth.setToken('');
    expect(tabStore[STORAGE_KEY]).toBeUndefined();
    expect(persistStore[STORAGE_KEY]).toBeUndefined();
    expect(auth.token()).toBe('');
  });

  it('clears error', () => {
    // Force an error state first
    mockFetch.mockResolvedValueOnce(new Response('Unauthorized', { status: 401 }));
    auth.validateToken('bad').then(() => {
      auth.setToken('new');
      expect(auth.errorKey()).toBeNull();
    });
  });
});

describe('auth.validateToken', () => {
  it('returns true and persists admin token to both tiers on success', async () => {
    mockFetch.mockResolvedValueOnce(jsonResponse([]));
    const result = await auth.validateToken('good-token');
    expect(result).toBe(true);
    expect(auth.token()).toBe('good-token');
    // Login hits the persistent tier so future tabs inherit the admin
    // identity without re-pasting the token.
    expect(tabStore[STORAGE_KEY]).toBe('good-token');
    expect(persistStore[STORAGE_KEY]).toBe('good-token');
    expect(auth.errorKey()).toBeNull();
    expect(auth.validating()).toBe(false);
  });

  it('returns false and sets the invalid-token error key on 401', async () => {
    mockFetch.mockResolvedValueOnce(new Response('Unauthorized', { status: 401 }));
    const result = await auth.validateToken('bad-token');
    expect(result).toBe(false);
    expect(auth.token()).toBe('');
    expect(auth.errorKey()).toBe('auth.error_invalid_token');
    expect(auth.validating()).toBe(false);
  });

  it('returns false with the connection-failed error key on network error', async () => {
    mockFetch.mockRejectedValueOnce(new Error('Network error'));
    const result = await auth.validateToken('any-token');
    expect(result).toBe(false);
    expect(auth.errorKey()).toBe('auth.error_connection_failed');
  });

  it('removes token from both tiers on failure', async () => {
    mockFetch.mockResolvedValueOnce(new Response('Unauthorized', { status: 401 }));
    await auth.validateToken('will-fail');
    expect(tabStore[STORAGE_KEY]).toBeUndefined();
    expect(persistStore[STORAGE_KEY]).toBeUndefined();
  });

  it('trims surrounding whitespace before persisting the token (F1-q1)', async () => {
    // Copy-pasted tokens routinely pick up a trailing newline from
    // the terminal or source document. Before v0.1.5 the raw value
    // landed in localStorage, producing a confusing "invalid token"
    // on every subsequent request. validateToken must normalise.
    mockFetch.mockResolvedValueOnce(jsonResponse([]));
    const result = await auth.validateToken('  padded-token\n');
    expect(result).toBe(true);
    expect(auth.token()).toBe('padded-token');
    expect(tabStore[STORAGE_KEY]).toBe('padded-token');
    expect(persistStore[STORAGE_KEY]).toBe('padded-token');
  });
});

describe('auth.logout', () => {
  it('clears token and both storage tiers', () => {
    auth.setToken('active-token', { persist: true });
    auth.logout();
    expect(auth.token()).toBe('');
    expect(auth.isAuthenticated()).toBe(false);
    expect(tabStore[STORAGE_KEY]).toBeUndefined();
    expect(persistStore[STORAGE_KEY]).toBeUndefined();
  });
});

describe('cross-tab isolation', () => {
  it('guest redeem in one "tab" does not clobber the admin token in another', async () => {
    // Simulate: admin logged in elsewhere — the persistent tier holds
    // their token. This tab has no sessionStorage entry yet, so any
    // subsequent guest setToken here MUST NOT overwrite the persistent
    // admin token.
    persistStore[STORAGE_KEY] = 'admin-token';

    auth.setToken('guest-token'); // invite-redeem path, tab-scoped only
    expect(tabStore[STORAGE_KEY]).toBe('guest-token');
    expect(persistStore[STORAGE_KEY]).toBe('admin-token');
  });
});

describe('auth.loadIdentity', () => {
  // H3 regression: Session.tsx dispatches its back/exit button between
  // "navigate to dashboard" (real user — keep identity) and "logout +
  // redirect" (scoped guest — their token is only valid for this one
  // session and has nowhere else to go). The dispatch reads
  // `auth.currentUserIsGuest()`, which is populated here from the
  // whoami response. If loadIdentity drops `is_guest`, a real admin
  // viewing a session they don't own gets silently logged out on back.
  it('populates isGuest=true from a guest whoami response', async () => {
    auth.setToken('guest-token'); // non-persistent — matches redeem flow
    mockFetch.mockResolvedValueOnce(
      jsonResponse({
        user_id: 'guest-1',
        name: 'guest',
        is_admin: false,
        is_guest: true,
      }),
    );
    await auth.loadIdentity();
    expect(auth.currentUserId()).toBe('guest-1');
    expect(auth.currentUserIsAdmin()).toBe(false);
    expect(auth.currentUserIsGuest()).toBe(true);
  });

  it('populates isGuest=false for a long-lived admin identity', async () => {
    auth.setToken('admin-token', { persist: true });
    mockFetch.mockResolvedValueOnce(
      jsonResponse({
        user_id: 'root',
        name: 'admin',
        is_admin: true,
        is_guest: false,
      }),
    );
    await auth.loadIdentity();
    expect(auth.currentUserId()).toBe('root');
    expect(auth.currentUserIsAdmin()).toBe(true);
    expect(auth.currentUserIsGuest()).toBe(false);
  });

  it('returns null for the role flags before loadIdentity has run', () => {
    // Pre-whoami state: the dashboard's owner gate and Session.tsx's
    // back-button dispatch both need a tri-state signal so they can
    // distinguish "not yet known" from "known to be a non-admin /
    // non-guest". The signal must be null until the whoami round-trip
    // completes, not default to false.
    auth.setToken('any-token');
    expect(auth.currentUserIsAdmin()).toBeNull();
    expect(auth.currentUserIsGuest()).toBeNull();
    expect(auth.currentUserId()).toBe('');
  });

  // M1 regression: AdminGuard used to block on `currentUserIsAdmin() !== null`
  // and show a `<div />` forever when whoami failed. The fix adds
  // `identityChecked`, which flips to `true` once the first fetch
  // settles — success OR failure — so AdminGuard can redirect to `/`
  // instead of staying blank. These tests pin the three transitions.
  it('identityChecked is false before any loadIdentity call', () => {
    auth.setToken('fresh-token');
    expect(auth.identityChecked()).toBe(false);
  });

  it('identityChecked becomes true after a successful whoami', async () => {
    auth.setToken('admin-token', { persist: true });
    mockFetch.mockResolvedValueOnce(
      jsonResponse({ user_id: 'root', name: 'admin', is_admin: true, is_guest: false }),
    );
    expect(auth.identityChecked()).toBe(false);
    await auth.loadIdentity();
    expect(auth.identityChecked()).toBe(true);
    expect(auth.currentUserIsAdmin()).toBe(true);
  });

  it('identityChecked becomes true even when whoami fails', async () => {
    // This is the core M1 regression: a transient network error
    // previously left identityChecked=false (implicitly, since the
    // signal didn't exist) and AdminGuard would show an empty `<div />`
    // forever. After the fix, identityChecked flips on any settlement,
    // so the guard can fall through to the `Navigate href="/"` branch.
    auth.setToken('admin-token', { persist: true });
    mockFetch.mockRejectedValueOnce(new Error('network down'));
    await auth.loadIdentity();
    expect(auth.identityChecked()).toBe(true);
    // Identity stays null — the guard's redirect path handles this.
    expect(auth.currentUserIsAdmin()).toBeNull();
  });

  // Token-swap regression: before the fix, setToken only cleared the
  // cached `currentUser` on the empty-string branch. The admin → guest
  // invite flow in the same tab goes through `auth.setToken(guestToken)`
  // (not validateToken), so `currentUser` was left pointing at the
  // previous admin identity. AdminGuard, the dashboard owner gate, and
  // the Session back-button all read from that cache — they would
  // happily run as the wrong user until the next tab close. The fix
  // clears the cache whenever the token value actually changes.
  it('clears cached identity when setToken swaps the token to a new value', async () => {
    // Prime: admin logs in and whoami populates the cache.
    auth.setToken('admin-token', { persist: true });
    mockFetch.mockResolvedValueOnce(
      jsonResponse({ user_id: 'root', name: 'admin', is_admin: true, is_guest: false }),
    );
    await auth.loadIdentity();
    expect(auth.currentUserIsAdmin()).toBe(true);
    expect(auth.identityChecked()).toBe(true);

    // Same tab follows an invite link — Join.tsx path: setToken with a
    // fresh guest token, no validateToken round-trip. The stale admin
    // identity must NOT survive this swap.
    auth.setToken('guest-token');
    expect(auth.currentUserIsAdmin()).toBeNull();
    expect(auth.currentUserIsGuest()).toBeNull();
    expect(auth.currentUserId()).toBe('');
    expect(auth.identityChecked()).toBe(false);

    // And the next loadIdentity must actually hit the network — the
    // in-flight memo also has to have been reset, otherwise a stale
    // null promise would short-circuit the new fetch.
    mockFetch.mockResolvedValueOnce(
      jsonResponse({ user_id: 'guest-1', name: 'guest', is_admin: false, is_guest: true }),
    );
    await auth.loadIdentity();
    expect(auth.currentUserId()).toBe('guest-1');
    expect(auth.currentUserIsGuest()).toBe(true);
  });

  // Batched-write regression: setToken mutates `token`, `currentUser`,
  // `identityChecked`, and `errorKey`. Before these writes were
  // wrapped in `batch()`, a subscriber reading `token()` and
  // `currentUserIsAdmin()` together observed an intermediate frame
  // where the token had already flipped to the guest value but
  // `currentUser` still pointed at the previous admin — during an
  // admin → guest invite swap that briefly let `AdminGuard` render
  // admin UI against a guest token before the next microtask
  // invalidated it. The contract after the fix: every reactive
  // observer of these signals must see exactly ONE transition per
  // `setToken` call, with token and identity settled to their post-
  // swap values as an atomic unit.
  it('updates token and identity atomically (single batched transition)', async () => {
    // Prime admin identity so the cache has something to invalidate.
    auth.setToken('admin-token', { persist: true });
    mockFetch.mockResolvedValueOnce(
      jsonResponse({ user_id: 'root', name: 'admin', is_admin: true, is_guest: false }),
    );
    await auth.loadIdentity();
    expect(auth.currentUserIsAdmin()).toBe(true);

    // Run observers inside a root so we can dispose them cleanly
    // after the assertion — stray effects across tests would pollute
    // the next run's reactive graph.
    const observations: Array<{ token: string; isAdmin: boolean | null }> = [];
    const dispose = createRoot((disposeFn) => {
      createEffect(() => {
        observations.push({
          token: auth.token(),
          isAdmin: auth.currentUserIsAdmin(),
        });
      });
      return disposeFn;
    });

    // Initial synchronous run of the effect captures the primed
    // state. Everything AFTER this point is the transition under
    // test.
    expect(observations).toEqual([{ token: 'admin-token', isAdmin: true }]);
    observations.length = 0;

    // The invite-swap: new token, identity must flip to null in the
    // same frame. With `batch()`, Solid coalesces the writes and the
    // effect re-runs exactly once. Without it, there would be two
    // runs — the first observing the stale `{ token: guest, isAdmin: true }`
    // intermediate state that `AdminGuard` used to race.
    auth.setToken('guest-token');
    expect(observations).toEqual([{ token: 'guest-token', isAdmin: null }]);

    dispose();
  });

  // Idempotence: writing the SAME token twice (e.g. a persist-tier
  // upgrade from a non-persistent guest to an admin login with the
  // same token value, or a retry after a transient failure) must not
  // throw away an already-loaded identity, otherwise every repeat
  // setToken call forces a needless whoami round-trip.
  it('does not clear cached identity when setToken writes the same value', async () => {
    auth.setToken('admin-token');
    mockFetch.mockResolvedValueOnce(
      jsonResponse({ user_id: 'root', name: 'admin', is_admin: true, is_guest: false }),
    );
    await auth.loadIdentity();
    expect(auth.currentUserIsAdmin()).toBe(true);

    auth.setToken('admin-token', { persist: true });
    expect(auth.currentUserIsAdmin()).toBe(true);
    expect(auth.currentUserId()).toBe('root');
    expect(auth.identityChecked()).toBe(true);
  });

  it('identityChecked resets to false on logout', async () => {
    auth.setToken('admin-token', { persist: true });
    mockFetch.mockResolvedValueOnce(
      jsonResponse({ user_id: 'root', name: 'admin', is_admin: true, is_guest: false }),
    );
    await auth.loadIdentity();
    expect(auth.identityChecked()).toBe(true);
    auth.logout();
    expect(auth.identityChecked()).toBe(false);
  });
});

describe('auth.refreshIdentity', () => {
  it('forces a fresh whoami even when identity is already cached', async () => {
    auth.setToken('admin-token', { persist: true });
    // First load: pending user
    mockFetch.mockResolvedValueOnce(
      jsonResponse({
        user_id: 'u1', name: 'alice', is_admin: false, is_guest: false,
        session_enabled: false,
      }),
    );
    await auth.loadIdentity();
    expect(auth.currentUserSessionEnabled()).toBe(false);

    // Admin approves, user clicks "Check status"
    mockFetch.mockResolvedValueOnce(
      jsonResponse({
        user_id: 'u1', name: 'alice', is_admin: false, is_guest: false,
        session_enabled: true,
      }),
    );
    await auth.refreshIdentity();
    expect(auth.currentUserSessionEnabled()).toBe(true);
  });

  it('no-ops when there is no token', async () => {
    auth.logout();
    await auth.refreshIdentity();
    expect(auth.currentUserId()).toBe('');
  });
});

describe('onTokenChange', () => {
  // Regression: module-level stores (sessionStore) cached per-identity
  // data across tabs. Before `onTokenChange`, logout/login in the same
  // tab left user A's targets and sessions in memory; on user B's next
  // Dashboard mount they were rendered for one frame before the refetch
  // landed. The contract: subscribers fire only when the token *value*
  // changes, receive both prev and next, and one listener's throw must
  // not starve others.
  it('fires on every value change with (prev, next)', () => {
    const events: Array<[string, string]> = [];
    const unsubscribe = onTokenChange((prev, next) => {
      events.push([prev, next]);
    });
    try {
      auth.setToken('a');
      auth.setToken('b');
      auth.setToken('');
      expect(events).toEqual([
        ['', 'a'],
        ['a', 'b'],
        ['b', ''],
      ]);
    } finally {
      unsubscribe();
    }
  });

  it('does NOT fire when setToken is called with the same value', () => {
    // Matches the identity-cache invariant: back-to-back writes of the
    // same token (persist upgrade, retry) must not churn listeners.
    auth.setToken('same');
    let count = 0;
    const unsubscribe = onTokenChange(() => { count += 1; });
    try {
      auth.setToken('same');
      auth.setToken('same', { persist: true });
      expect(count).toBe(0);
    } finally {
      unsubscribe();
    }
  });

  it('unsubscribe stops notifications', () => {
    let count = 0;
    const unsubscribe = onTokenChange(() => { count += 1; });
    auth.setToken('first');
    expect(count).toBe(1);
    unsubscribe();
    auth.setToken('second');
    expect(count).toBe(1);
  });

  it('a throwing listener does not prevent other listeners from firing', () => {
    // The emitter wraps each call in try/catch so one buggy subscriber
    // (e.g. stale module from a hot-reload) cannot strand the others.
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    let bCalled = false;
    const unA = onTokenChange(() => { throw new Error('listener A boom'); });
    const unB = onTokenChange(() => { bCalled = true; });
    try {
      auth.setToken('ok');
      expect(bCalled).toBe(true);
      expect(errorSpy).toHaveBeenCalled();
    } finally {
      unA();
      unB();
      errorSpy.mockRestore();
    }
  });
});

describe('auth.resendOtp', () => {
  it('calls register endpoint and returns true on success', async () => {
    mockFetch.mockResolvedValueOnce(
      jsonResponse({ message: 'Verification code sent to your email.' }, 201),
    );
    const result = await auth.resendOtp('alice@example.com', 'pw123', 'Alice');
    expect(result).toBe(true);
    expect(auth.errorKey()).toBeNull();
    // Verify it hit the register endpoint with the right body
    const [url, opts] = mockFetch.mock.calls[0];
    expect(url).toBe('/api/auth/register');
    expect(opts.method).toBe('POST');
    expect(JSON.parse(opts.body)).toEqual({
      email: 'alice@example.com',
      password: 'pw123',
      display_name: 'Alice',
    });
  });

  it('returns false and sets error on SMTP failure (503)', async () => {
    mockFetch.mockResolvedValueOnce(new Response('SMTP not configured', { status: 503 }));
    const result = await auth.resendOtp('a@b.com', 'pw', 'A');
    expect(result).toBe(false);
    expect(auth.errorKey()).toBe('auth.error_smtp_unavailable');
  });

  it('returns false and sets error on network failure', async () => {
    mockFetch.mockRejectedValueOnce(new Error('offline'));
    const result = await auth.resendOtp('a@b.com', 'pw', 'A');
    expect(result).toBe(false);
    expect(auth.errorKey()).toBe('auth.error_connection_failed');
  });
});
