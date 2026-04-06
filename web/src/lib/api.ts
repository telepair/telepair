// web/src/lib/api.ts
import type { TargetInfo, Session, InviteInfo, RedeemResult, Role, InputMode } from './protocol';
import { STORAGE_KEY } from '../stores/auth';

const BASE = '/api';

class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
    this.name = 'ApiError';
  }
}

/**
 * Endpoints that may be called WITHOUT a bearer token (or whose 401
 * must not trigger a global logout/redirect). The redeem endpoint is
 * the obvious one — the whole point of the new invite flow is that
 * guests hit it anonymously — but anything added here should be the
 * exception, not the rule.
 */
const PUBLIC_PATHS = new Set<string>(['/invite/redeem']);

/**
 * Called from the request helper when the server returns 401 on an
 * authenticated endpoint. This is the single place responsible for
 * recovering from a stale token: drop the cached credential and push
 * the user back to the login screen instead of letting them stare at
 * a broken dashboard (which is what used to happen — a saved-but-
 * expired token left the dashboard rendering "No targets available"
 * and hid the real auth failure).
 *
 * Exported as a swap point so tests can stub navigation without
 * touching window.location directly.
 */
export let handleAuthExpired: () => void = () => {
  try {
    localStorage.removeItem(STORAGE_KEY);
  } catch {
    // ignore — private-mode / storage quota is not actionable here
  }
  // Use a hard navigation instead of the router: by the time we
  // notice a stale token the reactive auth store is still
  // advertising the old value, and SolidJS routes won't re-run
  // AuthGuard unless we force a reload.
  if (typeof window !== 'undefined' && window.location.pathname !== '/login') {
    window.location.assign('/login');
  }
};

/** Test hook — lets Vitest override the expiry handler. */
export function __setAuthExpiredHandler(fn: () => void) {
  handleAuthExpired = fn;
}

async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const token = localStorage.getItem(STORAGE_KEY);
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
    // 401 on a protected endpoint = the cached token is bad. Kick
    // the user out before surfacing the error so the UI can't linger
    // in a broken state. PUBLIC_PATHS opts out of this — redeeming
    // an invite anonymously is not an auth failure.
    if (resp.status === 401 && !PUBLIC_PATHS.has(path)) {
      handleAuthExpired();
    }
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

  createSession(target_name: string, input_mode?: InputMode): Promise<Session> {
    return request('/sessions', {
      method: 'POST',
      body: JSON.stringify({ target_name, input_mode }),
    });
  },

  createInvite(sessionId: string, role: Role, maxUses?: number): Promise<InviteInfo> {
    return request(`/sessions/${sessionId}/invite`, {
      method: 'POST',
      body: JSON.stringify({ role, max_uses: maxUses ?? 1 }),
    });
  },

  redeemInvite(token: string): Promise<RedeemResult> {
    return request('/invite/redeem', {
      method: 'POST',
      body: JSON.stringify({ token }),
    });
  },
};

export { ApiError };
