// web/src/i18n/locales/en.ts
//
// English dictionary — the source of truth for the `Dict` type.
// All other locales must mirror this exact structure (enforced by
// `: typeof en` annotations on sibling files).
//
// Conventions:
// - Nested object groups, flattened to `domain.subdomain.key` at runtime
//   via `i18n.flatten()`.
// - Placeholders use `{{ name }}` (double curlies — required by
//   `@solid-primitives/i18n`'s `resolveTemplate`).
// - Plural forms are sibling keys (`xxx.singular` / `xxx.plural`); the
//   component picks the right one with `n === 1 ? singular : plural`.
//   No ICU runtime — overkill for two languages.
// - Brand names (`telepair`) and CLI strings inside <code> tags are NOT
//   translated; they appear in JSX directly.

export const en = {
  common: {
    cancel: 'Cancel',
    confirm: 'Confirm',
    copy: 'Copy',
    copied: 'Copied!',
    refresh: 'Refresh',
    refreshing: 'Refreshing…',
    logout: 'Logout',
    back: '← Back',
    done: 'Done',
    dismiss: 'Dismiss notification',
  },
  login: {
    subtitle: 'Web terminal collaboration',
    token_label: 'API Token',
    token_placeholder: 'Paste your token here',
    connect: 'Connect',
    validating: 'Validating...',
    help_show: "Don't have a token?",
    help_hide: 'Hide help',
    help_first_run:
      'First run? telepair prints the admin token to the server console on startup and saves it to {{ path }}.',
    help_lost: 'Lost it? Run {{ cmd }} on the server to print it again.',
    help_joining:
      "Joining a session? Just open your invite link — no token needed. You'll be signed in automatically as a guest.",
  },
  dashboard: {
    targets_heading: 'Targets',
    targets_hint: 'Click a target to configure mode and launch.',
    targets_empty_title: 'No targets available',
    targets_empty_body:
      'No targets are configured for this account. If you expected to see one here, contact your administrator or check the server logs.',
    sessions_heading: 'Sessions',
    sessions_hint:
      'Active sessions let you jump back in; closed sessions show the recent history.',
    sessions_empty_active: 'No active sessions',
    sessions_empty_closed: 'No closed sessions yet',
    sessions_empty_all: 'No sessions to show',
    sessions_tab_active: 'Active',
    sessions_tab_closed: 'Closed',
    sessions_tab_all: 'All',
    sessions_status_active: 'Active',
    sessions_closed_by_owner: 'Closed by owner',
    sessions_closed_by_reaper: 'Closed (idle timeout)',
    sessions_closed_by_startup: 'Closed on restart',
    sessions_closed_by_error: 'Closed (launch error)',
    sessions_closed_unknown: 'Closed',
    session_duration_sec: '{{ n }}s',
    session_duration_min: '{{ n }}m',
    session_duration_hr: '{{ h }}h {{ m }}m',
    refresh_aria: 'Refresh targets and sessions',
    admin_targets_link: 'Admin · Targets',
    admin_targets_link_aria: 'Open admin target management',
    sessions_filter_target: 'Filter: {{ name }}',
    sessions_filter_clear: 'Clear filter',
  },
  create_session: {
    title: 'Start a session',
    mode_label: 'Input mode',
    mode_label_aria: 'Input mode',
    mode_collaborative: 'Collaborative',
    mode_collaborative_desc: 'Invited operators can type, resize, and chat.',
    mode_solo: 'Solo',
    mode_solo_desc: 'Only you can type — guests watch and chat.',
    launch: 'Launch',
    launching: 'Launching…',
    error_failed: 'Failed to create session',
  },
  session: {
    label: 'Session: {{ id }}',
    invite: 'Invite',
    close: 'Close',
    close_confirm: 'Click again to close',
    close_failed: 'Failed to close session: {{ msg }}',
    sidebar_show: 'Show Sidebar',
    sidebar_hide: 'Hide Sidebar',
    banner_ended: 'This session has ended.',
    banner_not_found: 'Session not found — it may have been deleted.',
    banner_not_participant: 'You are not a participant of this session.',
    banner_storage_error: 'Temporary storage error — please retry in a moment.',
    banner_connection_lost: 'Connection lost. Automatic retry gave up.',
    banner_reconnecting:
      'Connection lost — retrying {{ attempt }}/{{ max }} (next in {{ seconds }}s)',
    banner_reconnect_action: 'Reconnect',
    banner_back_to_dashboard: 'Back to Dashboard',
    toast_reconnected: 'Reconnected',
    toast_reconnecting: 'Reconnecting…',
    toast_giveup: 'Could not reconnect to session',
    toast_giveup_retry: 'Retry',
    toast_session_ended: 'Session has ended',
    toast_input_denied_viewer:
      'View-only session — your keystrokes are not sent.',
    toast_input_denied_solo:
      'Solo mode — only the session owner can type here.',
    toast_input_denied_generic: 'Typing is not allowed in this session.',
    toast_auth_failed: 'Authentication failed. Please log in again.',
  },
  invite: {
    title: 'Invite to Session',
    role_label: 'Role',
    role_operator: 'Operator',
    role_operator_desc_multiplexed: 'Can type, resize, and chat',
    role_operator_desc_solo:
      'Can resize and chat (solo mode — only the owner types)',
    role_viewer: 'Viewer',
    role_viewer_desc: 'Can only watch and chat',
    max_uses_label: 'Max uses',
    max_uses_aria: 'Maximum number of redemptions',
    max_uses_one_shot: 'One-shot',
    expires_label: 'Expires',
    expires_aria: 'Expiry time',
    expires_15min: '15 min',
    expires_1hour: '1 hour',
    expires_24hours: '24 hours',
    expires_7days: '7 days',
    expires_no_expiry: 'No expiry',
    create: 'Create Invite Link',
    creating: 'Creating...',
    link_label: 'Invite Link',
    usable_singular: 'Usable {{ n }} time · expires {{ when }}.',
    usable_plural: 'Usable {{ n }} times · expires {{ when }}.',
    share_hint: 'Share this link with the person you want to invite.',
    expiry_never: 'Never (until session closes)',
    expiry_unknown: 'unknown',
    expiry_expired: 'expired',
    expiry_in_min: 'in ~{{ n }} min',
    expiry_in_hours: 'in ~{{ n }} hr',
    expiry_in_days_singular: 'in ~{{ n }} day',
    expiry_in_days_plural: 'in ~{{ n }} days',
    failed: 'Failed to create invite: {{ msg }}',
    copy_failed:
      'Could not copy automatically. Select the link and press {{ shortcut }} to copy.',
    manage_heading: 'Active invites',
    manage_empty: 'No invites yet — create one below.',
    manage_loading: 'Loading invites…',
    manage_failed: 'Failed to load invites: {{ msg }}',
    manage_remaining_singular: '{{ remaining }} of {{ total }} use left',
    manage_remaining_plural: '{{ remaining }} of {{ total }} uses left',
    manage_exhausted: 'Exhausted',
    manage_expired: 'Expired',
    manage_role_operator: 'Operator',
    manage_role_viewer: 'Viewer',
    manage_revoke: 'Revoke',
    manage_revoke_confirm: 'Revoke?',
    manage_revoke_cancel: 'Cancel',
    manage_revoke_success: 'Invite revoked',
    manage_revoke_failed: 'Failed to revoke invite: {{ msg }}',
  },
  session_detail: {
    title: 'Session details',
    target: 'Target',
    created_at: 'Started',
    closed_at: 'Ended',
    timeline_heading: 'Audit timeline',
    timeline_empty: 'No audit events recorded for this session.',
    loading: 'Loading audit timeline…',
    load_failed: 'Failed to load audit timeline: {{ msg }}',
    detail_show: 'Show JSON',
    detail_hide: 'Hide JSON',
    detail_role: 'role: {{ role }}',
    detail_reason: 'reason: {{ reason }}',
    detail_as_guest: 'as guest',
    event_session_created: 'Session created',
    event_session_closed: 'Session closed',
    event_participant_joined: 'Participant joined',
    event_invite_minted: 'Invite minted',
    event_invite_redeemed: 'Invite redeemed',
    event_invite_revoked: 'Invite revoked',
    event_target_access_denied: 'Target access denied',
    event_target_reloaded: 'Targets reloaded',
  },
  chat: {
    heading: 'Chat',
    placeholder: 'Type a message...',
    send: 'Send',
    system_joined: '{{ name }} joined the session',
    system_left: '{{ name }} left the session',
  },
  participants: {
    heading: 'Participants ({{ count }})',
  },
  // Short labels for protocol enums (Role, InputMode). Kept separate
  // from `invite.role_*` and `create_session.mode_*` because those
  // include long descriptions tailored to the dialog they live in;
  // these are bare 1-word badges for lists and topbars where space
  // is tight. Resolved at render via `roleLabel` / `inputModeLabel`
  // in `i18n/labels.ts` so callers don't repeat the switch.
  roles: {
    owner: 'Owner',
    operator: 'Operator',
    viewer: 'Viewer',
  },
  input_mode: {
    multiplexed: 'Collaborative',
    serialized: 'Solo',
  },
  join: {
    joining: 'Joining session...',
    error_invalid: 'Invalid or expired invite link',
    error_closed: 'This session has been closed',
    error_failed: 'Failed to join session',
    go_dashboard: 'Go to Dashboard',
  },
  admin_targets: {
    title: 'Target management',
    subtitle:
      'Inspect every virtual target and hot-reload targets.yaml without restarting the server.',
    reload: 'Reload targets.yaml',
    reloading: 'Reloading…',
    reload_success: 'Reloaded {{ count }} targets from {{ path }}',
    reload_failed_generic: 'Reload failed: {{ msg }}',
    reload_failed_parse: 'targets.yaml is malformed: {{ msg }}',
    reload_failed_no_path:
      'telepair was started without a targets.yaml — configure one and restart.',
    reload_failed_still_referenced_title:
      'Reload blocked: the new targets.yaml would drop targets that still have live sessions.',
    reload_failed_still_referenced_hint:
      'Close the sessions below (or restore the target in targets.yaml) and try again.',
    reload_failed_still_referenced_row: '{{ target }} — {{ count }} active session(s)',
    back_to_dashboard: '← Back to dashboard',
    loading: 'Loading targets…',
    load_failed: 'Failed to load targets: {{ msg }}',
    empty: 'No targets defined.',
    kind_virtual: 'Virtual',
    kind_local: 'Local',
    admin_only_badge: 'Admin only',
    command_label: 'Command',
    args_label: 'Args',
    args_count: '{{ n }} argument(s)',
    shell_label: 'Shell',
    tags_label: 'Tags',
    env_label: 'Environment',
    env_empty: 'No environment variables',
    env_set: 'set',
    env_unset: 'unset',
    active_sessions_singular: '{{ n }} active session',
    active_sessions_plural: '{{ n }} active sessions',
    active_sessions_zero: 'No active sessions',
    active_sessions_link_aria: 'Show active sessions for {{ name }}',
  },
  auth: {
    error_invalid_token: 'Invalid token',
    error_connection_failed: 'Connection failed',
  },
  toast: {
    region_label: 'Notifications',
  },
  locale: {
    switch_zh: '中文',
    switch_en: 'English',
    switch_aria: 'Switch language',
  },
};

/** Dictionary type — every other locale must satisfy this. Built from
 *  `typeof en` *without* `as const` so leaf values widen to `string`,
 *  which is what other languages need to satisfy. The structural keys
 *  remain checked: missing or extra keys still produce a `tsc` error. */
export type Dict = typeof en;
