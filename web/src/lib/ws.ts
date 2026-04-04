// web/src/lib/ws.ts
import type { ClientMessage, ServerMessage } from './protocol';

export type MessageHandler = (msg: ServerMessage) => void;
export type BinaryHandler = (data: Uint8Array) => void;
export type StatusHandler = (status: 'connecting' | 'connected' | 'disconnected' | 'error') => void;

export class TelepairSocket {
  private ws: WebSocket | null = null;
  private onMessage: MessageHandler;
  private onBinary: BinaryHandler;
  private onStatus: StatusHandler;
  private sessionId = '';
  private token = '';
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 10;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private intentionalClose = false;

  constructor(onMessage: MessageHandler, onBinary: BinaryHandler, onStatus: StatusHandler) {
    this.onMessage = onMessage;
    this.onBinary = onBinary;
    this.onStatus = onStatus;
  }

  connect(sessionId: string, token: string) {
    this.sessionId = sessionId;
    this.token = token;
    this.intentionalClose = false;
    this.reconnectAttempts = 0;
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
        return;
      }
      this.scheduleReconnect();
    };

    this.ws.onerror = () => {}; // onclose fires after onerror
  }

  private scheduleReconnect() {
    if (this.reconnectAttempts >= this.maxReconnectAttempts) {
      this.onStatus('disconnected');
      return;
    }
    this.onStatus('connecting');
    const delay = Math.min(1000 * 2 ** this.reconnectAttempts, 30000);
    this.reconnectAttempts++;
    this.reconnectTimer = setTimeout(() => this.doConnect(), delay);
  }

  send(msg: ClientMessage) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  sendInput(data: number[]) {
    this.send({ type: 'TermInput', data });
  }

  sendResize(cols: number, rows: number) {
    this.send({ type: 'TermResize', cols, rows });
  }

  disconnect() {
    this.intentionalClose = true;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.ws?.close();
    this.ws = null;
  }
}
