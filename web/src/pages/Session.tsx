// web/src/pages/Session.tsx
import { createSignal, onCleanup, Show } from 'solid-js';
import { useParams, useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { TelepairSocket } from '../lib/ws';
import type { ConnectionStatus, ReconnectInfo } from '../lib/ws';
import { encodeInput, canInput } from '../lib/protocol';
import type { ServerMessage, Role, ParticipantInfo } from '../lib/protocol';
import type { TerminalHandle } from '../components/Terminal';
import type { ChatMessage } from '../components/ChatPanel';
import Terminal from '../components/Terminal';
import ParticipantList from '../components/ParticipantList';
import ChatPanel from '../components/ChatPanel';
import InviteDialog from '../components/InviteDialog';
import Banner from '../components/Banner';
import { toast } from '../stores/toast';

export default function SessionPage() {
  const params = useParams<{ id: string }>();
  const navigate = useNavigate();

  const [status, setStatus] = createSignal<ConnectionStatus>('connecting');
  const [reconnectInfo, setReconnectInfo] = createSignal<ReconnectInfo | null>(null);
  const [role, setRole] = createSignal<Role>('viewer');
  const [errorMsg, setErrorMsg] = createSignal('');
  const [endedReason, setEndedReason] = createSignal<string | null>(null);
  const [participants, setParticipants] = createSignal<ParticipantInfo[]>([]);
  const [chatMessages, setChatMessages] = createSignal<ChatMessage[]>([]);
  const [showInvite, setShowInvite] = createSignal(false);
  const [sidebarOpen, setSidebarOpen] = createSignal(true);
  const [currentUserId, setCurrentUserId] = createSignal('');
  let hasConnectedOnce = false;

  let termHandle: TerminalHandle | undefined;
  let pendingOutput: Uint8Array[] = [];
  let socket: TelepairSocket | undefined;

  const handleBinary = (data: Uint8Array) => {
    if (termHandle) {
      termHandle.write(data);
    } else {
      pendingOutput.push(data);
    }
  };

  const handleMessage = (msg: ServerMessage) => {
    switch (msg.type) {
      case 'SessionState':
        setRole(msg.your_role);
        setParticipants(msg.participants);
        setCurrentUserId(msg.your_user_id);
        break;
      case 'PeerJoined':
        setParticipants((prev) => [
          ...prev.filter((p) => p.user_id !== msg.user_id),
          { user_id: msg.user_id, name: msg.name, role: msg.role, color: msg.color },
        ]);
        break;
      case 'PeerLeft':
        setParticipants((prev) => prev.filter((p) => p.user_id !== msg.user_id));
        break;
      case 'PeerChat':
        setChatMessages((prev) => [
          ...prev.slice(-(500 - 1)),
          { user_id: msg.user_id, name: msg.name, text: msg.text, ts: msg.ts },
        ]);
        break;
      case 'PeerCursor':
        break;
      case 'PermUpdate':
        setParticipants((prev) =>
          prev.map((p) =>
            p.user_id === msg.user_id ? { ...p, role: msg.new_role } : p
          )
        );
        if (msg.user_id === currentUserId()) {
          setRole(msg.new_role);
        }
        break;
      case 'Error':
        handleServerError(msg.code, msg.message);
        break;
    }
  };

  const handleServerError = (code: string, message: string) => {
    switch (code) {
      case 'SESSION_CLOSED':
        setEndedReason('This session has ended.');
        toast.info('Session has ended', { duration: 4000 });
        break;
      case 'SESSION_NOT_FOUND':
        setEndedReason('Session not found — it may have been deleted.');
        break;
      case 'NOT_PARTICIPANT':
        setErrorMsg('You are not a participant of this session.');
        break;
      case 'AUTH_FAILED':
      case 'AUTH_TIMEOUT':
        // Token is invalid/expired — clear auth state so AuthGuard redirects
        // to /login. A banner would be invisible since this page is about to
        // unmount, so surface the reason via a global toast instead.
        toast.error('Authentication failed. Please log in again.');
        auth.logout();
        break;
      default:
        setErrorMsg(`${code}: ${message}`);
    }
  };

  const handleStatus = (s: ConnectionStatus) => {
    if (s === 'connected') {
      if (hasConnectedOnce) {
        toast.success('Reconnected', { id: 'reconnect' });
      }
      hasConnectedOnce = true;
    } else if (s === 'giveup') {
      toast.error('Could not reconnect to session', {
        id: 'reconnect',
        action: { label: 'Retry', onClick: () => socket?.reconnectNow() },
      });
    }
    setStatus(s);
  };

  const handleData = (data: string) => {
    if (!canInput(role())) return;
    socket?.sendInput(encodeInput(data));
  };

  const handleResize = (cols: number, rows: number) => {
    if (!canInput(role())) return;
    socket?.sendResize(cols, rows);
  };

  const handleSendChat = (text: string) => {
    socket?.send({ type: 'ChatMessage', text });
  };

  const handleManualReconnect = () => {
    toast.info('Reconnecting…', { id: 'reconnect', duration: 2000 });
    socket?.reconnectNow();
  };

  // Connect WebSocket
  socket = new TelepairSocket(handleMessage, handleBinary, handleStatus);
  socket.onReconnectInfo = setReconnectInfo;
  socket.connect(params.id, auth.token());

  onCleanup(() => {
    // Drop any sticky reconnect toast so its Retry action cannot resurrect
    // this page's socket after the user has navigated away or logged out.
    toast.dismiss('reconnect');
    socket?.disconnect();
  });

  return (
    <div class="session-page">
      <header class="session-topbar">
        <button class="back-btn" onClick={() => navigate('/')}>← Back</button>
        <span class="session-label">Session: <code>{params.id}</code></span>
        <span class="role-badge" data-role={role()}>{role()}</span>
        <span class="status-dot" data-status={status()} />
        <div class="topbar-actions">
          <Show when={role() === 'owner'}>
            <button class="action-btn" onClick={() => setShowInvite(true)}>Invite</button>
          </Show>
          <button class="action-btn" onClick={() => setSidebarOpen(!sidebarOpen())}>
            {sidebarOpen() ? 'Hide' : 'Show'} Sidebar
          </button>
        </div>
      </header>

      <Show when={errorMsg()}>
        <Banner variant="error" onDismiss={() => setErrorMsg('')}>
          {errorMsg()}
        </Banner>
      </Show>

      <Show when={endedReason()}>
        <Banner
          variant="info"
          role="status"
          action={{ label: 'Back to Dashboard', onClick: () => navigate('/') }}
        >
          {endedReason()}
        </Banner>
      </Show>

      <Show when={!endedReason() && status() === 'giveup'}>
        <Banner
          variant="error"
          role="status"
          action={{ label: 'Reconnect', onClick: handleManualReconnect }}
        >
          Connection lost. Automatic retry gave up.
        </Banner>
      </Show>

      <Show when={!endedReason() && status() !== 'giveup' && reconnectInfo()}>
        <Banner variant="warning" role="status">
          <span>
            Connection lost — retrying{' '}
            <strong>
              {reconnectInfo()?.attempt}/{reconnectInfo()?.maxAttempts}
            </strong>{' '}
            (next in {Math.round((reconnectInfo()?.nextDelayMs ?? 0) / 1000)}s)
          </span>
        </Banner>
      </Show>

      <div class="session-body">
        <div class="terminal-container">
          <Terminal
            onData={handleData}
            onResize={handleResize}
            ref={(h) => {
              termHandle = h;
              for (const data of pendingOutput) h.write(data);
              pendingOutput = [];
            }}
          />
        </div>

        <Show when={sidebarOpen()}>
          <aside class="sidebar">
            <div class="sidebar-section">
              <ParticipantList participants={participants()} />
            </div>
            <div class="sidebar-section chat-section">
              <ChatPanel messages={chatMessages()} onSend={handleSendChat} />
            </div>
          </aside>
        </Show>
      </div>

      <InviteDialog
        sessionId={params.id}
        open={showInvite()}
        onClose={() => setShowInvite(false)}
      />

      <style>{`
        .session-page { display: flex; flex-direction: column; height: 100vh; }
        .session-topbar {
          display: flex; align-items: center; gap: 12px;
          padding: 8px 16px; border-bottom: 1px solid var(--border);
          background: var(--bg-secondary); font-size: 13px;
        }
        .back-btn { font-size: 13px; padding: 4px 10px; }
        .session-label code { font-family: var(--font-mono); color: var(--accent); }
        .role-badge { padding: 2px 8px; border-radius: 12px; font-size: 11px; }
        .status-dot { width: 8px; height: 8px; border-radius: 50%; }
        .status-dot[data-status="connecting"] { background: var(--warning); }
        .status-dot[data-status="connected"] { background: var(--success); }
        .status-dot[data-status="disconnected"] { background: var(--text-secondary); }
        .status-dot[data-status="error"] { background: var(--error); }
        .status-dot[data-status="giveup"] { background: var(--error); }
        .topbar-actions { margin-left: auto; display: flex; gap: 8px; }
        .topbar-actions .action-btn { font-size: 12px; padding: 4px 10px; }
        .session-body { flex: 1; display: flex; overflow: hidden; }
        .terminal-container { flex: 1; padding: 4px; overflow: hidden; }
        .sidebar {
          width: 260px; border-left: 1px solid var(--border);
          background: var(--bg-secondary); display: flex;
          flex-direction: column; overflow: hidden;
        }
        .sidebar-section { padding: 12px; }
        .sidebar-section.chat-section {
          flex: 1; border-top: 1px solid var(--border);
          min-height: 0; display: flex; flex-direction: column;
        }
      `}</style>
    </div>
  );
}
