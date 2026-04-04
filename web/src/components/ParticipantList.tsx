import { For } from 'solid-js';
import type { ParticipantInfo } from '../lib/protocol';

interface ParticipantListProps {
  participants: ParticipantInfo[];
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
      `}</style>
    </div>
  );
}
