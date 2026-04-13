// TypeScript types mirroring crates/telepair-core/src/protocol.rs

// Stable error codes carried by ServerMessage.Error. Keep in sync with
// `pub mod error_codes` in crates/telepair-core/src/protocol.rs — a typo
// on either side silently degrades UX.
export const ErrorCode = {
  AUTH_FAILED: 'AUTH_FAILED',
  AUTH_TIMEOUT: 'AUTH_TIMEOUT',
  EXPECTED_JOIN: 'EXPECTED_JOIN',
  SESSION_NOT_FOUND: 'SESSION_NOT_FOUND',
  SESSION_CLOSED: 'SESSION_CLOSED',
  NOT_PARTICIPANT: 'NOT_PARTICIPANT',
  TARGET_NOT_FOUND: 'TARGET_NOT_FOUND',
  PTY_ERROR: 'PTY_ERROR',
  /**
   * Transient storage failure during the WS handshake (e.g. the
   * participants lookup errored out). Clients should treat it as
   * "retry in a moment" — it is NOT a permission or invite problem.
   * Paired with `CloseCode.TRANSIENT` on the close frame.
   */
  STORAGE_ERROR: 'STORAGE_ERROR',
  SESSION_DISABLED: 'SESSION_DISABLED',
} as const;
export type ErrorCode = (typeof ErrorCode)[keyof typeof ErrorCode];

/**
 * WebSocket close codes the gateway uses to signal "retry vs give up"
 * to the client. MUST stay in sync with `CLOSE_CODE_TERMINAL` /
 * `CLOSE_CODE_TRANSIENT` in `crates/telepair-core/src/protocol.rs` —
 * the close code is the single signal the client has to decide whether
 * to reconnect, since the preceding JSON `Error` frame may be dropped
 * if the socket tears down mid-write.
 */
export const CloseCode = {
  /**
   * Permanent refusal — auth / permission / not-found / target-missing.
   * `TelepairSocket.onclose` MUST surface an error state and NOT
   * schedule a reconnect, otherwise a revoked token would DoS the
   * gateway with a retry storm.
   */
  TERMINAL: 4001,
  /**
   * Transient failure (e.g. a one-off SQLite hiccup during the
   * handshake). The client is expected to reconnect on its own. Sits
   * in the private-use range (4000-4999) and is chosen to be visually
   * distinct from TERMINAL rather than for any IANA meaning.
   */
  TRANSIENT: 4503,
} as const;
export type CloseCode = (typeof CloseCode)[keyof typeof CloseCode];

export type Role = 'owner' | 'operator' | 'viewer';

export type InputMode = 'serialized' | 'multiplexed';

export type SessionStatus = 'active' | 'closed';

/**
 * Reason a session moved from `active` to `closed`. Mirrors the
 * `CloseReason` enum in `crates/telepair-core/src/session.rs` — the
 * server serializes it as a lowercase string and the history view
 * renders each variant as a distinct chip, so a new variant on the
 * backend MUST be added here at the same time or the UI will show
 * it as `Unknown`.
 *
 * - `owner`   – the owner clicked Close in the session page.
 * - `reaper`  – the idle reaper closed it after no participants were left.
 * - `startup` – server restart cleaned up an orphaned active row.
 * - `error`   – WS-phase launch failure (target vanished, PTY spawn failed, etc.).
 */
export type CloseReason = 'owner' | 'reaper' | 'startup' | 'error';

export interface Session {
  id: string;
  owner_id: string;
  target_name: string;
  input_mode: InputMode;
  status: SessionStatus;
  created_at: string;
  closed_at: string | null;
  /**
   * Populated only for closed rows. `null` on a session closed before
   * the v0.1.1 upgrade added the column, so the UI has to tolerate
   * `null` even on `status === 'closed'` entries.
   */
  closed_reason?: CloseReason | null;
}

export interface ParticipantInfo {
  user_id: string;
  name: string;
  role: Role;
  color: string;
}

export interface TargetInfo {
  name: string;
  display: string;
  tags: string[];
  /** "global" for targets.yaml entries; "user" for user-owned targets. */
  source: 'global' | 'user';
  /** Present only for user-owned targets. */
  id?: string;
  admin_only: boolean;
}

export interface UserTargetInfo {
  id: string;
  user_id: string;
  name: string;
  display: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  tags: string[];
  created_at: string;
  updated_at: string;
}

