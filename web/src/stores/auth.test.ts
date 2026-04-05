import { describe, it, expect, vi, beforeEach } from 'vitest';

const mockFetch = vi.fn();
vi.stubGlobal('fetch', mockFetch);

const store: Record<string, string> = {};
vi.stubGlobal('localStorage', {
  getItem: (key: string) => store[key] ?? null,
  setItem: (key: string, value: string) => { store[key] = value; },
  removeItem: (key: string) => { delete store[key]; },
});

const { auth, STORAGE_KEY } = await import('./auth');

function jsonResponse(data: unknown, status = 200) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

beforeEach(() => {
  mockFetch.mockReset();
  delete store[STORAGE_KEY];
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
  it('persists token to localStorage', () => {
    auth.setToken('my-token');
    expect(store[STORAGE_KEY]).toBe('my-token');
    expect(auth.token()).toBe('my-token');
  });

  it('removes from localStorage when empty', () => {
    auth.setToken('temp');
    auth.setToken('');
    expect(store[STORAGE_KEY]).toBeUndefined();
    expect(auth.token()).toBe('');
  });

  it('clears error', () => {
    // Force an error state first
    mockFetch.mockResolvedValueOnce(new Response('Unauthorized', { status: 401 }));
    auth.validateToken('bad').then(() => {
      auth.setToken('new');
      expect(auth.error()).toBe('');
    });
  });
});

describe('auth.validateToken', () => {
  it('returns true and persists on valid token', async () => {
    mockFetch.mockResolvedValueOnce(jsonResponse([]));
    const result = await auth.validateToken('good-token');
    expect(result).toBe(true);
    expect(auth.token()).toBe('good-token');
    expect(store[STORAGE_KEY]).toBe('good-token');
    expect(auth.error()).toBe('');
    expect(auth.validating()).toBe(false);
  });

  it('returns false and sets "Invalid token" on 401', async () => {
    mockFetch.mockResolvedValueOnce(new Response('Unauthorized', { status: 401 }));
    const result = await auth.validateToken('bad-token');
    expect(result).toBe(false);
    expect(auth.token()).toBe('');
    expect(auth.error()).toBe('Invalid token');
    expect(auth.validating()).toBe(false);
  });

  it('returns false with "Connection failed" on network error', async () => {
    mockFetch.mockRejectedValueOnce(new Error('Network error'));
    const result = await auth.validateToken('any-token');
    expect(result).toBe(false);
    expect(auth.error()).toBe('Connection failed');
  });

  it('removes token from localStorage on failure', async () => {
    mockFetch.mockResolvedValueOnce(new Response('Unauthorized', { status: 401 }));
    await auth.validateToken('will-fail');
    expect(store[STORAGE_KEY]).toBeUndefined();
  });
});

describe('auth.logout', () => {
  it('clears token and localStorage', () => {
    auth.setToken('active-token');
    auth.logout();
    expect(auth.token()).toBe('');
    expect(auth.isAuthenticated()).toBe(false);
    expect(store[STORAGE_KEY]).toBeUndefined();
  });
});
