// web/src/pages/Session.tsx
import { createSignal, onCleanup, Show } from 'solid-js';
import { useParams, useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { TelepairSocket } from '../lib/ws';
import type { ConnectionStatus, ReconnectInfo } from '../lib/ws';
import { encodeInput, canInput, ErrorCode, InputDeniedReason } from '../lib/protocol';
import type { ServerMessage, Role, ParticipantInfo, InputMode } from '../lib/protocol';
import type { TerminalHandle } from '../components/Terminal';
import type { ChatMessage } from '../components/ChatPanel';
import Terminal from '../components/Terminal';
import ParticipantList from '../components/ParticipantList';
import ChatPanel from '../components/ChatPanel';
import InviteDialog from '../components/InviteDialog';
import Banner from '../components/Banner';
import LocaleSwitcher from '../components/LocaleSwitcher';
import { toast } from '../stores/toast';
import {
  renderTemplate,
  roleLabel,
  useI18n,
  type TranslationKey,
} from '../i18n';

const MAX_CHAT_HISTORY = 500;

export default function SessionPage() {
  const { t } = useI18n();
  const params = useParams<{ id: string }>();
  const navigate = useNavigate();

  const [status, setStatus] = createSignal<ConnectionStatus>('connecting');
  const [reconnectInfo, setReconnectInfo] = createSignal<ReconnectInfo | null>(null);
  const [role, setRole] = createSignal<Role>('viewer');
  // Input mode is captured from the first `SessionState` frame and is
  // read by `canInput` on every keystroke. Default `serialized` is the
  // safer preclusion: if SessionState ever fails to arrive, we'd rather
  // block input than silently spam a dead channel.
  const [inputMode, setInputMode] = createSignal<InputMode>('serialized');
  // Banner state is split into two channels so locale switches re-render
  // the text live (mirrors the `auth.errorKey` pattern):
  //   - `*Key` holds an i18n key for messages we ship strings for, so the
  //     translator runs at render time and follows `setLocale`.
  //   - `errorText` holds the raw `${code}: ${message}` fallback for
  //     server errors we don't have copy for; that text is locale-neutral
  //     by design (server returns English) and shown verbatim.
  // Stashing the already-translated string would cache the language at
  // the moment of failure and produce a mixed-language UI after a
  // language toggle.
  const [errorKey, setErrorKey] = createSignal<TranslationKey | null>(null);
  const [errorText, setErrorText] = createSignal('');
  const [endedReasonKey, setEndedReasonKey] = createSignal<TranslationKey | null>(
    null,
  );

  // Reactive view: returns the resolved banner string for the current
  // locale, or empty when no error is set. Reads `errorKey()` /
  // `errorText()` so Solid auto-tracks both inputs and the locale-bound
  // `t()`.
  const errorMessage = (): string => {
    const k = errorKey();
    if (k) return t(k);
    return errorText();
  };
  const dismissError = () => {
    setErrorKey(null);
    setErrorText('');
  };
  const [participants, setParticipants] = createSignal<ParticipantInfo[]>([]);
  const [chatMessages, setChatMessages] = createSignal<ChatMessage[]>([]);
  const [showInvite, setShowInvite] = createSignal(false);
  const [sidebarOpen, setSidebarOpen] = createSignal(true);
  let hasConnectedOnce = false;

  let termHandle: TerminalHandle | undefined;
  let pendingOutput: Uint8Array[] = [];
  let socket: TelepairSocket | undefined;
  // One-shot latch so HMR/StrictMode re-entry into the Terminal ref
  // callback doesn't reopen the WebSocket.
  let socketOpened = false;

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
        setInputMode(msg.session.input_mode);
        setParticipants(msg.participants);
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
          ...prev.slice(-(MAX_CHAT_HISTORY - 1)),
          { user_id: msg.user_id, name: msg.name, text: msg.text, ts: msg.ts },
        ]);
        break;
      case 'PeerCursor':
        break;
      case 'InputDenied':
        handleInputDenied(msg.reason);
        break;
      case 'Error':
        handleServerError(msg.code, msg.message);
        break;
    }
  };

  // De-dupe toast id: we only want one "input blocked" toast active at a
  // time even if the server sends multiple `InputDenied` frames across
  // reconnects.
  const INPUT_DENIED_TOAST_ID = 'input-denied';
  // `string & {}` keeps the autocomplete from the literal union while
  // still accepting any string the server might send in a future
  // protocol revision — the `default` branch is the forward-compat path.
  const handleInputDenied = (reason: InputDeniedReason | (string & {})) => {
    switch (reason) {
      case InputDeniedReason.VIEWER:
        toast.info(t('session.toast_input_denied_viewer'), {
          id: INPUT_DENIED_TOAST_ID,
          duration: 5000,
        });
        break;
      case InputDeniedReason.SERIALIZED_NOT_OWNER:
        toast.info(t('session.toast_input_denied_solo'), {
          id: INPUT_DENIED_TOAST_ID,
          duration: 5000,
        });
        break;
      default:
        toast.info(t('session.toast_input_denied_generic'), {
          id: INPUT_DENIED_TOAST_ID,
          duration: 5000,
        });
    }
  };

  const handleServerError = (code: string, message: string) => {
    switch (code) {
      case ErrorCode.SESSION_CLOSED:
        setEndedReasonKey('session.banner_ended');
        toast.info(t('session.toast_session_ended'), { duration: 4000 });
        break;
      case ErrorCode.SESSION_NOT_FOUND:
        setEndedReasonKey('session.banner_not_found');
        break;
      case ErrorCode.NOT_PARTICIPANT:
        setErrorKey('session.banner_not_participant');
        setErrorText('');
        break;
      case ErrorCode.STORAGE_ERROR:
        // Transient DB hiccup, not a permission problem — the banner
        // prompts a retry and the reconnect loop handles the rest.
        setErrorKey('session.banner_storage_error');
        setErrorText('');
        break;
      case ErrorCode.AUTH_FAILED:
      case ErrorCode.AUTH_TIMEOUT:
        // Token is invalid/expired — clear auth state so AuthGuard redirects
        // to /login. A banner would be invisible since this page is about to
        // unmount, so surface the reason via a global toast instead.
        toast.error(t('session.toast_auth_failed'));
        auth.logout();
        break;
      default:
        // Server-side error code + raw message; the code is locale-neutral
        // and the message comes from the server in English by design.
        setErrorKey(null);
        setErrorText(`${code}: ${message}`);
    }
  };

  const handleStatus = (s: ConnectionStatus) => {
    if (s === 'connected') {
      if (hasConnectedOnce) {
        toast.success(t('session.toast_reconnected'), { id: 'reconnect' });
      }
      hasConnectedOnce = true;
    } else if (s === 'giveup') {
      toast.error(t('session.toast_giveup'), {
        id: 'reconnect',
        action: { label: t('session.toast_giveup_retry'), onClick: () => socket?.reconnectNow() },
      });
    }
    setStatus(s);
  };

  const handleData = (data: string) => {
    if (!canInput(role(), inputMode())) {
      // Client-side pre-filter: if we don't send the bytes at all, the
      // server will never reply with `InputDenied` — so the denial
      // toast would never fire and the user sees a dead keyboard with
      // zero feedback. Reuse the exact same denial handler the server
      // path uses so both code paths share copy, de-dupe id, and
      // duration. Reason picked to match the server's categorisation.
      const reason =
        role() === 'viewer'
          ? InputDeniedReason.VIEWER
          : InputDeniedReason.SERIALIZED_NOT_OWNER;
      handleInputDenied(reason);
      return;
    }
    socket?.sendInput(encodeInput(data));
  };

  const handleResize = (cols: number, rows: number) => {
    // Resize follows its own permission gate server-side: operators may
    // resize even in solo (`serialized`) mode. Mirror that here so we
    // don't block a legitimate resize with the stricter `canInput`
    // check — which would prevent the terminal from fitting its pane.
    if (role() === 'viewer') return;
    socket?.sendResize(cols, rows);
  };

  const handleSendChat = (text: string) => {
    socket?.send({ type: 'ChatMessage', text });
  };

  const handleManualReconnect = () => {
    toast.info(t('session.toast_reconnecting'), { id: 'reconnect', duration: 2000 });
    socket?.reconnectNow();
  };

  // Owners have a real dashboard to return to. Non-owners — guests
  // and invited operators/viewers — do not: a scoped-guest token
  // 403s on every dashboard route (`require_unscoped` in
  // `crates/telepair-gateway/src/http.rs::list_targets`), and even
  // a regular invited user has no business on the owner's
  // dashboard. Before this fix, both the topbar "← Back" button and
  // the session-ended banner action navigated to `/`, which left
  // guests stranded on a broken empty-state page that also leaked
  // the server-side config path. Route them through logout instead
  // so they land cleanly on /login and can re-redeem their invite
  // (or be done).
  const goHomeOrLogout = () => {
    if (role() === 'owner') {
      navigate('/');
      return;
    }
    auth.logoutAndRedirect();
  };

  // `connect()` is deferred until the Terminal ref fires so the
  // initial `SessionJoin` frame can carry the fit-computed cols/rows;
  // otherwise the server spawns the PTY at 80×24 and full-screen TUIs
  // render inside a tiny viewport until the first corrective resize.
  socket = new TelepairSocket(handleMessage, handleBinary, handleStatus);
  socket.onReconnectInfo = setReconnectInfo;

  onCleanup(() => {
    // Drop any sticky reconnect toast so its Retry action cannot resurrect
    // this page's socket after the user has navigated away or logged out.
    toast.dismiss('reconnect');
    socket?.disconnect();
  });

  return (
    <div class="session-page">
      <header class="session-topbar">
        <button class="back-btn" onClick={goHomeOrLogout}>
          {role() === 'owner' ? t('common.back') : t('common.logout')}
        </button>
        <span class="session-label">
          {/* Use `renderTemplate` so the session id stays inside a real
              <code> element (preserving the monospace styling) instead
              of being interpolated as plain text. Calling `t(...)`
              without any params leaves the `{{ id }}` slot literal,
              which is exactly what `renderTemplate` then splits on. */}
          {renderTemplate(t('session.label'), {
            id: <code>{params.id}</code>,
          })}
        </span>
        <span class="role-badge" data-role={role()}>{roleLabel(t, role())}</span>
        <span class="status-dot" data-status={status()} />
        <div class="topbar-actions">
          <LocaleSwitcher variant="topbar" />
          <Show when={role() === 'owner'}>
            <button class="action-btn" onClick={() => setShowInvite(true)}>{t('session.invite')}</button>
          </Show>
          <button class="action-btn" onClick={() => setSidebarOpen(!sidebarOpen())}>
            {sidebarOpen() ? t('session.sidebar_hide') : t('session.sidebar_show')}
          </button>
        </div>
      </header>

      <Show when={errorMessage()}>
        <Banner variant="error" onDismiss={dismissError}>
          {errorMessage()}
        </Banner>
      </Show>

      <Show when={endedReasonKey()}>
        {(key) => (
          <Banner
            variant="info"
            role="status"
            action={{
              label: role() === 'owner' ? t('session.banner_back_to_dashboard') : t('common.logout'),
              onClick: goHomeOrLogout,
            }}
          >
            {t(key())}
          </Banner>
        )}
      </Show>

      <Show when={!endedReasonKey() && status() === 'giveup'}>
        <Banner
          variant="error"
          role="status"
          action={{ label: t('session.banner_reconnect_action'), onClick: handleManualReconnect }}
        >
          {t('session.banner_connection_lost')}
        </Banner>
      </Show>

      <Show when={!endedReasonKey() && status() !== 'giveup' && reconnectInfo()}>
        <Banner variant="warning" role="status">
          <span>
            {t('session.banner_reconnecting', {
              attempt: String(reconnectInfo()?.attempt ?? 0),
              max: String(reconnectInfo()?.maxAttempts ?? 0),
              seconds: String(Math.round((reconnectInfo()?.nextDelayMs ?? 0) / 1000)),
            })}
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
              if (!socketOpened) {
                socketOpened = true;
                socket?.connect(params.id, auth.token(), h.cols, h.rows);
              }
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
        inputMode={inputMode()}
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
