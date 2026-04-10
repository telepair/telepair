import { describe, it, expect, vi, beforeEach } from 'vitest';

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

const { auth, STORAGE_KEY } = await import('./auth');
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
