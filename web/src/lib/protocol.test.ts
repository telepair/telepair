import { describe, it, expect } from 'vitest';
import { encodeInput } from './protocol';
import type { ClientMessage, ServerMessage } from './protocol';

describe('encodeInput', () => {
  it('encodes ASCII string to number array', () => {
    const result = encodeInput('hello');
    expect(result).toEqual([104, 101, 108, 108, 111]);
  });

  it('encodes empty string to empty array', () => {
    expect(encodeInput('')).toEqual([]);
  });

  it('encodes multi-byte UTF-8 characters', () => {
    const result = encodeInput('日');
    // '日' is U+65E5, encoded as 3 bytes in UTF-8: 0xE6, 0x97, 0xA5
    expect(result).toEqual([0xe6, 0x97, 0xa5]);
  });
});

describe('ClientMessage type narrowing', () => {
  it('discriminates on type field', () => {
    const msg: ClientMessage = { type: 'TermInput', data: [65] };
    if (msg.type === 'TermInput') {
      expect(msg.data).toEqual([65]);
    }
  });

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
});
