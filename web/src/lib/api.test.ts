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
const { api, ApiError } = await import('./api');

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
