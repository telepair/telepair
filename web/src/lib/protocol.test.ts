import { describe, it, expect } from 'vitest';
import { encodeInput, canInput, CloseCode, InputDeniedReason } from './protocol';
import type { ClientMessage, ServerMessage } from './protocol';

describe('encodeInput', () => {
  // TextEncoder under jsdom returns a Node-realm Uint8Array, so `instanceof`
  // fails against jsdom's global Uint8Array. We assert shape via byteLength
  // + Array.from, which works across realms.
  it('encodes ASCII string to a typed byte array', () => {
    const result = encodeInput('hello');
    expect(Array.from(result)).toEqual([104, 101, 108, 108, 111]);
  });

  it('encodes empty string to zero bytes', () => {
    const result = encodeInput('');
    expect(result.byteLength).toBe(0);
  });

  it('encodes multi-byte UTF-8 characters', () => {
    const result = encodeInput('日');
    // '日' is U+65E5, encoded as 3 bytes in UTF-8: 0xE6, 0x97, 0xA5
    expect(Array.from(result)).toEqual([0xe6, 0x97, 0xa5]);
  });
});

describe('ClientMessage type narrowing', () => {
  it('TermResize carries cols and rows', () => {
    const msg: ClientMessage = { type: 'TermResize', cols: 80, rows: 24 };
    if (msg.type === 'TermResize') {
      expect(msg.cols).toBe(80);
      expect(msg.rows).toBe(24);
    }
  });
});

describe('ServerMessage type narrowing', () => {
  it('discriminates SessionState', () => {
    const msg: ServerMessage = {
      type: 'SessionState',
      session: {
        id: 'abc',
        owner_id: '550e8400-e29b-41d4-a716-446655440000',
        target_name: 'local-shell',
        input_mode: 'serialized',
        status: 'active',
        created_at: '2026-01-01T00:00:00Z',
        closed_at: null,
      },
      participants: [],
      your_role: 'owner',
      your_user_id: '550e8400-e29b-41d4-a716-446655440000',
      chat_history: [],
      recording: null,
    };
    if (msg.type === 'SessionState') {
      expect(msg.your_role).toBe('owner');
      expect(msg.session.id).toBe('abc');
    }
  });

  it('discriminates Error', () => {
    const msg: ServerMessage = { type: 'Error', code: 'AUTH', message: 'invalid' };
    if (msg.type === 'Error') {
      expect(msg.code).toBe('AUTH');
    }
  });

  it('discriminates InputDenied with a known reason', () => {
    const msg: ServerMessage = {
      type: 'InputDenied',
      reason: InputDeniedReason.SERIALIZED_NOT_OWNER,
    };
    if (msg.type === 'InputDenied') {
      expect(msg.reason).toBe('SERIALIZED_NOT_OWNER');
    }
  });
});

describe('CloseCode', () => {
  // These two values are the protocol's retry-vs-giveup signal — they
  // MUST stay in sync with `CLOSE_CODE_TERMINAL` / `CLOSE_CODE_TRANSIENT`
  // in crates/telepair-core/src/protocol.rs. A desync silently strands
  // users on a dead session page OR turns bad credentials into a
  // reconnect storm, depending on which side drifts.
  it('TERMINAL is 4001 (auth/permission/not-found failures)', () => {
    expect(CloseCode.TERMINAL).toBe(4001);
  });

  it('TRANSIENT is 4503 (storage hiccups, retry on client)', () => {
    expect(CloseCode.TRANSIENT).toBe(4503);
  });

  it('TERMINAL and TRANSIENT are distinct so the client can distinguish them', () => {
    expect(CloseCode.TERMINAL).not.toBe(CloseCode.TRANSIENT);
  });
});

describe('canInput', () => {
  // Matrix: the owner always drives; operators only drive in multiplexed
  // sessions; viewers are always read-only. The 2-arg signature exists
  // because a 1-arg version once silently allowed an operator to type in
  // a serialized session — the bytes were then dropped server-side,
  // producing a dead-keyboard UX. These tests pin the matrix so a
  // regression can't sneak back via a default-argument refactor.
  it('owner can always input', () => {
    expect(canInput('owner', 'multiplexed')).toBe(true);
    expect(canInput('owner', 'serialized')).toBe(true);
  });

  it('operator can input only in multiplexed mode', () => {
    expect(canInput('operator', 'multiplexed')).toBe(true);
    expect(canInput('operator', 'serialized')).toBe(false);
  });

  it('viewer can never input', () => {
    expect(canInput('viewer', 'multiplexed')).toBe(false);
    expect(canInput('viewer', 'serialized')).toBe(false);
  });
});
