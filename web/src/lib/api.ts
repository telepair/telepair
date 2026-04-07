// web/src/lib/api.ts
import type { TargetInfo, Session, InviteInfo, RedeemResult, Role, InputMode } from './protocol';
import { auth, readCurrentToken } from '../stores/auth';

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
 * authenticated endpoint. Delegates to `auth.logoutAndRedirect`, which
 * is the single place responsible for "drop the cached credential and
 * push the user back to /login" — the previous inline implementation
 * here had drifted from the dashboard 403 path and the Session.tsx
 * non-owner exit, leaving each call site with a slightly different
 * guard around `window.location.assign`.
 *
 * Still exported as a swap point so tests can stub navigation without
 * touching window.location directly.
 */
export let handleAuthExpired: () => void = () => {
  auth.logoutAndRedirect();
};

/** Test hook — lets Vitest override the expiry handler. */
export function __setAuthExpiredHandler(fn: () => void) {
  handleAuthExpired = fn;
}

async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  // Always read through the auth store's helper so the sessionStorage
  // tier wins over the persistent admin fallback. Reading localStorage
  // directly here would let a tab with a guest sessionStorage entry
  // get the admin token stamped onto its requests — the cross-tab
  // identity-bleed bug the two-tier storage is designed to prevent.
  const token = readCurrentToken();
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

  /**
   * Create an invite token. `maxUses` defaults to 1 (one-shot link);
   * `expiresInMinutes` defaults to undefined (no TTL → only bounded
   * by `maxUses` and session lifetime). The server enforces hard caps
   * (`max_uses ≤ 100`, TTL ≤ 7 days) and 400s anything beyond them.
   */
  createInvite(
    sessionId: string,
    role: Role,
    opts: { maxUses?: number; expiresInMinutes?: number } = {},
  ): Promise<InviteInfo> {
    return request(`/sessions/${sessionId}/invite`, {
      method: 'POST',
      body: JSON.stringify({
        role,
        max_uses: opts.maxUses ?? 1,
        expires_in_minutes: opts.expiresInMinutes,
      }),
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