// --- Client → Server ---
//
// Terminal input is NOT a JSON message — it is sent as a raw binary WebSocket
// frame (see TelepairSocket.sendInput). All other messages below are JSON.

export type ClientMessage =
  | { type: 'SessionJoin'; session_id: string; token: string; cols: number; rows: number }
  | { type: 'TermResize'; cols: number; rows: number }
  | { type: 'CursorMove'; x: number; y: number }
  | { type: 'ChatMessage'; text: string };

// --- Server → Client ---

export type ServerMessage =
  | { type: 'SessionState'; session: Session; participants: ParticipantInfo[]; your_role: Role; your_user_id: string }
  | { type: 'PeerJoined'; user_id: string; name: string; role: Role; color: string }
  | { type: 'PeerLeft'; user_id: string }
  | { type: 'PeerCursor'; user_id: string; x: number; y: number }
  | { type: 'PeerChat'; user_id: string; name: string; text: string; ts: string }
  | { type: 'InputDenied'; reason: InputDeniedReason }
  | { type: 'Error'; code: string; message: string };

// Reason codes mirroring `crates/telepair-core/src/protocol.rs::input_denied`.
// The server only sends these strings; any unknown value should be rendered
// as a generic "input not allowed" so a protocol upgrade doesn't silently
// swallow the notice.
export const InputDeniedReason = {
  VIEWER: 'VIEWER',
  SERIALIZED_NOT_OWNER: 'SERIALIZED_NOT_OWNER',
} as const;
export type InputDeniedReason = (typeof InputDeniedReason)[keyof typeof InputDeniedReason];

/**
 * Audit event type discriminant, mirroring `AuditEventType::as_str()`
 * in `crates/telepair-core/src/audit.rs`. The on-disk / on-wire form is
 * the dotted-lowercase string — this constant pins the exact set so the
 * frontend's `eventLabel` switch stays exhaustive under TypeScript and
 * a backend rename surfaces as a compile error instead of a rendered
 * raw string.
 */
export const AuditEventType = {
  SESSION_CREATED: 'session.created',
  SESSION_CLOSED: 'session.closed',
  PARTICIPANT_JOINED: 'participant.joined',
  INVITE_MINTED: 'invite.minted',
  INVITE_REDEEMED: 'invite.redeemed',
  INVITE_REVOKED: 'invite.revoked',
  TARGET_ACCESS_DENIED: 'target.access_denied',
  TARGET_RELOADED: 'target.reloaded',
  AUTH_LOGIN_FAILED: 'auth.login_failed',
  AUTH_REGISTER_REJECTED: 'auth.register_rejected',
  AUTH_REGISTER_COMPLETED: 'auth.register_completed',
  AUTH_VERIFY_FAILED: 'auth.verify_failed',
  AUTH_USER_ENABLED: 'auth.user_enabled',
  AUTH_USER_DISABLED: 'auth.user_disabled',
  AUTH_SESSION_ACCESS_DENIED: 'auth.session_access_denied',
} as const;
export type AuditEventType = (typeof AuditEventType)[keyof typeof AuditEventType];

// --- Helpers ---

const textEncoder = new TextEncoder();

export function encodeInput(text: string): Uint8Array {
  return textEncoder.encode(text);
}

/**
 * Returns whether the given (role, inputMode) combo may forward
 * keystrokes to the PTY. The old 1-arg signature silently let operators
 * type in serialized sessions — the server then dropped those bytes,
 * producing a dead-keyboard UX. Callers MUST pass `inputMode` so the
 * client and server enforcement agree.
 */
export function canInput(role: Role, inputMode: InputMode): boolean {
  if (role === 'owner') return true;
  if (role === 'operator') return inputMode === 'multiplexed';
  return false;
}

export interface InviteInfo {
  token: string;
  role: Role;
  max_uses: number;
  /**
   * Absolute expiry resolved server-side. `null` means the invite
   * never expires on its own (it will still die when `max_uses` runs
   * out or the session closes). The frontend should render a humanised
   * countdown when this is set so the owner can see at a glance how
   * long the link will stay live.
   */
  expires_at: string | null;
  session_id: string;
}

export interface RedeemResult {
  session_id: string;
  role: Role;
  /**
   * Freshly issued guest token when the redeem call was made without
   * (or with an invalid) bearer token. The frontend should store this
   * into auth state before navigating to the session. `null` when the
   * caller was already authenticated — they keep using their own
   * token in that case.
   */
  token: string | null;
}

