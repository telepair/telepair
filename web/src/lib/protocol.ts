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

export interface Session {
  id: string;
  owner_id: string;
  target_name: string;
  input_mode: InputMode;
  status: SessionStatus;
  created_at: string;
  closed_at: string | null;
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
