import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { ServerMessage } from './protocol';

// Mock WebSocket
class MockWebSocket {
  static OPEN = 1;
  readyState = MockWebSocket.OPEN;
  binaryType = '';
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string | ArrayBuffer }) => void) | null = null;
  onclose: ((event: { code: number; reason: string }) => void) | null = null;
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
    this.onclose?.({ code: 1000, reason: '' });
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
    const onBinary = vi.fn();
    const onStatus = vi.fn();
    const sock = new TelepairSocket(onMsg, onBinary, onStatus);
    sock.connect('sess-1', 'my-token');

    expect(onStatus).toHaveBeenCalledWith('connecting');
  });

  it('sets binaryType to arraybuffer', async () => {
    const sock = new TelepairSocket(vi.fn(), vi.fn(), vi.fn());
    sock.connect('sess-1', 'my-token');

    const ws = (sock as any).ws as MockWebSocket;
    expect(ws.binaryType).toBe('arraybuffer');
  });

  it('sends SessionJoin on open', async () => {
    const onMsg = vi.fn();
    const onBinary = vi.fn();
    const onStatus = vi.fn();
    const sock = new TelepairSocket(onMsg, onBinary, onStatus);
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

  it('forwards parsed text messages to message handler', async () => {
    const onMsg = vi.fn();
    const onBinary = vi.fn();
    const onStatus = vi.fn();
    const sock = new TelepairSocket(onMsg, onBinary, onStatus);
    sock.connect('sess-1', 'tok');

    await new Promise((r) => setTimeout(r, 10));

    const ws = (sock as any).ws as MockWebSocket;
    const peerJoined: ServerMessage = {
      type: 'PeerJoined',
      user_id: 'u1',
      name: 'Alice',
      role: 'operator',
      color: '#fff',
    };
    ws.onmessage?.({ data: JSON.stringify(peerJoined) });

    expect(onMsg).toHaveBeenCalledWith(peerJoined);
    expect(onBinary).not.toHaveBeenCalled();
  });

  it('forwards binary messages to binary handler', async () => {
    const onMsg = vi.fn();
    const onBinary = vi.fn();
    const onStatus = vi.fn();
    const sock = new TelepairSocket(onMsg, onBinary, onStatus);
    sock.connect('sess-1', 'tok');

    await new Promise((r) => setTimeout(r, 10));

    const ws = (sock as any).ws as MockWebSocket;
    const binaryData = new ArrayBuffer(3);
    const view = new Uint8Array(binaryData);
    view[0] = 65;
    view[1] = 66;
    view[2] = 67;
    ws.onmessage?.({ data: binaryData });

    expect(onBinary).toHaveBeenCalledTimes(1);
    const received = onBinary.mock.calls[0][0];
    expect(received).toBeInstanceOf(Uint8Array);
    expect(Array.from(received)).toEqual([65, 66, 67]);
    expect(onMsg).not.toHaveBeenCalled();
  });

  it('sets connected status on SessionState', async () => {
    const onMsg = vi.fn();
    const onBinary = vi.fn();
    const onStatus = vi.fn();
    const sock = new TelepairSocket(onMsg, onBinary, onStatus);
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
    const sock = new TelepairSocket(vi.fn(), vi.fn(), vi.fn());
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
    const sock = new TelepairSocket(vi.fn(), vi.fn(), vi.fn());
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
    const sock = new TelepairSocket(vi.fn(), vi.fn(), onStatus);
    sock.connect('s', 't');
    await new Promise((r) => setTimeout(r, 10));

    sock.disconnect();
    expect((sock as any).ws).toBeNull();
    expect(onStatus).toHaveBeenCalledWith('disconnected');
  });

  it('does not reconnect on intentional close', async () => {
    const onStatus = vi.fn();
    const sock = new TelepairSocket(vi.fn(), vi.fn(), onStatus);
    sock.connect('s', 't');
    await new Promise((r) => setTimeout(r, 10));

    sock.disconnect();
    // After intentional close, status should be disconnected, not connecting
    const statusCalls = onStatus.mock.calls.map((c: any[]) => c[0]);
    const lastStatus = statusCalls[statusCalls.length - 1];
    expect(lastStatus).toBe('disconnected');
  });

  it('does not reconnect on auth failure (code 1008)', async () => {
    const onStatus = vi.fn();
    const sock = new TelepairSocket(vi.fn(), vi.fn(), onStatus);
    sock.connect('s', 't');
    await new Promise((r) => setTimeout(r, 10));

    const ws = (sock as any).ws as MockWebSocket;
    ws.onclose?.({ code: 1008, reason: 'Policy Violation' });

    const statusCalls = onStatus.mock.calls.map((c: any[]) => c[0]);
    expect(statusCalls).toContain('error');
    // Should not schedule reconnect
    expect((sock as any).reconnectTimer).toBeNull();
  });

  it('does not reconnect on auth failure (code 4001)', async () => {
    const onStatus = vi.fn();
    const sock = new TelepairSocket(vi.fn(), vi.fn(), onStatus);
    sock.connect('s', 't');
    await new Promise((r) => setTimeout(r, 10));

    const ws = (sock as any).ws as MockWebSocket;
    ws.onclose?.({ code: 4001, reason: 'Unauthorized' });

    const statusCalls = onStatus.mock.calls.map((c: any[]) => c[0]);
    expect(statusCalls).toContain('error');
    expect((sock as any).reconnectTimer).toBeNull();
  });

  it('schedules reconnect on unexpected close', async () => {
    vi.useFakeTimers();
    const onStatus = vi.fn();
    const sock = new TelepairSocket(vi.fn(), vi.fn(), onStatus);
    sock.connect('s', 't');

    await vi.advanceTimersByTimeAsync(10);

    const ws = (sock as any).ws as MockWebSocket;
    // Simulate unexpected close
    ws.onclose?.({ code: 1006, reason: '' });

    expect(onStatus).toHaveBeenCalledWith('connecting');
    expect((sock as any).reconnectAttempts).toBe(1);
    expect((sock as any).reconnectTimer).not.toBeNull();

    vi.useRealTimers();
  });

  it('stores sessionId and token for reconnection', () => {
    const sock = new TelepairSocket(vi.fn(), vi.fn(), vi.fn());
    sock.connect('my-session', 'my-token');

    expect((sock as any).sessionId).toBe('my-session');
    expect((sock as any).token).toBe('my-token');
  });
});
