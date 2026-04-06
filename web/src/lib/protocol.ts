// TypeScript types mirroring crates/telepair-core/src/protocol.rs

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
  | { type: 'Error'; code: string; message: string };

// --- Helpers ---

const textEncoder = new TextEncoder();

export function encodeInput(text: string): Uint8Array {
  return textEncoder.encode(text);
}

export function canInput(role: Role): boolean {
  return role === 'owner' || role === 'operator';
}

export interface InviteInfo {
  token: string;
  role: Role;
  max_uses: number;
  session_id: string;
}

export interface RedeemResult {
  session_id: string;
  role: Role;
}
