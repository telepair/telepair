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
});

describe('sessionStore.createSession', () => {
  it('creates session and appends to list', async () => {
    // Clear sessions first
    mockFetch.mockResolvedValueOnce(jsonResponse([]));
    await sessionStore.fetchSessions();

    mockFetch.mockResolvedValueOnce(jsonResponse(fakeSession, 201));
    const result = await sessionStore.createSession('local-shell');
    expect(result.id).toBe('sess-1');
    expect(sessionStore.sessions()).toContainEqual(fakeSession);
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