/**
 * One row of the per-session audit timeline. Mirrors `AuditEvent` in
 * `crates/telepair-core/src/audit.rs`. The server returns rows newest
 * first (`ts DESC`), capped at 500.
 *
 * `event_type` is the dotted-lowercase form (`session.created`,
 * `participant.joined`, …) — the same string the CLI `--type` flag
 * accepts and the same column value stored in `audit_events.event_type`.
 * The list of variants is intentionally finite; an unknown value most
 * likely means the frontend is older than the backend it's talking to,
 * so the UI renders unknown types verbatim instead of throwing.
 *
 * `actor_id` and `actor_name` are nullable independently:
 *   - `actor_id` is null for events that happened without an
 *     authenticated identity (e.g. invite redemption by an anonymous
 *     visitor — the row is stamped with `actor_name='guest'` and a
 *     freshly minted user id, but pre-redeem rows have neither).
 *   - `actor_name` is a denormalized snapshot taken at insertion time;
 *     renaming a user later does NOT rewrite history.
 *
 * `detail` is an opaque JSON value (object, string, number, or null).
 * The shape varies per event type — see the matching variant comments
 * in `crates/telepair-core/src/audit.rs::AuditEventType`. The UI does
 * not assume a specific shape; it pretty-prints the JSON on demand and
 * extracts the few well-known keys (`role`, `reason`) lazily.
 */
export interface AuditEvent {
  id: number | null;
  ts: string;
  actor_id: string | null;
  actor_name: string | null;
  event_type: string;
  session_id: string | null;
  detail: unknown;
}

/**
 * Sanitized view of a single invite row for the owner-facing
 * management dialog. Mirrors `InviteSummary` in
 * `crates/telepair-control/src/invite_service.rs` — the backend
 * deliberately does NOT leak the raw bearer token here. The UI uses
 * `token_prefix` (first 8 chars of the sha) as a stable per-row label
 * and `token_sha256` as the DELETE path parameter when revoking.
 */
export interface InviteSummary {
  token_sha256: string;
  token_prefix: string;
  session_id: string;
  role: Role;
  max_uses: number;
  used_count: number;
  /** `max_uses - used_count`, clamped to zero. Precomputed server-side so every client renders the same number. */
  remaining_uses: number;
  expires_at: string | null;
  created_at: string | null;
}

/**
 * Env var presence marker for the admin-targets detail view. The
 * server deliberately NEVER returns the resolved value — leaking
 * `PGPASSWORD=...` through an HTTP API would widen the blast radius
 * beyond the "anyone who can write targets.yaml can exfiltrate env"
 * trust boundary we already accept. The UI renders `set: true` as a
 * filled chip and `set: false` as a hollow chip so an admin can spot
 * a missing variable at a glance.
 */
export interface AdminTargetEnvKey {
  key: string;
  set: boolean;
}

/**
 * One row returned by `GET /api/admin/targets`. Mirrors
 * `AdminTargetInfo` in `crates/telepair-gateway/src/http.rs`. Unlike
 * the public `TargetInfo`, this carries the full config (command,
 * args, shell) plus the runtime `active_sessions` count — everything
 * the admin page needs to render a complete detail card and a
 * per-target deep link into the session history view.
 */
export interface AdminTargetInfo {
  name: string;
  display: string;
  /** `virtual` (from targets.yaml) or `local` (the built-in local-shell target). */
  type: 'virtual' | 'local' | string;
  command: string | null;
  args: string[];
  shell: string | null;
  tags: string[];
  admin_only: boolean;
  env: AdminTargetEnvKey[];
  active_sessions: number;
}

/**
 * Success body returned by `POST /api/admin/targets/reload`. `path`
 * is the absolute path re-read from disk; `targets` is the number of
 * targets in the new engine. The UI surfaces both in the success
 * toast so the admin can confirm they reloaded the file they meant to.
 */
export interface ReloadTargetsResult {
  path: string;
  targets: number;
}

/**
 * Mirrors `AdminUserInfo` in `crates/telepair-gateway/src/http.rs`.
 * Returned by `GET /api/admin/users` and
 * `POST /api/admin/users/{id}/{enable,disable}`.
 */
export interface AdminUserInfo {
  id: string;
  name: string;
  email: string | null;
  is_admin: boolean;
  session_enabled: boolean;
  created_at: string;
  updated_at: string;
}
