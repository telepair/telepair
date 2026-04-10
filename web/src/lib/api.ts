// web/src/lib/api.ts
import type {
  TargetInfo,
  Session,
  SessionStatus,
  InviteInfo,
  InviteSummary,
  AuditEvent,
  RedeemResult,
  Role,
  InputMode,
  AdminTargetInfo,
  ReloadTargetsResult,
} from './protocol';

/**
 * Optional filter for `api.listSessions`. Introduced alongside the
 * v0.1.1 session-history view so the dashboard can toggle
 * Active / Closed / All without the server returning the whole
 * history on every refresh.
 *
 * Omit `status` (or pass `'all'`) to get both active and closed rows.
 * `targetName` narrows the result to a single virtual target — used by
 * the "N active sessions" deep link from the admin targets page.
 */
export interface ListSessionsOptions {
  status?: SessionStatus | 'all';
  targetName?: string;
  limit?: number;
  offset?: number;
}
import { auth } from '../stores/auth';

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
  // Read the in-memory signal — the single source of truth for the
  // current tab's credential. The signal is primed at module init from
  // sessionStorage/localStorage (via readInitialToken) and updated
  // synchronously by setToken(), so it is always correct even when
  // storage writes fail (private browsing, quota exceeded, sandboxed
  // iframes). Using the signal here keeps REST and WebSocket identity
  // in lockstep — both read auth.token().
  const token = auth.token();
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

  // 204 No Content (DELETE endpoints) has no body — calling
  // `resp.json()` on it would throw SyntaxError. Return undefined
  // cast to T so `Promise<void>` callers work without a special case.
  if (resp.status === 204) {
    return undefined as T;
  }

  return resp.json();
}

/**
 * Caller identity returned by `GET /api/auth/whoami`. The auth store
 * caches `user_id` so the dashboard can compare it against
 * `session.owner_id` when deciding whether to open the owner-only
 * audit dialog — without this comparison, non-owner participants would
 * see closed rows in their history list (since `list_sessions_for_user`
 * returns "owned OR joined") and clicking through would deterministi
 * cally hit a 403 from `/audit`.
 */
export interface WhoamiResponse {
  user_id: string;
  name: string;
  is_admin: boolean;
  is_guest: boolean;
}

