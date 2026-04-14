import { describe, it, expect, vi, beforeEach } from 'vitest';
import { CloseCode } from './protocol';
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
  sent: Array<string | Uint8Array> = [];

  constructor(public url: string) {
    // Simulate async open
    setTimeout(() => this.onopen?.(), 0);
  }

  send(data: string | Uint8Array) {
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
    const joinMsg = JSON.parse(ws.sent[0] as string);
    expect(joinMsg.type).toBe('SessionJoin');
    expect(joinMsg.session_id).toBe('sess-1');
    expect(joinMsg.token).toBe('my-token');
    expect(joinMsg.cols).toBe(80);
    expect(joinMsg.rows).toBe(24);
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
      your_user_id: 'u',
      chat_history: [],
    };
    ws.onmessage?.({ data: JSON.stringify(state) });

    expect(onStatus).toHaveBeenCalledWith('connected');
  });

  it('sendInput sends a raw binary frame', async () => {
    const sock = new TelepairSocket(vi.fn(), vi.fn(), vi.fn());
    sock.connect('s', 't');
    await new Promise((r) => setTimeout(r, 10));

    sock.sendInput(new Uint8Array([65, 66]));

    const ws = (sock as any).ws as MockWebSocket;
    // sent[0] is the SessionJoin JSON, sent[1] is the binary keystroke frame
    const payload = ws.sent[1];
    expect(payload).toBeInstanceOf(Uint8Array);
    expect(Array.from(payload as Uint8Array)).toEqual([65, 66]);
  });

  it('sendResize sends TermResize message', async () => {
    const sock = new TelepairSocket(vi.fn(), vi.fn(), vi.fn());
    sock.connect('s', 't');
    await new Promise((r) => setTimeout(r, 10));

    sock.sendResize(120, 40);

    const ws = (sock as any).ws as MockWebSocket;
    const msg = JSON.parse(ws.sent[1] as string);
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
    ws.onclose?.({ code: CloseCode.TERMINAL, reason: 'Unauthorized' });

    const statusCalls = onStatus.mock.calls.map((c: any[]) => c[0]);
    expect(statusCalls).toContain('error');
    expect((sock as any).reconnectTimer).toBeNull();
  });

  // The whole point of the STORAGE_ERROR fix: a transient storage
  // hiccup on the server must NOT strand us with an `error` status.
  // The close code (CloseCode.TRANSIENT) is in the private-use 4xxx
  // range but must fall through to the retry loop exactly like any
  // other unexpected close. A regression here brings back the "SQLite
  // blip => dead page until you hit refresh" UX that Codex flagged.
  it('reconnects on transient storage close (code CloseCode.TRANSIENT)', async () => {
    vi.useFakeTimers();
    const onStatus = vi.fn();
    const sock = new TelepairSocket(vi.fn(), vi.fn(), onStatus);
    sock.connect('s', 't');

    await vi.advanceTimersByTimeAsync(10);

    const ws = (sock as any).ws as MockWebSocket;
    ws.onclose?.({ code: CloseCode.TRANSIENT, reason: 'temporary storage failure' });

    const statusCalls = onStatus.mock.calls.map((c: any[]) => c[0]);
    expect(statusCalls).not.toContain('error');
    expect(statusCalls).toContain('connecting');
    expect((sock as any).reconnectAttempts).toBe(1);
    expect((sock as any).reconnectTimer).not.toBeNull();

    vi.useRealTimers();
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

  it('emits reconnect info on scheduled retry', async () => {
    vi.useFakeTimers();
    const onInfo = vi.fn();
    const sock = new TelepairSocket(vi.fn(), vi.fn(), vi.fn());
    sock.onReconnectInfo = onInfo;
    sock.connect('s', 't');

    await vi.advanceTimersByTimeAsync(10);

    const ws = (sock as any).ws as MockWebSocket;
    ws.onclose?.({ code: 1006, reason: '' });

    expect(onInfo).toHaveBeenCalled();
    const info = onInfo.mock.calls[onInfo.mock.calls.length - 1][0];
    expect(info).toMatchObject({ attempt: 1, maxAttempts: 5, nextDelayMs: 1000 });
    vi.useRealTimers();
  });

  it('clears reconnect info after a successful reconnect', async () => {
    vi.useFakeTimers();
    const onStatus = vi.fn();
    const onInfo = vi.fn();
    const sock = new TelepairSocket(vi.fn(), vi.fn(), onStatus);
    sock.onReconnectInfo = onInfo;
    sock.connect('s', 't');

    await vi.advanceTimersByTimeAsync(10);
    const ws = (sock as any).ws as MockWebSocket;
    ws.onclose?.({ code: 1006, reason: '' });
    expect(onInfo).toHaveBeenLastCalledWith(
      expect.objectContaining({ attempt: 1 }),
    );

    // Advance past the scheduled retry so doConnect runs again.
    await vi.advanceTimersByTimeAsync(1100);
    const ws2 = (sock as any).ws as MockWebSocket;
    // Simulate SessionState arrival which marks connected and clears info.
    ws2.onmessage?.({
      data: JSON.stringify({
        type: 'SessionState',
        session: { id: 's', owner_id: 'u', target_name: 't', input_mode: 'serialized', status: 'active', created_at: '', closed_at: null },
        participants: [],
        your_role: 'owner',
        your_user_id: 'u',
      }),
    });
    expect(onInfo).toHaveBeenLastCalledWith(null);
    expect(onStatus).toHaveBeenCalledWith('connected');
    vi.useRealTimers();
  });

  it('transitions to giveup after exhausting auto-retries', async () => {
    vi.useFakeTimers();
    const onStatus = vi.fn();
    const onInfo = vi.fn();
    const sock = new TelepairSocket(vi.fn(), vi.fn(), onStatus);
    sock.onReconnectInfo = onInfo;
    sock.connect('s', 't');

    await vi.advanceTimersByTimeAsync(10);

    // Pin reconnectAttempts at the max so the next scheduleReconnect call
    // falls into the giveup branch. Walking the full loop via mock close is
    // fragile because MockWebSocket auto-fires onopen, which resets attempts.
    (sock as any).reconnectAttempts = 5;

    const ws = (sock as any).ws as MockWebSocket;
    ws.onclose?.({ code: 1006, reason: '' });

    const statusCalls = onStatus.mock.calls.map((c: any[]) => c[0]);
    expect(statusCalls).toContain('giveup');
    expect(onInfo).toHaveBeenLastCalledWith(null);
    expect((sock as any).reconnectTimer).toBeNull();
    vi.useRealTimers();
  });

  it('reconnectNow cancels pending retry and immediately reconnects', async () => {
    vi.useFakeTimers();
    const onStatus = vi.fn();
    const onInfo = vi.fn();
    const sock = new TelepairSocket(vi.fn(), vi.fn(), onStatus);
    sock.onReconnectInfo = onInfo;
    sock.connect('s', 't');

    await vi.advanceTimersByTimeAsync(10);

    const ws = (sock as any).ws as MockWebSocket;
    ws.onclose?.({ code: 1006, reason: '' });
    expect((sock as any).reconnectTimer).not.toBeNull();

    sock.reconnectNow();

    // Pending timer should be cleared and a fresh ws created.
    expect((sock as any).reconnectTimer).toBeNull();
    expect((sock as any).reconnectAttempts).toBe(0);
    // onReconnectInfo should have been told to clear (null) on manual reset.
    expect(onInfo).toHaveBeenLastCalledWith(null);
    vi.useRealTimers();
  });
});
