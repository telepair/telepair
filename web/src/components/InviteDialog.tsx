import { createSignal, Show } from 'solid-js';
import { api } from '../lib/api';
import type { Role, InputMode } from '../lib/protocol';
import { toast } from '../stores/toast';

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
 *  session lifetime). */
const TTL_PRESETS: Array<{ label: string; minutes: number | null }> = [
  { label: '15 min', minutes: 15 },
  { label: '1 hour', minutes: 60 },
  { label: '24 hours', minutes: 24 * 60 },
  { label: '7 days', minutes: 7 * 24 * 60 },
  { label: 'No expiry', minutes: null },
];

/** `max_uses` presets. The server caps at 100 so higher values round-
 *  trip as 400; surface the safe range directly instead of a free-form
 *  number field that invites typos. */
const MAX_USES_PRESETS: number[] = [1, 3, 5, 10, 25];

function formatExpiry(iso: string | null): string {
  if (!iso) return 'Never (until session closes)';
  const when = new Date(iso);
  if (Number.isNaN(when.getTime())) return 'unknown';
  const diffMs = when.getTime() - Date.now();
  if (diffMs <= 0) return 'expired';
  const minutes = Math.round(diffMs / 60_000);
  if (minutes < 60) return `in ~${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `in ~${hours} hr`;
  const days = Math.round(hours / 24);
  return `in ~${days} day${days === 1 ? '' : 's'}`;
}

export default function InviteDialog(props: InviteDialogProps) {
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
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      toast.error(`Failed to create invite: ${msg}`);
    } finally {
      setCreating(false);
    }
  };

  const handleCopy = async () => {
    await navigator.clipboard.writeText(inviteUrl());
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
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
          <h3>Invite to Session</h3>
          <Show when={!inviteUrl()} fallback={
            <div class="invite-result">
              <label>Invite Link</label>
              <div class="invite-url-row">
                <input type="text" value={inviteUrl()} readonly />
                <button class="primary" onClick={handleCopy}>
                  {copied() ? 'Copied!' : 'Copy'}
                </button>
              </div>
              <p class="hint">
                Usable <strong>{inviteMaxUses()}</strong> time{inviteMaxUses() === 1 ? '' : 's'} · expires <strong>{formatExpiry(inviteExpiresAt())}</strong>.
              </p>
              <p class="hint">Share this link with the person you want to invite.</p>
              <button onClick={handleClose} style={{ 'margin-top': '12px', width: '100%' }}>Done</button>
            </div>
          }>
            <div class="invite-form">
              <label>Role</label>
              <div class="role-options">
                <button
                  class={role() === 'operator' ? 'role-btn active' : 'role-btn'}
                  onClick={() => setRole('operator')}
                >
                  Operator
                  <span class="role-desc">
                    {props.inputMode === 'multiplexed'
                      ? 'Can type, resize, and chat'
                      : 'Can resize and chat (solo mode — only the owner types)'}
                  </span>
                </button>
                <button
                  class={role() === 'viewer' ? 'role-btn active' : 'role-btn'}
                  onClick={() => setRole('viewer')}
                >
                  Viewer
                  <span class="role-desc">Can only watch and chat</span>
                </button>
              </div>

              <label>Max uses</label>
              <div class="chip-row" role="radiogroup" aria-label="Maximum number of redemptions">
                {MAX_USES_PRESETS.map((n) => (
                  <button
                    type="button"
                    role="radio"
                    aria-checked={maxUses() === n}
                    class={maxUses() === n ? 'chip active' : 'chip'}
                    onClick={() => setMaxUses(n)}
                  >
                    {n === 1 ? 'One-shot' : `${n}`}
                  </button>
                ))}
              </div>

              <label>Expires</label>
              <div class="chip-row" role="radiogroup" aria-label="Expiry time">
                {TTL_PRESETS.map((preset) => (
                  <button
                    type="button"
                    role="radio"
                    aria-checked={ttlMinutes() === preset.minutes}
                    class={ttlMinutes() === preset.minutes ? 'chip active' : 'chip'}
                    onClick={() => setTtlMinutes(preset.minutes)}
                  >
                    {preset.label}
                  </button>
                ))}
              </div>

              <button class="primary" onClick={handleCreate} disabled={creating()} style={{ width: '100%', 'margin-top': '16px' }}>
                {creating() ? 'Creating...' : 'Create Invite Link'}
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
          `}</style>
        </div>
      </div>
    </Show>
  );
}
