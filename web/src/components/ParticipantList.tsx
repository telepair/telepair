import { For, Show } from 'solid-js';
import type { ParticipantInfo, Role } from '../lib/protocol';
import { roleLabel, useI18n } from '../i18n';

interface ParticipantListProps {
  participants: ParticipantInfo[];
  /** Whether the current user is the session owner. */
  isOwner?: boolean;
  /** Callback when the owner clicks a role-toggle button. */
  onRoleChange?: (userId: string, newRole: Role) => void;
}

export default function ParticipantList(props: ParticipantListProps) {
  const { t } = useI18n();

  const toggleRole = (p: ParticipantInfo) => {
    if (!props.onRoleChange) return;
    const newRole: Role = p.role === 'viewer' ? 'operator' : 'viewer';
    props.onRoleChange(p.user_id, newRole);
  };

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
                <button
                  type="button"
                  class="role-toggle"
                  data-role={p.role}
                  title={t('participants.toggle_role_aria', { name: p.name })}
                  onClick={() => toggleRole(p)}
                >
                  {roleLabel(t, p.role)}
                </button>
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
        .role-toggle {
          font-size: 10px;
          padding: 2px 8px;
          border-radius: 999px;
          cursor: pointer;
          background: var(--bg-tertiary);
          border: 1px solid var(--border);
          color: var(--text-secondary);
          transition: all 0.15s;
        }
        .role-toggle:hover {
          border-color: var(--accent);
          color: var(--text-primary);
        }
      `}</style>
    </div>
  );
}
