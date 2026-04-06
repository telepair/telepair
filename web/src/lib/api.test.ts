import { describe, it, expect, vi, beforeEach } from 'vitest';

const mockFetch = vi.fn();
vi.stubGlobal('fetch', mockFetch);

// Stub localStorage since jsdom's may not be fully functional
const store: Record<string, string> = {};
vi.stubGlobal('localStorage', {
  getItem: (key: string) => store[key] ?? null,
  setItem: (key: string, value: string) => { store[key] = value; },
  removeItem: (key: string) => { delete store[key]; },
});

// Import after stubs are in place
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
  delete store['telepair_token'];
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
    store['telepair_token'] = 'test-token';
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

describe('api.createSession', () => {
  it('sends POST with JSON body', async () => {
    store['telepair_token'] = 'tok';
    const session = { id: 'abc', target_name: 'shell', input_mode: 'serialized', status: 'active', owner_id: 'u1', created_at: '', closed_at: null };
    mockFetch.mockResolvedValueOnce(jsonResponse(session));

    const result = await api.createSession('shell');
    const [url, init] = mockFetch.mock.calls[0];
    expect(url).toBe('/api/sessions');
    expect(init.method).toBe('POST');
    expect(init.headers['Content-Type']).toBe('application/json');
    expect(JSON.parse(init.body)).toEqual({ target_name: 'shell', input_mode: undefined });
    expect(result.id).toBe('abc');
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

  it('does NOT invoke the handler on non-401 errors', async () => {
    const onExpired = vi.fn();
    __setAuthExpiredHandler(onExpired);
    mockFetch.mockResolvedValueOnce(errorResponse('boom', 500));

    await expect(api.listTargets()).rejects.toThrow(ApiError);
    expect(onExpired).not.toHaveBeenCalled();
  });
});
