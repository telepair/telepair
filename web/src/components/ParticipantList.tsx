import { For, Show } from 'solid-js';
import type { ParticipantInfo, Role } from '../lib/protocol';

interface ParticipantListProps {
  participants: ParticipantInfo[];
  myRole: Role;
  onPromote?: (userId: string, newRole: Role) => void;
  onKick?: (userId: string) => void;
}

export default function ParticipantList(props: ParticipantListProps) {
  return (
    <div class="participant-list">
      <h4>Participants ({props.participants.length})</h4>
      <div class="participants">
        <For each={props.participants}>
          {(p) => (
            <div class="participant-row">
              <span class="participant-color" style={{ background: p.color }} />
              <span class="participant-name">{p.name}</span>
              <span class="participant-role" data-role={p.role}>{p.role}</span>
              <Show when={props.myRole === 'owner' && p.role !== 'owner'}>
                <div class="participant-actions">
                  <Show when={p.role === 'viewer'}>
                    <button class="action-btn" title="Promote to operator" onClick={() => props.onPromote?.(p.user_id, 'operator')}>+</button>
                  </Show>
                  <Show when={p.role === 'operator'}>
                    <button class="action-btn" title="Demote to viewer" onClick={() => props.onPromote?.(p.user_id, 'viewer')}>-</button>
                  </Show>
                  <button class="action-btn kick" title="Remove from session" onClick={() => props.onKick?.(p.user_id)}>x</button>
                </div>
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
        .participant-role { font-size: 10px; padding: 1px 6px; border-radius: 8px; text-transform: uppercase; font-weight: 600; }
        .participant-role[data-role="owner"] { background: rgba(63,185,80,0.2); color: var(--success); }
        .participant-role[data-role="operator"] { background: rgba(88,166,255,0.2); color: var(--accent); }
        .participant-role[data-role="viewer"] { background: rgba(139,148,158,0.2); color: var(--text-secondary); }
        .participant-actions { display: flex; gap: 2px; }
        .action-btn { width: 20px; height: 20px; padding: 0; font-size: 12px; line-height: 1; border-radius: 4px; display: flex; align-items: center; justify-content: center; }
        .action-btn.kick { color: var(--error); }
      `}</style>
    </div>
  );
}
