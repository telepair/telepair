// web/src/pages/Session.tsx
import { createEffect, createSignal, onCleanup, onMount, Show } from 'solid-js';
import { useParams, useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { api, errorMessage as fmtError } from '../lib/api';
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
import SettingsPanel from '../components/SettingsPanel';
import RecordingIndicator from '../components/RecordingIndicator';
import ShareRecordingDialog from '../components/ShareRecordingDialog';
import { toast } from '../stores/toast';
import { terminalSettings } from '../stores/settings';
import { notify } from '../lib/notifications';
import {
  renderTemplate,
  roleLabel,
  useI18n,
  type TranslationKey,
} from '../i18n';

const MAX_CHAT_HISTORY = 500;

function shouldNotify(senderId: string): boolean {
  return (
    terminalSettings().notificationsEnabled &&
    document.visibilityState !== 'visible' &&
    senderId !== auth.currentUserId()
  );
}

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
  const [isRecording, setIsRecording] = createSignal(false);
  const [recordingId, setRecordingId] = createSignal<string | null>(null);
  const [showShareDialog, setShowShareDialog] = createSignal(false);
  // Initial sidebar state is viewport-dependent: on narrow screens
  // (<=640px matches the @media rule below) the sidebar promotes to a
  // full-width overlay drawer that covers the terminal, so defaulting
  // to open hides the actual workspace behind a participant list until
  // the user discovers the toggle. Start closed there so the terminal
  // is the first thing they see. QA finding F3-2.
  const [sidebarOpen, setSidebarOpen] = createSignal(
    typeof window === 'undefined' ? true : window.innerWidth > 640,
  );
  // Auto-close sidebar when the viewport shrinks below the overlay
  // breakpoint (640px). Without this, a user who opens the sidebar on a
  // wide screen and then rotates their phone to portrait ends up with a
  // full-screen overlay hiding the terminal and no obvious dismiss cue.
  const narrowMql =
    typeof window !== 'undefined' ? window.matchMedia('(max-width: 640px)') : null;
  const handleNarrow = (e: MediaQueryListEvent) => {
    if (e.matches) setSidebarOpen(false);
  };
  narrowMql?.addEventListener('change', handleNarrow);
  onCleanup(() => narrowMql?.removeEventListener('change', handleNarrow));
  // Close-session "armed" latch: the first click flips this on and
  // swaps the button label to a confirmation prompt; a second click
  // within 3s actually closes. The modeless inline-confirm pattern
  // avoids a native `window.confirm()` (which looks cheap) and also
  // dodges the need for a whole ConfirmDialog component for one
  // button. Stored as a signal so Solid rerenders the label reactively.
  const [closeArmed, setCloseArmed] = createSignal(false);
  const [closing, setClosing] = createSignal(false);
  let closeDisarmTimer: ReturnType<typeof setTimeout> | undefined;
  let hasConnectedOnce = false;

  const [termHandle, setTermHandle] = createSignal<TerminalHandle | null>(null);
  // Set true once SessionState (or any role message) lands, so the
  // readOnly effect doesn't lock the terminal on the default
  // pre-state 'viewer' value.
  const [rolePinned, setRolePinned] = createSignal(false);
  let pendingOutput: Uint8Array[] = [];
  // Latches after the first `SessionState` seeds the chat panel from
  // the server backlog. Subsequent reconnect-triggered SessionState
  // frames must NOT overwrite the local chat — by that point live
  // `PeerChat` broadcasts have populated entries the server backlog
  // may have already dropped past its cap, and locally synthesized
  // system notices (join/leave) don't exist server-side.
  let chatSeeded = false;
  let socket: TelepairSocket | undefined;
  // One-shot latch so HMR/StrictMode re-entry into the Terminal ref
  // callback doesn't reopen the WebSocket.
  let socketOpened = false;

  const handleBinary = (data: Uint8Array) => {
    const th = termHandle();
    if (th) {
      th.write(data);
    } else {
      pendingOutput.push(data);
    }
  };

  // Push a synthetic "system" entry into the chat stream (join/leave
  // notices). Kept to the same 500-item ring as regular chat so
  // long-lived sessions with lots of churn can't balloon memory.
  // `idSuffix` keeps the For key stable per-event so Solid doesn't
  // see two system messages as the same row.
  const appendSystemChat = (idSuffix: string, text: string) => {
    setChatMessages((prev) => [
      ...prev.slice(-(MAX_CHAT_HISTORY - 1)),
      {
        user_id: `__system__:${idSuffix}:${Date.now()}`,
        name: '',
        text,
        ts: new Date().toISOString(),
        kind: 'system',
      },
    ]);
  };

  const handleMessage = (msg: ServerMessage) => {
    switch (msg.type) {
      case 'SessionState':
        setRole(msg.your_role);
        setRolePinned(true);
        setInputMode(msg.session.input_mode);
        setParticipants(msg.participants);
        // Sync recording status from the session snapshot.
        if (msg.recording != null) {
          setIsRecording(true);
          setRecordingId(msg.recording.recording_id);
        } else {
          setIsRecording(false);
          setRecordingId(null);
        }
        // Seed the chat panel from the server's bounded backlog on the
        // FIRST SessionState only. A reconnect delivers SessionState
        // again, but by then local state is richer than the server
        // snapshot (system notices, plus live entries that aged out of
        // the server cap) — overwriting would look like data loss to
        // the user.
        if (!chatSeeded) {
          chatSeeded = true;
          if (msg.chat_history.length > 0) {
            setChatMessages(msg.chat_history.slice(-MAX_CHAT_HISTORY));
          }
        }
        break;
      case 'PeerJoined': {
        const entry = {
          user_id: msg.user_id,
          name: msg.name,
          role: msg.role,
          color: msg.color,
        };
        setParticipants((prev) =>
          prev.some((p) => p.user_id === entry.user_id)
            ? prev.map((p) => (p.user_id === entry.user_id ? entry : p))
            : [...prev, entry],
        );
        appendSystemChat(
          `peer-joined:${msg.user_id}`,
          t('chat.system_joined', { name: msg.name }),
        );
        if (shouldNotify(msg.user_id)) {
          notify('telepair', t('notifications.joined', { name: msg.name }));
        }
        break;
      }
      case 'PeerLeft':
        // Read the leaving peer's name from the current participant
        // list BEFORE we prune it, otherwise the system message falls
        // back to the user_id and looks like a debug log.
        {
          const leaving = participants().find((p) => p.user_id === msg.user_id);
          const leavingName = leaving?.name ?? msg.user_id;
          setParticipants((prev) => prev.filter((p) => p.user_id !== msg.user_id));
          appendSystemChat(
            `peer-left:${msg.user_id}`,
            t('chat.system_left', { name: leavingName }),
          );
        }
        break;
      case 'PeerEvicted':
        // Force-removal of a participant. We prune the row the same
        // way `PeerLeft` does, but swap the system chat string based
        // on `reason` so collaborators don't mistake a routine
        // password rotation for an admin action:
        //   - `account_disabled` → "was removed by an admin"
        //   - `token_rotated`    → "re-authenticated" (neutral)
        // The server follows this frame with `Close(CLOSE_CODE_TERMINAL)`,
        // but that close only flips `status` to 'error' — it never
        // navigates. When the evicted user is US, route proactively
        // so the tab doesn't linger with a stale OWNER badge and a
        // dead terminal (surface-map §6 item 15 split).
        {
          const leaving = participants().find((p) => p.user_id === msg.user_id);
          const leavingName = leaving?.name ?? msg.user_id;
          setParticipants((prev) => prev.filter((p) => p.user_id !== msg.user_id));
          const chatKey: TranslationKey =
            msg.reason === 'token_rotated'
              ? 'chat.system_reauth_required'
              : 'chat.system_evicted';
          appendSystemChat(`peer-evicted:${msg.user_id}`, t(chatKey, { name: leavingName }));
          if (msg.user_id === auth.currentUserId()) {
            if (msg.reason === 'token_rotated') {
              toast.info(t('session.toast_evicted_token_rotated'), { duration: 4000 });
              auth.logoutAndRedirect();
            } else {
              // `account_disabled` (and any future reason we haven't named
              // yet): keep the credential — the user is still logged in,
              // just no longer session-enabled — and route home so the
              // Dashboard's pending-approval banner is the landing UI.
              // `refreshIdentity` fires in the background so the banner
              // reflects the freshly-disabled `session_enabled=false`
              // instead of the stale cached value.
              toast.info(t('session.toast_evicted_account_disabled'), { duration: 4000 });
              void auth.refreshIdentity();
              navigate('/');
            }
          }
        }
        break;
      case 'PeerChat':
        setChatMessages((prev) => [
          ...prev.slice(-(MAX_CHAT_HISTORY - 1)),
          { user_id: msg.user_id, name: msg.name, text: msg.text, ts: msg.ts },
        ]);
        if (shouldNotify(msg.user_id)) {
          notify('telepair', `${msg.name}: ${msg.text}`);
        }
        break;
      case 'PeerRoleChanged':
        setParticipants((prev) =>
          prev.map((p) =>
            p.user_id === msg.user_id ? { ...p, role: msg.new_role } : p,
          ),
        );
        // If it's our own role that changed, update the local signal
        // so canInput / toolbar badges react immediately, and surface
        // a proactive toast — without one, a demoted viewer sees a
        // dead textarea with no explanation until they try to type.
        if (msg.user_id === auth.currentUserId()) {
          const prev = role();
          setRole(msg.new_role);
          if (prev !== msg.new_role) {
            if (msg.new_role === 'viewer') {
              toast.warning(t('session.toast_role_demoted_to_viewer'), {
                id: ROLE_CHANGE_TOAST_ID,
                duration: 5000,
              });
            } else if (msg.new_role === 'operator') {
              toast.info(t('session.toast_role_promoted_to_operator'), {
                id: ROLE_CHANGE_TOAST_ID,
                duration: 4000,
              });
            }
          }
        }
        break;
      case 'PeerCursor':
        break;
      case 'InputDenied':
        handleInputDenied(msg.reason);
        break;
      case 'Error':
        handleServerError(msg.code, msg.message);
        break;
      case 'RecordingStarted':
        setIsRecording(true);
        setRecordingId(msg.recording_id);
        break;
      case 'RecordingStopped':
        setIsRecording(false);
        setRecordingId(msg.recording_id);
        break;
    }
  };

  // De-dupe toast id: we only want one "input blocked" toast active at a
  // time even if the server sends multiple `InputDenied` frames across
  // reconnects.
  const INPUT_DENIED_TOAST_ID = 'input-denied';
  // Separate dedupe slot for role-transition toasts so a rapid
  // demote→promote sequence (owner clicks the wrong option then
  // corrects) doesn't leave the stale "you are now Viewer" hanging.
  const ROLE_CHANGE_TOAST_ID = 'role-change';
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

  const handleRoleChange = async (userId: string, newRole: Role) => {
    // Resolve the name from the current participant list BEFORE the
    // async call; by the time the toast fires the user may have left
    // and been pruned, which would render "Role for  set to Viewer".
    const name = participants().find((p) => p.user_id === userId)?.name ?? '';
    try {
      await api.updateParticipantRole(params.id, userId, newRole);
      toast.success(
        t('session.role_change_success', { name, role: roleLabel(t, newRole) }),
        { duration: 3000 },
      );
    } catch (e) {
      toast.error(t('session.role_change_failed', { msg: fmtError(e) }));
    }
  };

  const handleManualReconnect = () => {
    toast.info(t('session.toast_reconnecting'), { id: 'reconnect', duration: 2000 });
    socket?.reconnectNow();
  };

  // Two-step close flow:
  //   1. First click arms the button — label flips to "Click again
  //      to close" and a 3s timer disarms it so an accidental single
  //      click can't leak into a later real intent to close.
  //   2. Second click within the window calls DELETE /api/sessions/{id}.
  //      On success we do NOT navigate — the server broadcasts a
  //      SESSION_CLOSED frame over WS which triggers the existing
  //      `endedReasonKey` banner with a "Back to Dashboard" action.
  //      This keeps all participants (including the closer) on the
  //      same exit path.
  const handleCloseSession = async () => {
    if (closing()) return;
    if (!closeArmed()) {
      setCloseArmed(true);
      clearTimeout(closeDisarmTimer);
      closeDisarmTimer = setTimeout(() => setCloseArmed(false), 3000);
      return;
    }
    clearTimeout(closeDisarmTimer);
    setClosing(true);
    try {
      await api.closeSession(params.id);
      // Success — banner will land via WS SESSION_CLOSED. Leave the
      // button in "closing" state so it can't be re-clicked.
    } catch (e) {
      toast.error(t('session.close_failed', { msg: fmtError(e) }));
      setClosing(false);
      setCloseArmed(false);
    }
  };

  const handleStopRecording = async () => {
    try {
      await api.stopRecording(params.id);
      // State will update via RecordingStopped WS message.
    } catch (e) {
      toast.error(`Failed to stop recording: ${fmtError(e)}`);
    }
  };

  // Return button dispatch: the choice between "back to dashboard"
  // and "log out" is a property of the CREDENTIAL, not the session
  // role. A scoped-guest token is only valid for this one session
  // (`require_unscoped` 403s on every dashboard route), so a guest
  // has nowhere else to go and must be routed to /login. A real
  // logged-in user — including an admin who joined someone else's
  // session as a non-owner to test an invite link, or an operator
  // who was invited into a peer's session — still has their own
  // account and dashboard to return to, and must NOT be logged out
  // on the way back.
  //
  // Previously this was keyed off `role() === 'owner'`, which
  // silently logged out any non-owner real user. That was a
  // regression against the backend's own `redeem_invite` design:
  // the handler explicitly preserves an existing identity when a
  // logged-in caller redeems an invite, exactly so admins can test
  // their own invite links without spawning throwaway guests.
  //
  // `auth.currentUserIsGuest()` returns `null` until the first
  // successful `whoami`; in that ambiguous state we default to
  // "navigate to dashboard" because (a) real users are the common
  // case, and (b) a guest who slips through lands on the dashboard,
  // whose first data fetch 403s through the global interceptor and
  // bounces them to /login anyway.
  const isGuestCredential = () => auth.currentUserIsGuest() === true;
  const goHomeOrLogout = () => {
    if (isGuestCredential()) {
      auth.logoutAndRedirect();
      return;
    }
    navigate('/');
  };

  // Deep-linked session pages may mount with no prior whoami — the
  // user could have pasted a URL directly into a new tab. Trigger
  // identity load so the return-button dispatch above reads a
  // populated `isGuest` rather than falling through to the
  // null-default. `loadIdentity` is idempotent and de-duped, so
  // this is safe if Dashboard or AdminGuard already kicked one off.
  onMount(() => {
    void auth.loadIdentity();
  });

  // Keep the terminal's read-only state in sync with the live role.
  // Viewers (and anyone briefly in the `viewer` state during role
  // transitions) get an inert xterm so stray keystrokes can't bypass
  // the Session.tsx canInput pre-filter — e.g. via a clipboard paste
  // which doesn't go through the keyboard event path at all.
  //
  // The effect gates on `role() !== 'viewer'` *after* the first
  // SessionState lands. role() defaults to 'viewer' at mount (the
  // safer preclusion noted above for inputMode), but applying that
  // default would inadvertently lock the terminal for every owner /
  // operator during the brief gap between WS open and SessionState
  // arrival. `rolePinned` flips true on the first real role write so
  // the pre-pin default is "unlocked" for non-viewers.
  createEffect(() => {
    const th = termHandle();
    if (!th) return;
    const shouldLock = rolePinned() && role() === 'viewer';
    th.setReadOnly(shouldLock);
  });

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
    clearTimeout(closeDisarmTimer);
    socket?.disconnect();
  });

  return (
    <div class="session-page">
      <header class="session-topbar">
        <button class="back-btn" onClick={goHomeOrLogout}>
          {isGuestCredential() ? t('common.leave_session') : t('common.back')}
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
          <RecordingIndicator
            isRecording={isRecording()}
            isOwner={role() === 'owner'}
            onStop={handleStopRecording}
          />
          <LocaleSwitcher variant="topbar" />
          <SettingsPanel />
          <Show when={role() === 'owner' && !endedReasonKey()}>
            <button class="action-btn" onClick={() => setShowInvite(true)}>{t('session.invite')}</button>
            <Show when={recordingId() && !isRecording()}>
              <button class="action-btn" onClick={() => setShowShareDialog(true)}>Share Rec</button>
            </Show>
            <button
              class={closeArmed() ? 'action-btn danger armed' : 'action-btn danger'}
              onClick={handleCloseSession}
              disabled={closing()}
            >
              {closeArmed() ? t('session.close_confirm') : t('session.close')}
            </button>
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
              label: isGuestCredential()
                ? t('common.leave_session')
                : t('session.banner_back_to_dashboard'),
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
        <div class="terminal-container" data-readonly={rolePinned() && role() === 'viewer' ? 'true' : undefined}>
          {/* Persistent viewer indicator. The role-change toast only
              lingers ~5 seconds; once it clears, a demoted user hitting
              keys sees a dead prompt with no feedback (QA finding
              F4-3). The pinned badge sits in the corner of the
              terminal container so the read-only state is always
              visible without stealing screen space or capturing
              focus. */}
          <Show when={rolePinned() && role() === 'viewer'}>
            <div class="terminal-readonly-badge" role="status" aria-live="polite">
              {t('session.viewer_readonly_badge')}
            </div>
          </Show>
          <Terminal
            onData={handleData}
            onResize={handleResize}
            ref={(h) => {
              setTermHandle(h);
              for (const data of pendingOutput) h.write(data);
              pendingOutput = [];
              if (!socketOpened) {
                socketOpened = true;
                socket?.connect(params.id, auth.token(), h.cols, h.rows);
              }
            }}
          />
        </div>

        {/* Sidebar is hidden via CSS (not <Show>) so ChatPanel stays
            mounted and preserves unsent draft text across toggles. */}
        <div class="sidebar-backdrop" classList={{ hidden: !sidebarOpen() }} onClick={() => setSidebarOpen(false)} />
        <aside class="sidebar" classList={{ hidden: !sidebarOpen() }}>
          <div class="sidebar-section">
            <ParticipantList
              participants={participants()}
              isOwner={role() === 'owner'}
              onRoleChange={handleRoleChange}
            />
          </div>
          <div class="sidebar-section chat-section">
            <ChatPanel messages={chatMessages()} onSend={handleSendChat} />
          </div>
        </aside>
      </div>

      <InviteDialog
        sessionId={params.id}
        inputMode={inputMode()}
        open={showInvite()}
        onClose={() => setShowInvite(false)}
      />

      <Show when={showShareDialog() && recordingId()}>
        <ShareRecordingDialog
          recordingId={recordingId()!}
          onClose={() => setShowShareDialog(false)}
        />
      </Show>

      <style>{`
        .session-page {
          display: flex; flex-direction: column; height: 100vh;
          /* Mobile viewports otherwise overflow horizontally when the
             topbar buttons and fixed-width sidebar sum wider than the
             viewport; belt-and-suspenders against a stray long token
             or long session id string. */
          overflow-x: hidden;
        }
        .session-topbar {
          display: flex; align-items: center; gap: 12px; flex-wrap: wrap;
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
        .topbar-actions .action-btn.danger {
          color: var(--error);
          border-color: rgba(248, 81, 73, 0.4);
        }
        .topbar-actions .action-btn.danger:hover {
          background: rgba(248, 81, 73, 0.1);
          border-color: var(--error);
        }
        .topbar-actions .action-btn.danger.armed {
          background: var(--error);
          color: #fff;
          border-color: var(--error);
          animation: close-pulse 1s ease-in-out infinite;
        }
        @keyframes close-pulse {
          0%, 100% { box-shadow: 0 0 0 0 rgba(248, 81, 73, 0.5); }
          50%      { box-shadow: 0 0 0 4px rgba(248, 81, 73, 0); }
        }
        .session-body { flex: 1; display: flex; overflow: hidden; position: relative; }
        .terminal-container { flex: 1; padding: 4px; overflow: hidden; min-width: 0; position: relative; }
        /* Pinned read-only badge for viewer role (F4-3). Positioned
           over the terminal so it never scrolls out of view, but
           pointer-events:none so it doesn't intercept clicks into the
           xterm area below. */
        .terminal-readonly-badge {
          position: absolute;
          top: 10px;
          right: 14px;
          z-index: 5;
          padding: 3px 10px;
          border-radius: 10px;
          background: rgba(210, 153, 34, 0.15);
          color: #e3b341;
          border: 1px solid rgba(227, 179, 65, 0.55);
          font-size: 12px;
          font-weight: 500;
          pointer-events: none;
          user-select: none;
          letter-spacing: 0.02em;
        }
        .sidebar-backdrop { display: none; }
        .sidebar-backdrop.hidden, .sidebar.hidden { display: none !important; }
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

        /* Narrow viewports: the 260px fixed sidebar leaves <120px
           for the terminal on a phone, and forces horizontal scroll.
           Promote the sidebar to a full-width overlay drawer so the
           terminal keeps its whole width; toggling "Show sidebar" in
           the topbar still works as the same show/hide control.
           The topbar's outer flex-wrap lets topbar-actions drop to a
           second row, but the action cluster itself is also a single
           flex line — when the owner close button toggles to the
           armed label (~140px) the Locale/Invite/Close/Show-Sidebar
           row exceeds 375px and overflow-x:hidden clips the
           rightmost controls. Add an inner wrap and right-align so
           every button stays tappable.
           The backdrop sits behind the drawer (z-index 9 < sidebar 10)
           so tapping outside the sidebar dismisses it. The matchMedia
           listener in the component auto-closes the sidebar when the
           viewport shrinks below the breakpoint. */
        @media (max-width: 640px) {
          .sidebar-backdrop {
            display: block;
            position: absolute; inset: 0;
            background: rgba(0, 0, 0, 0.4);
            z-index: 9;
          }
          .sidebar {
            position: absolute; top: 0; right: 0; bottom: 0;
            width: 80%; max-width: 320px;
            border-left: none; z-index: 10;
          }
          .topbar-actions {
            flex-wrap: wrap;
            justify-content: flex-end;
            row-gap: 6px;
          }
        }
      `}</style>
    </div>
  );
}