export const api = {
  health(): Promise<{ status: string }> {
    return request('/health');
  },

  /**
   * Read the authenticated caller's identity. Returns 401 on missing
   * or invalid bearer (which the global interceptor turns into a
   * /login bounce); never 403, since "I am a guest" is still a valid
   * identity to surface.
   */
  whoami(): Promise<WhoamiResponse> {
    return request('/auth/whoami');
  },

  listTargets(): Promise<TargetInfo[]> {
    return request('/targets');
  },

  /**
   * List sessions visible to the caller. The server decides the
   * visibility: regular users see the sessions they own or are a
   * participant of; admins see everything. Without any options this
   * returns the legacy "active only" list — we keep that as the
   * default so all the pre-0.1.1 callers (terminal banner, reconnect
   * logic, etc.) carry on unchanged.
   */
  listSessions(opts: ListSessionsOptions = {}): Promise<Session[]> {
    const params = new URLSearchParams();
    // `all` maps to "no filter" — the server treats an absent status
    // as "all statuses", so we deliberately skip the param in that
    // case rather than forwarding the literal string.
    if (opts.status && opts.status !== 'all') {
      params.set('status', opts.status);
    }
    if (opts.targetName) {
      params.set('target_name', opts.targetName);
    }
    if (opts.limit != null) {
      params.set('limit', String(opts.limit));
    }
    if (opts.offset != null) {
      params.set('offset', String(opts.offset));
    }
    const qs = params.toString();
    return request(qs ? `/sessions?${qs}` : '/sessions');
  },

  createSession(target_name: string, input_mode?: InputMode): Promise<Session> {
    return request('/sessions', {
      method: 'POST',
      body: JSON.stringify({ target_name, input_mode }),
    });
  },

  /**
   * Owner-only. Closes the session: server stops the PTY, broadcasts
   * `SESSION_CLOSED` to all participants, and flips the row to
   * `closed` in storage. Returns 204 No Content on success — the
   * handler throws on any non-2xx, so the caller can `await` it and
   * rely on an exception for failures.
   */
  closeSession(sessionId: string): Promise<void> {
    return request(`/sessions/${sessionId}`, { method: 'DELETE' });
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
    return request(`/sessions/${sessionId}/invites`, {
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

  /**
   * Owner-only. List every invite ever minted for `sessionId`, active
   * and expired/exhausted alike — the management dialog wants the full
   * history and renders state chips per row. The response deliberately
   * omits raw bearer tokens; callers label rows by `token_prefix` (8
   * hex chars) and revoke by `token_sha256`.
   */
  listInvites(sessionId: string): Promise<InviteSummary[]> {
    return request(`/sessions/${sessionId}/invites`);
  },

  /**
   * Owner-only. Hard-delete an invite row by its SHA-256 digest.
   * Returns 204 on success, 400 on "already gone", 403 when the
   * caller does not own the session. The UI refreshes its list on
   * every response so a concurrent revoke from another owner tab
   * converges on its own.
   */
  revokeInvite(sessionId: string, tokenSha256: string): Promise<void> {
    return request(`/sessions/${sessionId}/invites/${tokenSha256}`, {
      method: 'DELETE',
    });
  },

  /**
   * Owner-only. Read the audit timeline for `sessionId`, newest first.
   * The endpoint stays open after the session is closed (the whole
   * point of the history view is reading what happened on a closed
   * session), so the caller does not need to special-case status
   * transitions. Capped server-side at 500 rows; pagination will land
   * if a real session ever needs it.
   */
  listSessionAudit(sessionId: string): Promise<AuditEvent[]> {
    return request(`/sessions/${sessionId}/audit`);
  },

  /**
   * Admin-only. Full target list with command / args / env presence
   * and per-target active session counts. Env values are never
   * returned by the server — see `AdminTargetEnvKey` for the security
   * rationale. Regular users get 403; unauthenticated calls get 401
   * and the global interceptor bounces them to /login.
   */
  listAdminTargets(): Promise<AdminTargetInfo[]> {
    return request('/admin/targets');
  },

  /**
   * Admin-only. Re-read `targets.yaml` from disk and atomically swap
   * the in-memory target engine. Three failure modes the caller must
   * distinguish — the server returns structured JSON on 400 rather
   * than a bare HTTP status so the UI can pick the right message:
   *
   * - 400 `reason=no_targets_path`: operator never configured a yaml
   *   file. The UI should tell them to set `--targets` and restart.
   * - 400 `reason=parse_error`: the file on disk is malformed now.
   *   The UI should surface the parse error verbatim so they can fix
   *   it without opening a terminal.
   * - 200: the engine was swapped. The old pointer is dropped lazily
   *   by ArcSwap once all in-flight readers release it.
   *
   * The ApiError thrown for 400/403/401 carries the raw response body
   * as its message; the AdminTargets page parses it and picks the
   * right toast. We keep this helper thin (no error shape transform)
   * so the caller retains full control over the branching.
   */
  reloadTargets(): Promise<ReloadTargetsResult> {
    return request('/admin/targets/reload', { method: 'POST' });
  },
};

export { ApiError };

/**
 * Extract a user-facing message string from an unknown thrown value.
 * Centralised here because every `catch (e)` site used to inline the
 * same `e instanceof Error ? e.message : String(e)` dance — five or
 * six copies that were one `toString()`-vs-`String()` typo away from
 * silently drifting. `ApiError.message` carries the server's error
 * body (see `request()` above), which is already the right thing to
 * show in a toast, so no special-casing is needed here.
 */
export function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
