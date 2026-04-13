import { describe, it, expect, vi, beforeEach } from 'vitest';

const mockFetch = vi.fn();
vi.stubGlobal('fetch', mockFetch);

const store: Record<string, string> = {};
vi.stubGlobal('localStorage', {
  getItem: (key: string) => store[key] ?? null,
  setItem: (key: string, value: string) => { store[key] = value; },
  removeItem: (key: string) => { delete store[key]; },
});

const { sessionStore } = await import('./session');

function jsonResponse(data: unknown, status = 200) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const fakeTargets = [
  { name: 'local-shell', display: 'Local Shell', tags: [] },
  { name: 'prod-db', display: 'Production DB', tags: ['database'] },
];

const fakeSession = {
  id: 'sess-1',
  owner_id: 'u1',
  target_name: 'local-shell',
  input_mode: 'serialized',
  status: 'active',
  created_at: '2026-04-04T12:00:00Z',
  closed_at: null,
};

beforeEach(() => {
  mockFetch.mockReset();
  store['telepair_token'] = 'test-token';
});

describe('sessionStore.fetchTargets', () => {
  it('fetches and stores targets', async () => {
    mockFetch.mockResolvedValueOnce(jsonResponse(fakeTargets));
    await sessionStore.fetchTargets();
    expect(sessionStore.targets()).toEqual(fakeTargets);
    expect(sessionStore.loading()).toBe(false);
  });

  it('sets loading during fetch', async () => {
    let resolvePromise: (v: Response) => void;
    const pending = new Promise<Response>((r) => { resolvePromise = r; });
    mockFetch.mockReturnValueOnce(pending);

    const p = sessionStore.fetchTargets();
    expect(sessionStore.loading()).toBe(true);

    resolvePromise!(jsonResponse(fakeTargets));
    await p;
    expect(sessionStore.loading()).toBe(false);
  });

  it('clears credentials and bounces to /login on 403 (scoped guest reaches dashboard)', async () => {
    // Regression for the QA-pass finding: a scoped guest token that
    // somehow reached the dashboard route used to trip the
    // `/api/targets` 403, fall through to the catch, and silently
    // render the empty-state — which both stranded the guest AND
    // leaked the server-side targets.yaml path. The contract: a 403
    // here clears the cached token and hard-redirects to /login.
    store['telepair_token'] = 'guest-token';
    mockFetch.mockResolvedValueOnce(new Response('forbidden', { status: 403 }));

    const assignSpy = vi.fn();
    const originalLocation = window.location;
    Object.defineProperty(window, 'location', {
      configurable: true,
      writable: true,
      value: { pathname: '/', assign: assignSpy },
    });

    try {
      await sessionStore.fetchTargets();
      // Token must be wiped from storage so a subsequent reload can't
      // resurrect the rejected session.
      expect(store['telepair_token']).toBeUndefined();
      // And we hard-bounce to /login (the API layer's other 401 path
      // does the same — this matches it for 403 on /targets).
      expect(assignSpy).toHaveBeenCalledWith('/login');
    } finally {
      Object.defineProperty(window, 'location', {
        configurable: true,
        writable: true,
        value: originalLocation,
      });
    }
  });

  it('does not redirect a 403 caller already on /login', async () => {
    // Edge case: if some code path triggers fetchTargets while the
    // user is already on /login (e.g. a stale background reactive
    // watcher), don't trample the page with a redundant assign() —
    // it would short-circuit any in-progress login attempt.
    store['telepair_token'] = 'guest-token';
    mockFetch.mockResolvedValueOnce(new Response('forbidden', { status: 403 }));

    const assignSpy = vi.fn();
    const originalLocation = window.location;
    Object.defineProperty(window, 'location', {
      configurable: true,
      writable: true,
      value: { pathname: '/login', assign: assignSpy },
    });

    try {
      await sessionStore.fetchTargets();
      expect(assignSpy).not.toHaveBeenCalled();
    } finally {
      Object.defineProperty(window, 'location', {
        configurable: true,
        writable: true,
        value: originalLocation,
      });
    }
  });
});

describe('sessionStore.fetchSessions', () => {
  it('fetches and stores sessions', async () => {
    mockFetch.mockResolvedValueOnce(jsonResponse([fakeSession]));
    await sessionStore.fetchSessions();
    expect(sessionStore.sessions()).toEqual([fakeSession]);
  });

  it('swallows errors silently', async () => {
    mockFetch.mockResolvedValueOnce(new Response('Error', { status: 500 }));
    // Should not throw
    await sessionStore.fetchSessions();
  });

  it('clears stale rows when a tab-switch fetch fails', async () => {
    // Regression for the v0.1.1-dev bug: `fetchSessions` used to
    // update `currentFilter` synchronously and then silently swallow
    // the fetch error, leaving the previous tab's rows under the
    // new tab. The fix drops the old rows on failure so the UI
    // renders an empty state rather than data from the wrong bucket.
    mockFetch.mockResolvedValueOnce(jsonResponse([fakeSession]));
    await sessionStore.fetchSessions('active');
    expect(sessionStore.sessions()).toEqual([fakeSession]);
    expect(sessionStore.currentFilter()).toBe('active');

    // Now switch to Closed and have the backend blow up.
    mockFetch.mockResolvedValueOnce(new Response('boom', { status: 500 }));
    await sessionStore.fetchSessions('closed');

    // The tab highlight follows user intent — they clicked Closed,
    // so that's what the chip shows.
    expect(sessionStore.currentFilter()).toBe('closed');
    // But we must NOT leave the active rows visible under Closed.
    expect(sessionStore.sessions()).toEqual([]);

    // Reset the filter back to 'active' so sibling tests in this
    // file (which share a singleton sessionStore) inherit the
    // default tab state.
    mockFetch.mockResolvedValueOnce(jsonResponse([]));
    await sessionStore.fetchSessions('active');
  });
});

