import { createEffect, createSignal, For, Show } from 'solid-js';
import { api } from '../lib/api';
import type { InviteSummary, Role, InputMode } from '../lib/protocol';
import { toast } from '../stores/toast';
import { useI18n, type Translator, type TranslationKey } from '../i18n';

interface InviteDialogProps {
  sessionId: string;
  /** Session's input mode — drives the role description text so
   * guests aren't promised the ability to type in a solo session. */
  inputMode: InputMode;
  open: boolean;
  onClose: () => void;
}

/** Preset TTL choices surfaced in the UI. Matches the server's hard
 *  cap of 7 days; anything longer would 400 anyway. A `null` value
 *  means "never expire on its own" (still bounded by max_uses and
 *  session lifetime). The label is an i18n key resolved at render. */
const TTL_PRESETS: Array<{ key: TranslationKey; minutes: number | null }> = [
  { key: 'invite.expires_15min', minutes: 15 },
  { key: 'invite.expires_1hour', minutes: 60 },
  { key: 'invite.expires_24hours', minutes: 24 * 60 },
  { key: 'invite.expires_7days', minutes: 7 * 24 * 60 },
  { key: 'invite.expires_no_expiry', minutes: null },
];

/** `max_uses` presets. The server caps at 100 so higher values round-
 *  trip as 400; surface the safe range directly instead of a free-form
 *  number field that invites typos. */
const MAX_USES_PRESETS: number[] = [1, 3, 5, 10, 25];

/** Format an ISO expiry timestamp into a human phrase. Takes the
 *  translator so the output is locale-aware; called from the render
 *  body so it re-runs (and re-translates) on every locale change. */
function formatExpiry(t: Translator, iso: string | null): string {
  if (!iso) return t('invite.expiry_never');
  const when = new Date(iso);
  if (Number.isNaN(when.getTime())) return t('invite.expiry_unknown');
  const diffMs = when.getTime() - Date.now();
  if (diffMs <= 0) return t('invite.expiry_expired');
  const minutes = Math.round(diffMs / 60_000);
  if (minutes < 60) return t('invite.expiry_in_min', { n: String(minutes) });
  const hours = Math.round(minutes / 60);
  if (hours < 48) return t('invite.expiry_in_hours', { n: String(hours) });
  const days = Math.round(hours / 24);
  return days === 1
    ? t('invite.expiry_in_days_singular', { n: String(days) })
    : t('invite.expiry_in_days_plural', { n: String(days) });
}

