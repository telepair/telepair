import { describe, it, expect, vi, beforeEach } from 'vitest';

const mockFetch = vi.fn();
vi.stubGlobal('fetch', mockFetch);

// Stub both storage tiers. The auth store reads sessionStorage first
// (per-tab identity) and falls back to localStorage (persistent admin
// fallback) during signal initialisation. api.ts reads the in-memory
// signal directly (auth.token()), so the test seeds storage to prime
// the signal at import time. Tests that want the "signed in as admin"
// state write to `store` (localStorage) because that's where login
// persists; tests that want tab-scoped state write to `tabStore`.
const store: Record<string, string> = {};
const tabStore: Record<string, string> = {};
vi.stubGlobal('localStorage', {
  getItem: (key: string) => store[key] ?? null,
  setItem: (key: string, value: string) => { store[key] = value; },
  removeItem: (key: string) => { delete store[key]; },
});
vi.stubGlobal('sessionStorage', {
  getItem: (key: string) => tabStore[key] ?? null,
  setItem: (key: string, value: string) => { tabStore[key] = value; },
  removeItem: (key: string) => { delete tabStore[key]; },
});

// Import after stubs are in place
const { auth } = await import('../stores/auth');
const { api, ApiError, __setAuthExpiredHandler } = await import('./api');

function jsonResponse(data: unknown, status = 200) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

function errorResponse(body: string, status: number) {
  return new Response(body, { status });
}

beforeEach(() => {
  mockFetch.mockReset();
  auth.setToken('');
  delete store['telepair_token'];
  delete tabStore['telepair_token'];
  // Replace the default handler (which tries to navigate via
  // window.location.assign and trips jsdom's "navigation not
  // implemented" noise) with a no-op so each test that cares can
  // override it explicitly. Tests below that need to observe the
  // handler being called install their own spy.
  __setAuthExpiredHandler(() => {});
});

describe('api.health', () => {
  it('calls GET /api/health', async () => {
    mockFetch.mockResolvedValueOnce(jsonResponse({ status: 'ok' }));
    const result = await api.health();
    expect(result).toEqual({ status: 'ok' });
    expect(mockFetch).toHaveBeenCalledWith('/api/health', expect.objectContaining({}));
  });
});

describe('api.listTargets', () => {
  it('sends Authorization header when token exists', async () => {
    auth.setToken('test-token');
    mockFetch.mockResolvedValueOnce(jsonResponse([]));
    await api.listTargets();
    const [, init] = mockFetch.mock.calls[0];
    expect(init.headers['Authorization']).toBe('Bearer test-token');
  });

  it('omits Authorization header when no token', async () => {
    mockFetch.mockResolvedValueOnce(jsonResponse([]));
    await api.listTargets();
    const [, init] = mockFetch.mock.calls[0];
    expect(init.headers['Authorization']).toBeUndefined();
  });
});

describe('api.listSessions filter', () => {
  it('omits query string when no filter is passed', async () => {
    mockFetch.mockResolvedValueOnce(jsonResponse([]));
    await api.listSessions();
    expect(mockFetch.mock.calls[0][0]).toBe('/api/sessions');
  });

  it('emits status query param for a specific tab', async () => {
    mockFetch.mockResolvedValueOnce(jsonResponse([]));
    await api.listSessions({ status: 'closed' });
    expect(mockFetch.mock.calls[0][0]).toBe('/api/sessions?status=closed');
  });

  it("treats status='all' as no status param (server default)", async () => {
    // The backend handler's `ListSessionsQuery::into_filter` maps an
    // absent status to "all statuses" — forwarding the literal string
    // `all` would fail the handler's case-sensitive match. Keeping
    // this mapping in one place (the api layer) means the rest of the
    // app can still speak the readable 'all' | 'active' | 'closed'
    // vocabulary.
    mockFetch.mockResolvedValueOnce(jsonResponse([]));
    await api.listSessions({ status: 'all' });
    expect(mockFetch.mock.calls[0][0]).toBe('/api/sessions');
  });

  it('encodes target_name and pagination when present', async () => {
    mockFetch.mockResolvedValueOnce(jsonResponse([]));
    await api.listSessions({
      status: 'active',
      targetName: 'local shell',
      limit: 50,
      offset: 25,
    });
    const url = mockFetch.mock.calls[0][0] as string;
    expect(url).toContain('status=active');
    expect(url).toContain('target_name=local+shell');
    expect(url).toContain('limit=50');
    expect(url).toContain('offset=25');
  });
});

