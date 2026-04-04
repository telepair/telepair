// web/src/lib/api.ts
import type { TargetInfo, Session } from './protocol';

const BASE = '/api';

class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
    this.name = 'ApiError';
  }
}

async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const token = localStorage.getItem('telepair_token');
  const headers: Record<string, string> = {
    ...options.headers as Record<string, string>,
  };
  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  }
  if (options.body && typeof options.body === 'string') {
    headers['Content-Type'] = 'application/json';
  }

  const resp = await fetch(`${BASE}${path}`, { ...options, headers });

  if (!resp.ok) {
    throw new ApiError(resp.status, await resp.text());
  }

  return resp.json();
}

export const api = {
  health(): Promise<{ status: string }> {
    return request('/health');
  },

  listTargets(): Promise<TargetInfo[]> {
    return request('/targets');
  },

  listSessions(): Promise<Session[]> {
    return request('/sessions');
  },

  createSession(target_name: string, input_mode?: string): Promise<Session> {
    return request('/sessions', {
      method: 'POST',
      body: JSON.stringify({ target_name, input_mode }),
    });
  },
};

export { ApiError };