export default function InviteDialog(props: InviteDialogProps) {
  const { t } = useI18n();
  const [role, setRole] = createSignal<Role>('operator');
  const [maxUses, setMaxUses] = createSignal<number>(1);
  // `null` means "no TTL" — distinct from `undefined` so we don't
  // accidentally send `expires_in_minutes: undefined` from a
  // half-initialised dialog.
  const [ttlMinutes, setTtlMinutes] = createSignal<number | null>(60);
  const [inviteUrl, setInviteUrl] = createSignal('');
  const [inviteExpiresAt, setInviteExpiresAt] = createSignal<string | null>(null);
  const [inviteMaxUses, setInviteMaxUses] = createSignal<number>(1);
  const [creating, setCreating] = createSignal(false);
  const [copied, setCopied] = createSignal(false);
  // Management state: the list of existing invites the owner can
  // audit and revoke. Loaded when the dialog opens and after every
  // successful create / revoke so the two surfaces never diverge.
  const [invites, setInvites] = createSignal<InviteSummary[]>([]);
  const [invitesLoading, setInvitesLoading] = createSignal(false);
  const [invitesError, setInvitesError] = createSignal<string | null>(null);
  // Two-step revoke confirmation — same UX shape as "close session"
  // so owners don't accidentally kill a live link by misclicking.
  const [pendingRevoke, setPendingRevoke] = createSignal<string | null>(null);
  // Ref to the readonly <input> so the execCommand fallback can
  // select it — picking text from an unfocused input is a no-op on
  // most browsers.
  let urlInputRef: HTMLInputElement | undefined;

  const loadInvites = async () => {
    setInvitesLoading(true);
    setInvitesError(null);
    try {
      const rows = await api.listInvites(props.sessionId);
      setInvites(rows);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setInvitesError(msg);
    } finally {
      setInvitesLoading(false);
    }
  };

  // Re-load whenever the dialog becomes visible. `open` is the signal
  // the parent flips — we deliberately do NOT clear the list on close
  // so a fast re-open shows stale-but-useful rows while the refresh
  // flight is in progress.
  createEffect(() => {
    if (props.open) {
      void loadInvites();
    }
  });

  const handleCreate = async () => {
    setCreating(true);
    try {
      const ttl = ttlMinutes();
      const invite = await api.createInvite(props.sessionId, role(), {
        maxUses: maxUses(),
        expiresInMinutes: ttl === null ? undefined : ttl,
      });
      const url = `${location.origin}/join/${invite.token}`;
      setInviteUrl(url);
      setInviteExpiresAt(invite.expires_at);
      setInviteMaxUses(invite.max_uses);
      // Refresh the management list so the newly-minted row appears
      // the moment the owner navigates back from the copy-link view.
      void loadInvites();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      toast.error(t('invite.failed', { msg }));
    } finally {
      setCreating(false);
    }
  };

  const handleRevoke = async (tokenSha256: string) => {
    try {
      await api.revokeInvite(props.sessionId, tokenSha256);
      setPendingRevoke(null);
      toast.success(t('invite.manage_revoke_success'));
      // Optimistic local drop → immediate visual feedback → then a
      // full refresh to reconcile with the server (covers the
      // concurrent-revoke-from-another-tab case).
      setInvites((rows) => rows.filter((r) => r.token_sha256 !== tokenSha256));
      void loadInvites();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      toast.error(t('invite.manage_revoke_failed', { msg }));
      // Fall through to a refresh even on error — a 400 here means
      // the row was already revoked, and the refresh will drop it.
      void loadInvites();
    }
  };

  const remainingLabel = (row: InviteSummary): string => {
    if (row.remaining_uses <= 0) return t('invite.manage_exhausted');
    if (row.expires_at) {
      const when = new Date(row.expires_at).getTime();
      if (!Number.isNaN(when) && when <= Date.now()) return t('invite.manage_expired');
    }
    return t(
      row.remaining_uses === 1 ? 'invite.manage_remaining_singular' : 'invite.manage_remaining_plural',
      { remaining: String(row.remaining_uses), total: String(row.max_uses) },
    );
  };

  const roleBadge = (role: Role): string => {
    if (role === 'operator') return t('invite.manage_role_operator');
    if (role === 'viewer') return t('invite.manage_role_viewer');
    return role;
  };

  // Copy strategy:
  //   1. Prefer `navigator.clipboard.writeText` — works in secure
  //      contexts (localhost, HTTPS).
  //   2. Fall back to selecting the readonly input and firing
  //      `document.execCommand('copy')` — this is the only path that
  //      works when telepair is accessed over a LAN IP
  //      (`http://192.168.x.x:7700`), which is NOT a secure context
  //      and where `navigator.clipboard` is `undefined`. Without this
  //      fallback the first await throws, the exception is swallowed
  //      by the button's onClick handler, and the user experiences
  //      "Copy doesn't do anything" — the original bug report.
  //   3. If both fail, surface a toast telling the user to copy
  //      manually instead of silently failing.
  const handleCopy = async () => {
    const url = inviteUrl();
    if (!url) return;
    // Always select first so the user can manually copy even if
    // every programmatic path fails.
    urlInputRef?.focus();
    urlInputRef?.select();

    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(url);
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
        return;
      }
    } catch {
      // Fall through to execCommand fallback.
    }

    try {
      // execCommand is deprecated but still the de-facto fallback for
      // non-secure contexts; browsers won't remove it without a
      // replacement because of exactly this use case.
      if (document.execCommand && document.execCommand('copy')) {
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
        return;
      }
    } catch {
      // Fall through to toast.
    }

    // Platform-appropriate keyboard hint. Mac users see ⌘C, everyone
    // else sees Ctrl+C.
    const isMac =
      typeof navigator !== 'undefined' && /Mac|iPhone|iPad/i.test(navigator.platform);
    toast.error(
      t('invite.copy_failed', { shortcut: isMac ? '⌘C' : 'Ctrl+C' }),
    );
  };

  const handleClose = () => {
    setInviteUrl('');
    setInviteExpiresAt(null);
    setCopied(false);
    props.onClose();
  };

  return (
    <Show when={props.open}>
      <div class="dialog-backdrop" onClick={handleClose}>
        <div class="dialog" onClick={(e) => e.stopPropagation()}>
          <h3>{t('invite.title')}</h3>
          <Show when={!inviteUrl()} fallback={
            <div class="invite-result">
              <label>{t('invite.link_label')}</label>
              <div class="invite-url-row">
                <input
                  ref={urlInputRef}
                  type="text"
                  value={inviteUrl()}
                  readonly
                  onClick={(e) => e.currentTarget.select()}
                />
                <button class="primary" onClick={handleCopy}>
                  {copied() ? t('common.copied') : t('common.copy')}
                </button>
              </div>
              <p class="hint">
                {t(
                  inviteMaxUses() === 1 ? 'invite.usable_singular' : 'invite.usable_plural',
                  { n: String(inviteMaxUses()), when: formatExpiry(t, inviteExpiresAt()) },
                )}
              </p>
              <p class="hint">{t('invite.share_hint')}</p>
              <button onClick={handleClose} style={{ 'margin-top': '12px', width: '100%' }}>{t('common.done')}</button>
            </div>
          }>
            <div class="invite-form">
              <div class="invite-manage">
                <label class="invite-manage-label">
                  {t('invite.manage_heading')}
                </label>
                <Show when={invitesError()}>
                  <p class="manage-error">
                    {t('invite.manage_failed', { msg: invitesError() ?? '' })}
                  </p>
                </Show>
                <Show when={invitesLoading() && invites().length === 0}>
                  <p class="hint">{t('invite.manage_loading')}</p>
                </Show>
                <Show when={!invitesLoading() && invites().length === 0}>
                  <p class="hint">{t('invite.manage_empty')}</p>
                </Show>
                <ul class="invite-list" data-testid="invite-list">
                  <For each={invites()}>
                    {(row) => (
                      <li class="invite-row" data-testid="invite-row">
                        <div class="invite-row-main">
                          <span
                            class="invite-row-prefix"
                            data-testid="invite-prefix"
                            title={row.token_sha256}
                          >
                            {row.token_prefix}
                          </span>
                          <span class={`role-badge role-badge-${row.role}`}>
                            {roleBadge(row.role)}
                          </span>
                          <span
                            class="invite-row-remaining"
                            data-testid="invite-remaining"
                          >
                            {remainingLabel(row)}
                          </span>
                        </div>
                        <Show
                          when={pendingRevoke() === row.token_sha256}
                          fallback={
                            <button
                              type="button"
                              class="invite-revoke-btn"
                              data-testid="invite-revoke"
                              onClick={() => setPendingRevoke(row.token_sha256)}
                            >
                              {t('invite.manage_revoke')}
                            </button>
                          }
                        >
                          <div class="invite-revoke-confirm">
                            <button
                              type="button"
                              class="invite-revoke-yes"
                              data-testid="invite-revoke-confirm"
                              onClick={() => handleRevoke(row.token_sha256)}
                            >
                              {t('invite.manage_revoke_confirm')}
                            </button>
                            <button
                              type="button"
                              class="invite-revoke-no"
                              onClick={() => setPendingRevoke(null)}
                            >
                              {t('invite.manage_revoke_cancel')}
                            </button>
                          </div>
                        </Show>
                      </li>
                    )}
                  </For>
                </ul>
              </div>

              <label>{t('invite.role_label')}</label>
              <div class="role-options">
                <button
                  class={role() === 'operator' ? 'role-btn active' : 'role-btn'}
                  onClick={() => setRole('operator')}
                >
                  {t('invite.role_operator')}
                  <span class="role-desc">
                    {props.inputMode === 'multiplexed'
                      ? t('invite.role_operator_desc_multiplexed')
                      : t('invite.role_operator_desc_solo')}
                  </span>
                </button>
                <button
                  class={role() === 'viewer' ? 'role-btn active' : 'role-btn'}
                  onClick={() => setRole('viewer')}
                >
                  {t('invite.role_viewer')}
                  <span class="role-desc">{t('invite.role_viewer_desc')}</span>
                </button>
              </div>

              <label>{t('invite.max_uses_label')}</label>
              <div class="chip-row" role="radiogroup" aria-label={t('invite.max_uses_aria')}>
                {MAX_USES_PRESETS.map((n) => (
                  <button
                    type="button"
                    role="radio"
                    aria-checked={maxUses() === n}
                    class={maxUses() === n ? 'chip active' : 'chip'}
                    onClick={() => setMaxUses(n)}
                  >
                    {n === 1 ? t('invite.max_uses_one_shot') : `${n}`}
                  </button>
                ))}
              </div>

              <label>{t('invite.expires_label')}</label>
              <div class="chip-row" role="radiogroup" aria-label={t('invite.expires_aria')}>
                {TTL_PRESETS.map((preset) => (
                  <button
                    type="button"
                    role="radio"
                    aria-checked={ttlMinutes() === preset.minutes}
                    class={ttlMinutes() === preset.minutes ? 'chip active' : 'chip'}
                    onClick={() => setTtlMinutes(preset.minutes)}
                  >
                    {t(preset.key)}
                  </button>
                ))}
              </div>

              <button class="primary" onClick={handleCreate} disabled={creating()} style={{ width: '100%', 'margin-top': '16px' }}>
                {creating() ? t('invite.creating') : t('invite.create')}
              </button>
            </div>
          </Show>
          <style>{`
            .dialog-backdrop { position: fixed; inset: 0; background: rgba(0,0,0,0.5); display: flex; align-items: center; justify-content: center; z-index: 100; }
            .dialog { background: var(--bg-secondary); border: 1px solid var(--border); border-radius: 12px; padding: 24px; width: 440px; max-width: 90vw; }
            .dialog h3 { font-size: 16px; font-weight: 600; margin-bottom: 16px; }
            .dialog label { display: block; font-size: 12px; font-weight: 600; color: var(--text-secondary); margin-bottom: 8px; margin-top: 12px; }
            .dialog label:first-of-type { margin-top: 0; }
            .role-options { display: flex; gap: 8px; }
            .role-btn { flex: 1; padding: 12px; text-align: left; border-radius: 8px; display: flex; flex-direction: column; gap: 4px; }
            .role-btn.active { border-color: var(--accent); background: rgba(88,166,255,0.1); }
            .role-desc { font-size: 11px; color: var(--text-secondary); }
            .chip-row { display: flex; gap: 6px; flex-wrap: wrap; }
            .chip {
              padding: 6px 12px;
              font-size: 12px;
              border-radius: 999px;
              background: var(--bg-tertiary);
              color: var(--text-secondary);
              cursor: pointer;
            }
            .chip.active {
              background: rgba(88,166,255,0.15);
              border-color: var(--accent);
              color: var(--text-primary);
              font-weight: 600;
            }
            .invite-url-row { display: flex; gap: 8px; }
            .invite-url-row input { flex: 1; }
            .hint { font-size: 12px; color: var(--text-secondary); margin-top: 8px; }
            .invite-manage {
              margin-bottom: 16px;
              padding-bottom: 16px;
              border-bottom: 1px solid var(--border);
            }
            .invite-manage-label {
              margin-top: 0 !important;
              margin-bottom: 8px !important;
            }
            .invite-list {
              list-style: none;
              padding: 0;
              margin: 0;
              max-height: 150px;
              overflow-y: auto;
              display: flex;
              flex-direction: column;
              gap: 6px;
            }
            .invite-row {
              display: flex;
              align-items: center;
              justify-content: space-between;
              gap: 8px;
              padding: 8px 10px;
              background: var(--bg-tertiary);
              border-radius: 6px;
              font-size: 12px;
            }
            .invite-row-main {
              display: flex;
              align-items: center;
              gap: 10px;
              flex: 1;
              min-width: 0;
            }
            .invite-row-prefix {
              font-family: var(--font-mono, monospace);
              color: var(--text-secondary);
              font-size: 11px;
            }
            .invite-row-remaining {
              color: var(--text-secondary);
              font-size: 11px;
              margin-left: auto;
              white-space: nowrap;
              overflow: hidden;
              text-overflow: ellipsis;
            }
            .role-badge {
              padding: 2px 8px;
              border-radius: 999px;
              font-size: 10px;
              font-weight: 600;
              text-transform: uppercase;
              letter-spacing: 0.03em;
            }
            .role-badge-operator { background: rgba(88,166,255,0.15); color: var(--accent); }
            .role-badge-viewer { background: rgba(148,148,148,0.15); color: var(--text-secondary); }
            .invite-revoke-btn {
              padding: 4px 10px;
              font-size: 11px;
              background: transparent;
              color: var(--text-secondary);
              border: 1px solid var(--border);
              border-radius: 4px;
              cursor: pointer;
            }
            .invite-revoke-btn:hover {
              color: var(--danger, #ff6b6b);
              border-color: var(--danger, #ff6b6b);
            }
            .invite-revoke-confirm {
              display: flex;
              gap: 4px;
            }
            .invite-revoke-yes {
              padding: 4px 10px;
              font-size: 11px;
              background: var(--danger, #ff6b6b);
              color: white;
              border: none;
              border-radius: 4px;
              cursor: pointer;
              font-weight: 600;
            }
            .invite-revoke-no {
              padding: 4px 10px;
              font-size: 11px;
              background: transparent;
              color: var(--text-secondary);
              border: 1px solid var(--border);
              border-radius: 4px;
              cursor: pointer;
            }
            .manage-error {
              color: var(--danger, #ff6b6b);
              font-size: 11px;
              margin-bottom: 8px;
            }
          `}</style>
        </div>
      </div>
    </Show>
  );
}