describe('api.createSession', () => {
  it('serializes a global target as target_name', async () => {
    store['telepair_token'] = 'tok';
    const session = { id: 'abc', target_name: 'shell', input_mode: 'multiplexed', status: 'active', owner_id: 'u1', created_at: '', closed_at: null };
    mockFetch.mockResolvedValueOnce(jsonResponse(session));

    const result = await api.createSession({
      name: 'shell',
      display: 'Shell',
      tags: [],
      source: 'global',
      admin_only: false,
    });
    const [url, init] = mockFetch.mock.calls[0];
    expect(url).toBe('/api/sessions');
    expect(init.method).toBe('POST');
    expect(init.headers['Content-Type']).toBe('application/json');
    // Body MUST contain target_name and MUST NOT contain target_id —
    // this is the on-wire shape the new Rust handler asserts on, and
    // the namespace contract that prevents collision-shadowing.
    expect(JSON.parse(init.body)).toEqual({ target_name: 'shell' });
    expect(result.id).toBe('abc');
  });

  it('serializes a user target as target_id (never target_name)', async () => {
    // Regression guard for the collision bug: even when the user-owned
    // target shares its `name` with a global target, the on-wire body
    // MUST address the row by stable nanoid so the backend resolves
    // user-target storage and never the global engine. A future
    // refactor that quietly forwards `target.name` instead would put
    // us right back in the v0.1.1 collision-shadowing failure mode.
    store['telepair_token'] = 'tok';
    const session = { id: 'abc', target_name: 'vps', input_mode: 'multiplexed', status: 'active', owner_id: 'u1', created_at: '', closed_at: null, user_target_id: 'nano-1' };
    mockFetch.mockResolvedValueOnce(jsonResponse(session));

    await api.createSession({
      name: 'vps',
      display: 'My VPS',
      tags: [],
      source: 'user',
      id: 'nano-1',
      admin_only: false,
    });
    const [, init] = mockFetch.mock.calls[0];
    const body = JSON.parse(init.body);
    expect(body).toEqual({ target_id: 'nano-1' });
    expect(body.target_name).toBeUndefined();
  });

  it('passes input_mode through when set', async () => {
    store['telepair_token'] = 'tok';
    const session = { id: 'abc', target_name: 'shell', input_mode: 'serialized', status: 'active', owner_id: 'u1', created_at: '', closed_at: null };
    mockFetch.mockResolvedValueOnce(jsonResponse(session));

    await api.createSession(
      { name: 'shell', display: 'Shell', tags: [], source: 'global', admin_only: false },
      'serialized',
    );
    const [, init] = mockFetch.mock.calls[0];
    expect(JSON.parse(init.body)).toEqual({ target_name: 'shell', input_mode: 'serialized' });
  });
});

describe('error handling', () => {
  it('throws ApiError on non-ok response', async () => {
    mockFetch.mockResolvedValueOnce(errorResponse('Unauthorized', 401));
    await expect(api.listTargets()).rejects.toThrow(ApiError);
  });

  it('includes status code in ApiError', async () => {
    mockFetch.mockResolvedValueOnce(errorResponse('Not Found', 404));
    try {
      await api.listSessions();
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      expect((e as InstanceType<typeof ApiError>).status).toBe(404);
    }
  });

  it('extracts the `error` field from a JSON error body', async () => {
    // The gateway wraps ApiError bodies as `{"error": "..."}`. The
    // request helper must unwrap that so toasts show a clean message
    // instead of the raw JSON blob.
    mockFetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ error: 'Current password is incorrect.' }), {
        status: 401,
        headers: { 'Content-Type': 'application/json' },
      }),
    );
    try {
      await api.changePassword('wrong', 'newpw-12345');
      expect.fail('expected ApiError');
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      expect((e as InstanceType<typeof ApiError>).message).toBe(
        'Current password is incorrect.',
      );
    }
  });

  it('preserves richer JSON bodies (e.g. reload structured errors) as raw text', async () => {
    // `POST /admin/targets/reload` returns `{reason, message, targets}`
    // on 4xx; `AdminTargets.parseReloadError` relies on the raw body
    // reaching `ApiError.message` so it can route by `reason`.
    const body = {
      reason: 'still_referenced',
      message: 'refusing to drop targets with live sessions',
      targets: [{ target: 'db', active_sessions: 2 }],
    };
    mockFetch.mockResolvedValueOnce(
      new Response(JSON.stringify(body), {
        status: 400,
        headers: { 'Content-Type': 'application/json' },
      }),
    );
    try {
      await api.reloadTargets('deadbeef');
      expect.fail('expected ApiError');
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      expect(JSON.parse((e as InstanceType<typeof ApiError>).message)).toEqual(body);
    }
  });
});

