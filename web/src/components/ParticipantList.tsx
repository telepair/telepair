import { For, Show } from 'solid-js';
import type { ParticipantInfo, Role } from '../lib/protocol';
import { roleLabel, useI18n } from '../i18n';

interface ParticipantListProps {
  participants: ParticipantInfo[];
  /** Whether the current user is the session owner. */
  isOwner?: boolean;
  /** Callback when the owner picks a new role for a participant. */
  onRoleChange?: (userId: string, newRole: Role) => void;
}

// Roles the owner can assign. Owner is intentionally absent — the
// backend rejects promotion to Owner (a session has exactly one), and
// there is no backend endpoint for kick/remove, so "Operator" and
// "Viewer" are the only two values the server will accept.
const ASSIGNABLE_ROLES: ReadonlyArray<Exclude<Role, 'owner'>> = [
  'operator',
  'viewer',
];

export default function ParticipantList(props: ParticipantListProps) {
  const { t } = useI18n();

  return (
    <div class="participant-list">
      <h4>{t('participants.heading', { count: String(props.participants.length) })}</h4>
      <div class="participants">
        <For each={props.participants}>
          {(p) => (
            <div class="participant-row">
              <span class="participant-color" style={{ background: p.color }} />
              <span class="participant-name">{p.name}</span>
              <Show
                when={props.isOwner && p.role !== 'owner'}
                fallback={
                  <span class="participant-role" data-role={p.role}>{roleLabel(t, p.role)}</span>
                }
              >
                <select
                  class="role-select"
                  data-role={p.role}
                  value={p.role}
                  aria-label={t('participants.change_role_aria', { name: p.name })}
                  onChange={(e) => {
                    const next = e.currentTarget.value as Role;
                    if (next === p.role) return;
                    props.onRoleChange?.(p.user_id, next);
                  }}
                >
                  <For each={ASSIGNABLE_ROLES}>
                    {(r) => <option value={r}>{roleLabel(t, r)}</option>}
                  </For>
                </select>
              </Show>
            </div>
          )}
        </For>
      </div>
      <style>{`
        .participant-list h4 { font-size: 12px; font-weight: 600; color: var(--text-secondary); text-transform: uppercase; margin-bottom: 8px; }
        .participants { display: flex; flex-direction: column; gap: 4px; }
        .participant-row { display: flex; align-items: center; gap: 8px; padding: 6px 8px; border-radius: 6px; font-size: 13px; }
        .participant-row:hover { background: var(--bg-tertiary); }
        .participant-color { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
        .participant-name { flex: 1; }
        .participant-role { font-size: 10px; }
        .role-select {
          font-size: 10px;
          padding: 2px 20px 2px 8px;
          border-radius: 999px;
          cursor: pointer;
          background: var(--bg-tertiary);
          border: 1px solid var(--border);
          color: var(--text-secondary);
          transition: all 0.15s;
          appearance: none;
          -webkit-appearance: none;
          background-image: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10' viewBox='0 0 10 10'><path fill='%238b949e' d='M1 3l4 4 4-4z'/></svg>");
          background-repeat: no-repeat;
          background-position: right 6px center;
        }
        .role-select:hover {
          border-color: var(--accent);
          color: var(--text-primary);
        }
        .role-select:focus-visible {
          outline: 2px solid var(--accent);
          outline-offset: 1px;
        }
      `}</style>
    </div>
  );
}
