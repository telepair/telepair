// web/src/pages/Session.tsx
import { createSignal, onCleanup, Show } from 'solid-js';
import { useParams, useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { TelepairSocket } from '../lib/ws';
import { encodeInput, decodeOutput } from '../lib/protocol';
import type { ServerMessage, Role } from '../lib/protocol';
import type { TerminalHandle } from '../components/Terminal';
import Terminal from '../components/Terminal';

type ConnectionStatus = 'connecting' | 'connected' | 'disconnected' | 'error';

export default function SessionPage() {
  const params = useParams<{ id: string }>();
  const navigate = useNavigate();

  const [status, setStatus] = createSignal<ConnectionStatus>('connecting');
  const [role, setRole] = createSignal<Role>('viewer');
  const [errorMsg, setErrorMsg] = createSignal('');

  let termHandle: TerminalHandle | undefined;
  let socket: TelepairSocket | undefined;

  const handleMessage = (msg: ServerMessage) => {
    switch (msg.type) {
      case 'SessionState':
        setRole(msg.your_role);
        break;
      case 'TermOutput':
        termHandle?.write(decodeOutput(msg.data));
        break;
      case 'Error':
        setErrorMsg(`${msg.code}: ${msg.message}`);
        break;
    }
  };

  const handleStatus = (s: ConnectionStatus) => {
    setStatus(s);
  };

  const handleData = (data: string) => {
    if (role() === 'viewer') return;
    socket?.sendInput(encodeInput(data));
  };

  const handleResize = (cols: number, rows: number) => {
    if (role() === 'viewer') return;
    socket?.sendResize(cols, rows);
  };

  // Connect WebSocket
  socket = new TelepairSocket(handleMessage, handleStatus);
  socket.connect(params.id, auth.token());

  onCleanup(() => {
    socket?.disconnect();
  });

  return (
    <div class="session-page">
      <header class="session-topbar">
        <button class="back-btn" onClick={() => navigate('/')}>← Back</button>
        <span class="session-label">Session: <code>{params.id}</code></span>
        <span class="role-badge" data-role={role()}>{role()}</span>
        <span class="status-dot" data-status={status()} />
      </header>

      <Show when={errorMsg()}>
        <div class="error-banner">{errorMsg()}</div>
      </Show>

      <div class="terminal-container">
        <Terminal
          onData={handleData}
          onResize={handleResize}
          ref={(h) => { termHandle = h; }}
        />
      </div>

      <style>{`
        .session-page {
          display: flex;
          flex-direction: column;
          height: 100vh;
        }
        .session-topbar {
          display: flex;
          align-items: center;
          gap: 12px;
          padding: 8px 16px;
          border-bottom: 1px solid var(--border);
          background: var(--bg-secondary);
          font-size: 13px;
        }
        .back-btn {
          font-size: 13px;
          padding: 4px 10px;
        }
        .session-label code {
          font-family: var(--font-mono);
          color: var(--accent);
        }
        .role-badge {
          padding: 2px 8px;
          border-radius: 12px;
          font-size: 11px;
          font-weight: 600;
          text-transform: uppercase;
        }
        .role-badge[data-role="owner"] { background: rgba(63, 185, 80, 0.2); color: var(--success); }
        .role-badge[data-role="operator"] { background: rgba(88, 166, 255, 0.2); color: var(--accent); }
        .role-badge[data-role="viewer"] { background: rgba(139, 148, 158, 0.2); color: var(--text-secondary); }

        .status-dot {
          width: 8px;
          height: 8px;
          border-radius: 50%;
          margin-left: auto;
        }
        .status-dot[data-status="connecting"] { background: var(--warning); }
        .status-dot[data-status="connected"] { background: var(--success); }
        .status-dot[data-status="disconnected"] { background: var(--text-secondary); }
        .status-dot[data-status="error"] { background: var(--error); }

        .error-banner {
          padding: 8px 16px;
          background: rgba(248, 81, 73, 0.15);
          color: var(--error);
          font-size: 13px;
          border-bottom: 1px solid rgba(248, 81, 73, 0.3);
        }

        .terminal-container {
          flex: 1;
          padding: 4px;
          overflow: hidden;
        }
      `}</style>
    </div>
  );
}