const fakeBetaSession = {
  id: 'sess-2',
  owner_id: 'u1',
  target_name: 'prod-db',
  input_mode: 'serialized',
  status: 'active',
  created_at: '2026-04-04T12:01:00Z',
  closed_at: null,
};

// Helper builders so the createSession tests below stay readable. The
// store now takes a full TargetInfo so the api layer can pick the
// right namespace field — these mirror what `listTargets` returns.
const globalTarget = (name: string) => ({
  name,
  display: name,
  tags: [],
  source: 'global' as const,
  admin_only: false,
});
const userTarget = (name: string, id: string) => ({
  name,
  display: name,
  tags: [],
  source: 'user' as const,
  id,
  admin_only: false,
});

describe('sessionStore.createSession', () => {
  it('creates session and appends to list when no target filter', async () => {
    // Clear sessions first
    mockFetch.mockResolvedValueOnce(jsonResponse([]));
    await sessionStore.fetchSessions();

    mockFetch.mockResolvedValueOnce(jsonResponse(fakeSession, 201));
    const result = await sessionStore.createSession(globalTarget('local-shell'));
    expect(result.id).toBe('sess-1');
    expect(sessionStore.sessions()).toContainEqual(fakeSession);
  });

  it('appends new session when its target matches the active filter', async () => {
    // Filter is alpha (local-shell); creating a local-shell session
    // should still appear immediately.
    mockFetch.mockResolvedValueOnce(jsonResponse([]));
    await sessionStore.fetchSessions('active', 'local-shell');

    mockFetch.mockResolvedValueOnce(jsonResponse(fakeSession, 201));
    await sessionStore.createSession(globalTarget('local-shell'));
    expect(sessionStore.sessions()).toContainEqual(fakeSession);
  });

  it('does NOT append new session when its target does not match the active filter', async () => {
    // L1 regression: if the dashboard is filtered to ?target=local-shell
    // and the user somehow creates a `prod-db` session (possible if they
    // use the session store directly or the filter was recently changed),
    // the new row must not flash into the filtered list. It would vanish
    // on the next refetch anyway, but the premature insertion is
    // confusing. The fix checks currentTargetFilter() in createSession.
    mockFetch.mockResolvedValueOnce(jsonResponse([fakeSession]));
    await sessionStore.fetchSessions('active', 'local-shell');

    mockFetch.mockResolvedValueOnce(jsonResponse(fakeBetaSession, 201));
    await sessionStore.createSession(globalTarget('prod-db'));

    // List must still contain only the alpha session from the fetch.
    const ids = sessionStore.sessions().map((s) => s.id);
    expect(ids).toContain('sess-1');
    expect(ids).not.toContain('sess-2');
  });

  it('addresses user-owned targets by target_id, not target_name', async () => {
    // Regression guard for Fix #2 collision-shadowing: a user-owned
    // target with the same `name` as a global one MUST round-trip
    // through the store as `target_id`, otherwise the store would
    // serialise the name and the backend would launch the global
    // target. The store passes the full TargetInfo to api.createSession,
    // and api.ts does the namespace pick — this test pins both halves.
    mockFetch.mockResolvedValueOnce(jsonResponse([]));
    await sessionStore.fetchSessions();

    const userVps = {
      id: 'sess-3',
      owner_id: 'u1',
      target_name: 'vps',
      input_mode: 'multiplexed',
      status: 'active',
      created_at: '2026-04-04T12:02:00Z',
      closed_at: null,
      user_target_id: 'nano-1',
    };
    mockFetch.mockResolvedValueOnce(jsonResponse(userVps, 201));
    await sessionStore.createSession(userTarget('vps', 'nano-1'));

    const lastCall = mockFetch.mock.calls[mockFetch.mock.calls.length - 1];
    const body = JSON.parse((lastCall[1] as RequestInit).body as string);
    expect(body).toEqual({ target_id: 'nano-1' });
    expect(body.target_name).toBeUndefined();
  });
});

describe('sessionStore.refresh', () => {
  it('fetches both targets and sessions in parallel', async () => {
    mockFetch
      .mockResolvedValueOnce(jsonResponse(fakeTargets))
      .mockResolvedValueOnce(jsonResponse([fakeSession]));

    await sessionStore.refresh();

    expect(sessionStore.targets()).toEqual(fakeTargets);
    expect(sessionStore.sessions()).toEqual([fakeSession]);
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });
});
