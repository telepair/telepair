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

const { auth, STORAGE_KEY, readCurrentToken } = await import('./auth');
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

describe('readCurrentToken in storage-restricted environments', () => {
  // Regression for the storage-failure path Codex flagged: in private
  // browsing / sandboxed iframes / quota-exhausted contexts, `setItem`
  // throws and `safeSet` swallows the error. The signal still gets the
  // new token, so the UI thinks login succeeded — but if
  // `readCurrentToken` only reads storage, the API layer drops the
  // `Authorization` header on every follow-up request and the user
  // sees an inexplicable wall of 401s. The fallback below makes the
  // signal authoritative when storage is unwritable.
  it('falls back to the in-memory signal when storage writes silently fail', () => {
    storageWritesThrow = true;

    auth.setToken('memory-only-token');

    // Both stores stayed empty — `setItem` always threw — but the
    // signal captured the new token, and `readCurrentToken` should
    // surface it instead of returning ''.
    expect(tabStore[STORAGE_KEY]).toBeUndefined();
    expect(persistStore[STORAGE_KEY]).toBeUndefined();
    expect(auth.token()).toBe('memory-only-token');
    expect(readCurrentToken()).toBe('memory-only-token');
  });

  it('still prefers sessionStorage when both storage and signal hold a value', () => {
    // Sanity check: the fallback must not regress the existing
    // sessionStorage-first ordering, which is load-bearing for the
    // cross-tab isolation guarantee.
    auth.setToken('signal-token');
    tabStore[STORAGE_KEY] = 'tab-token';
    expect(readCurrentToken()).toBe('tab-token');
  });
});
