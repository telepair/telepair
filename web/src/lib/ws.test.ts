import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { ServerMessage } from './protocol';

// Mock WebSocket
class MockWebSocket {
  static OPEN = 1;
  readyState = MockWebSocket.OPEN;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  sent: string[] = [];

  constructor(public url: string) {
    // Simulate async open
    setTimeout(() => this.onopen?.(), 0);
  }

  send(data: string) {
    this.sent.push(data);
  }

  close() {
    this.onclose?.();
  }
}

vi.stubGlobal('WebSocket', MockWebSocket);
vi.stubGlobal('location', { protocol: 'http:', host: 'localhost:5173' });

// Import after mocking globals
const { TelepairSocket } = await import('./ws');

beforeEach(() => {
  vi.clearAllMocks();
});

describe('TelepairSocket', () => {
  it('connects with correct URL', () => {
    const onMsg = vi.fn();
    const onStatus = vi.fn();
    const sock = new TelepairSocket(onMsg, onStatus);
    sock.connect('sess-1', 'my-token');

    expect(onStatus).toHaveBeenCalledWith('connecting');
  });

  it('sends SessionJoin on open', async () => {
    const onMsg = vi.fn();
    const onStatus = vi.fn();
    const sock = new TelepairSocket(onMsg, onStatus);
    sock.connect('sess-1', 'my-token');

    // Wait for async open
    await new Promise((r) => setTimeout(r, 10));

    // Access internal ws to check sent messages
    const ws = (sock as any).ws as MockWebSocket;
    expect(ws.sent.length).toBe(1);
    const joinMsg = JSON.parse(ws.sent[0]);
    expect(joinMsg.type).toBe('SessionJoin');
    expect(joinMsg.session_id).toBe('sess-1');
    expect(joinMsg.token).toBe('my-token');
  });

  it('forwards parsed messages to handler', async () => {
    const onMsg = vi.fn();
    const onStatus = vi.fn();
    const sock = new TelepairSocket(onMsg, onStatus);
    sock.connect('sess-1', 'tok');

    await new Promise((r) => setTimeout(r, 10));

    const ws = (sock as any).ws as MockWebSocket;
    const termOutput: ServerMessage = { type: 'TermOutput', data: [65, 66] };
    ws.onmessage?.({ data: JSON.stringify(termOutput) });

    expect(onMsg).toHaveBeenCalledWith(termOutput);
  });

  it('sets connected status on SessionState', async () => {
    const onMsg = vi.fn();
    const onStatus = vi.fn();
    const sock = new TelepairSocket(onMsg, onStatus);
    sock.connect('sess-1', 'tok');

    await new Promise((r) => setTimeout(r, 10));

    const ws = (sock as any).ws as MockWebSocket;
    const state: ServerMessage = {
      type: 'SessionState',
      session: { id: 's', owner_id: 'u', target_name: 't', input_mode: 'serialized', status: 'active', created_at: '', closed_at: null },
      participants: [],
      your_role: 'owner',
    };
    ws.onmessage?.({ data: JSON.stringify(state) });

    expect(onStatus).toHaveBeenCalledWith('connected');
  });

  it('sendInput sends TermInput message', async () => {
    const sock = new TelepairSocket(vi.fn(), vi.fn());
    sock.connect('s', 't');
    await new Promise((r) => setTimeout(r, 10));

    sock.sendInput([65, 66]);

    const ws = (sock as any).ws as MockWebSocket;
    // sent[0] is SessionJoin, sent[1] is TermInput
    const msg = JSON.parse(ws.sent[1]);
    expect(msg.type).toBe('TermInput');
    expect(msg.data).toEqual([65, 66]);
  });

  it('sendResize sends TermResize message', async () => {
    const sock = new TelepairSocket(vi.fn(), vi.fn());
    sock.connect('s', 't');
    await new Promise((r) => setTimeout(r, 10));

    sock.sendResize(120, 40);

    const ws = (sock as any).ws as MockWebSocket;
    const msg = JSON.parse(ws.sent[1]);
    expect(msg.type).toBe('TermResize');
    expect(msg.cols).toBe(120);
    expect(msg.rows).toBe(40);
  });

  it('disconnect closes and nullifies ws', async () => {
    const onStatus = vi.fn();
    const sock = new TelepairSocket(vi.fn(), onStatus);
    sock.connect('s', 't');
    await new Promise((r) => setTimeout(r, 10));

    sock.disconnect();
    expect((sock as any).ws).toBeNull();
    expect(onStatus).toHaveBeenCalledWith('disconnected');
  });
});
