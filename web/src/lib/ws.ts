// web/src/lib/ws.ts
import type { ClientMessage, ServerMessage } from './protocol';

export type ConnectionStatus =
  | 'connecting'
  | 'connected'
  | 'disconnected'
  | 'error'
  | 'giveup';

export type ReconnectInfo = {
  /** 1-based retry attempt number currently in flight. */
  attempt: number;
  /** Maximum number of automatic retries before falling back to manual. */
  maxAttempts: number;
  /** Milliseconds until the scheduled retry fires. */
  nextDelayMs: number;
};

export type MessageHandler = (msg: ServerMessage) => void;
export type BinaryHandler = (data: Uint8Array) => void;
export type StatusHandler = (status: ConnectionStatus) => void;
export type ReconnectInfoHandler = (info: ReconnectInfo | null) => void;

export class TelepairSocket {
  private ws: WebSocket | null = null;
  private onMessage: MessageHandler;
  private onBinary: BinaryHandler;
  private onStatus: StatusHandler;
  /**
   * Optional listener for reconnect progress — fires with a fresh
   * {@link ReconnectInfo} on each scheduled retry and with `null` when
   * reconnection is no longer in progress (success, giveup, or manual reset).
   * Assign after construction: `sock.onReconnectInfo = (info) => ...`.
   */
  onReconnectInfo: ReconnectInfoHandler | null = null;

  private sessionId = '';
  private token = '';
  private cols = 80;
  private rows = 24;
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 5;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private intentionalClose = false;

  constructor(onMessage: MessageHandler, onBinary: BinaryHandler, onStatus: StatusHandler) {
    this.onMessage = onMessage;
    this.onBinary = onBinary;
    this.onStatus = onStatus;
  }

  connect(sessionId: string, token: string, cols = 80, rows = 24) {
    this.sessionId = sessionId;
    this.token = token;
    this.cols = cols;
    this.rows = rows;
    this.intentionalClose = false;
    this.reconnectAttempts = 0;
    this.onReconnectInfo?.(null);
    this.doConnect();
  }

  /**
   * Abort any pending auto-retry and immediately attempt a fresh connect.
   * Intended for the user-facing "Reconnect" button shown after the auto-retry
   * budget is exhausted.
   */
  reconnectNow() {
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.reconnectAttempts = 0;
    this.intentionalClose = false;
    this.onReconnectInfo?.(null);
    this.doConnect();
  }

  private doConnect() {
    this.onStatus('connecting');
    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${protocol}//${location.host}/ws/session/${this.sessionId}`;
    this.ws = new WebSocket(url);
    this.ws.binaryType = 'arraybuffer';

    this.ws.onopen = () => {
      this.reconnectAttempts = 0;
      this.send({
        type: 'SessionJoin',
        session_id: this.sessionId,
        token: this.token,
        cols: this.cols,
        rows: this.rows,
      });
    };

    this.ws.onmessage = (event) => {
      if (event.data instanceof ArrayBuffer) {
        this.onBinary(new Uint8Array(event.data));
        return;
      }
      try {
        const msg: ServerMessage = JSON.parse(event.data);
        if (msg.type === 'SessionState') {
          this.onStatus('connected');
          this.onReconnectInfo?.(null);
        }
        this.onMessage(msg);
      } catch {
        console.error('Failed to parse WS message:', event.data);
      }
    };

    this.ws.onclose = (event) => {
      if (this.intentionalClose) {
        this.onStatus('disconnected');
        return;
      }
      if (event.code === 1008 || event.code === 4001) {
        this.onStatus('error');
        this.onReconnectInfo?.(null);
        return;
      }
      this.scheduleReconnect();
    };

    this.ws.onerror = () => {}; // onclose fires after onerror
  }

  private scheduleReconnect() {
    if (this.reconnectAttempts >= this.maxReconnectAttempts) {
      this.onStatus('giveup');
      this.onReconnectInfo?.(null);
      return;
    }
    this.reconnectAttempts++;
    const delay = Math.min(1000 * 2 ** (this.reconnectAttempts - 1), 30_000);
    this.onStatus('connecting');
    this.onReconnectInfo?.({
      attempt: this.reconnectAttempts,
      maxAttempts: this.maxReconnectAttempts,
      nextDelayMs: delay,
    });
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.doConnect();
    }, delay);
  }

  send(msg: ClientMessage) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  sendInput(data: Uint8Array) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(data);
    }
  }

  sendResize(cols: number, rows: number) {
    this.send({ type: 'TermResize', cols, rows });
  }

  disconnect() {
    this.intentionalClose = true;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.ws?.close();
    this.ws = null;
    this.onReconnectInfo?.(null);
  }
}
