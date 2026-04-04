// web/src/lib/ws.ts
import type { ClientMessage, ServerMessage } from './protocol';

export type MessageHandler = (msg: ServerMessage) => void;
export type StatusHandler = (status: 'connecting' | 'connected' | 'disconnected' | 'error') => void;

export class TelepairSocket {
  private ws: WebSocket | null = null;
  private onMessage: MessageHandler;
  private onStatus: StatusHandler;

  constructor(onMessage: MessageHandler, onStatus: StatusHandler) {
    this.onMessage = onMessage;
    this.onStatus = onStatus;
  }

  connect(sessionId: string, token: string) {
    this.onStatus('connecting');

    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${protocol}//${location.host}/ws/session/${sessionId}`;
    this.ws = new WebSocket(url);

    this.ws.onopen = () => {
      this.send({
        type: 'SessionJoin',
        session_id: sessionId,
        token,
      });
    };

    this.ws.onmessage = (event) => {
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

    this.ws.onclose = () => {
      this.onStatus('disconnected');
    };

    this.ws.onerror = () => {
      this.onStatus('error');
    };
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
    this.ws?.close();
    this.ws = null;
  }
}