describe('401 auth-expired interceptor', () => {
  it('invokes the stale-token handler on 401 from a protected endpoint', async () => {
    const onExpired = vi.fn();
    __setAuthExpiredHandler(onExpired);
    mockFetch.mockResolvedValueOnce(errorResponse('Unauthorized', 401));

    await expect(api.listTargets()).rejects.toThrow(ApiError);
    expect(onExpired).toHaveBeenCalledTimes(1);
  });

  it('still throws ApiError alongside the handler call (callers must not silently swallow)', async () => {
    __setAuthExpiredHandler(() => {});
    mockFetch.mockResolvedValueOnce(errorResponse('Unauthorized', 401));

    try {
      await api.listSessions();
      expect.fail('expected ApiError');
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      expect((e as InstanceType<typeof ApiError>).status).toBe(401);
    }
  });

  it('does NOT invoke the handler on a 401 from the redeem endpoint', async () => {
    // The redeem endpoint is anonymous-friendly: an empty cached
    // token produces a 401 (rare) without meaning the *user* is
    // logged out of another session. Guests must not be yanked to
    // /login from a half-completed redeem.
    const onExpired = vi.fn();
    __setAuthExpiredHandler(onExpired);
    mockFetch.mockResolvedValueOnce(errorResponse('Unauthorized', 401));

    await expect(api.redeemInvite('some-invite-token')).rejects.toThrow(ApiError);
    expect(onExpired).not.toHaveBeenCalled();
  });

  it('does NOT invoke the handler on a 401 from change-password (wrong current password is not session expiry)', async () => {
    auth.setToken('valid-token');
    const onExpired = vi.fn();
    __setAuthExpiredHandler(onExpired);
    mockFetch.mockResolvedValueOnce(errorResponse('Current password is incorrect.', 401));

    await expect(
      api.changePassword('wrong-pw', 'new-pw-12345'),
    ).rejects.toThrow(ApiError);
    expect(onExpired).not.toHaveBeenCalled();
  });

  it('does NOT invoke the handler on non-401 errors', async () => {
    const onExpired = vi.fn();
    __setAuthExpiredHandler(onExpired);
    mockFetch.mockResolvedValueOnce(errorResponse('boom', 500));

    await expect(api.listTargets()).rejects.toThrow(ApiError);
    expect(onExpired).not.toHaveBeenCalled();
  });
});

describe('admin audit export', () => {
  it('auditExportPath encodes filters into the query string', async () => {
    const path = api.auditExportPath('csv', {
      event_type: 'SessionCreated',
      session_id: 'sess-1',
      since: '2026-01-01T00:00:00Z',
    });
    expect(path).toContain('/admin/audit/export?');
    expect(path).toContain('format=csv');
    expect(path).toContain('event_type=SessionCreated');
    expect(path).toContain('session_id=sess-1');
    expect(path).toContain('since=2026-01-01T00%3A00%3A00Z');
  });

  it('downloadBlob attaches the bearer token and returns blob + filename', async () => {
    auth.setToken('admin-token');
    // Pass the CSV as a string rather than a Blob: Node 22's undici
    // Response constructor resolves Blob bodies via `body.stream()`,
    // which vitest's happy-dom Blob polyfill does not implement. The
    // production path still receives a Blob because `response.blob()`
    // reconstitutes one from the bytes.
    mockFetch.mockResolvedValueOnce(
      new Response('id,ts\n1,x\n', {
        status: 200,
        headers: {
          'Content-Type': 'text/csv',
          'Content-Disposition': 'attachment; filename="telepair-audit-2026.csv"',
        },
      }),
    );

    const path = api.auditExportPath('csv');
    const { blob, filename } = await api.downloadBlob(path);

    const [url, init] = mockFetch.mock.calls[0];
    expect(url).toBe(`/api${path}`);
    expect(init.headers['Authorization']).toBe('Bearer admin-token');
    expect(filename).toBe('telepair-audit-2026.csv');
    // Duck-type check rather than `instanceof Blob`: on Node 22 the
    // Blob returned by undici's Response.blob() belongs to a different
    // realm than happy-dom's global Blob, so the instance check fails
    // even though the object is a Blob for all practical purposes.
    expect(blob.type).toBe('text/csv');
    expect(blob.size).toBeGreaterThan(0);
    expect(typeof blob.arrayBuffer).toBe('function');
  });

  it('downloadBlob trips the global 401 interceptor (regression)', async () => {
    // Previously AdminAudit.tsx bypassed the shared interceptor with a
    // raw `fetch()`; a token expiry on export left the dashboard in a
    // stale-logged-in state until the next api.* call tripped the
    // interceptor. downloadBlob now routes through the same authedFetch
    // helper as JSON requests, so a 401 from an authenticated download
    // path must invoke handleAuthExpired exactly once.
    auth.setToken('expired-token');
    const onExpired = vi.fn();
    __setAuthExpiredHandler(onExpired);
    mockFetch.mockResolvedValueOnce(errorResponse('Unauthorized', 401));

    await expect(
      api.downloadBlob(api.auditExportPath('json')),
    ).rejects.toThrow(ApiError);
    expect(onExpired).toHaveBeenCalledTimes(1);
  });
});
