// web/src/components/CollabSidebar.tsx
import { For, Show, createMemo } from 'solid-js';
import type { JSX } from 'solid-js';
import type { ParticipantPayload, ChatPayload } from '../lib/playback';

export interface CollabChatMessage extends ChatPayload {
  time: number;
}

export interface CollabSidebarProps {
  participants: ParticipantPayload[];
  chatMessages: CollabChatMessage[];
  /** Current playback position in seconds. Chat messages at time > currentTime are hidden. */
  currentTime: number;
}

/** Deterministic color from a user_id string. */
function nameColor(userId: string): string {
  const colors = [
    '#58a6ff', '#3fb950', '#d2a8ff', '#ffa657',
    '#f78166', '#79c0ff', '#a5d6ff', '#7ee787',
  ];
  let hash = 0;
  for (let i = 0; i < userId.length; i++) {
    hash = (hash * 31 + userId.charCodeAt(i)) >>> 0;
  }
  return colors[hash % colors.length];
}

export default function CollabSidebar(props: CollabSidebarProps): JSX.Element {
  const visibleMessages = createMemo(() =>
    props.chatMessages.filter((m) => m.time <= props.currentTime),
  );

  return (
    <div class="collab-sidebar">
      {/* Participants section */}
      <div class="collab-section">
        <h3 class="collab-section-title">Participants</h3>
        <Show
          when={props.participants.length > 0}
          fallback={<p class="collab-empty">No participants yet</p>}
        >
          <ul class="collab-participant-list">
            <For each={props.participants}>
              {(p) => (
                <li class="collab-participant">
                  <span class="collab-presence-dot" />
                  <span class="collab-participant-name" style={{ color: nameColor(p.user_id) }}>
                    {p.name}
                  </span>
                  <Show when={p.role}>
                    <span class="collab-participant-role">{p.role}</span>
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </div>

      {/* Chat section */}
      <div class="collab-section collab-chat-section">
        <h3 class="collab-section-title">Chat</h3>
        <Show
          when={visibleMessages().length > 0}
          fallback={<p class="collab-empty">No messages yet</p>}
        >
          <div class="collab-chat-list">
            <For each={visibleMessages()}>
              {(m) => (
                <div class="collab-chat-entry">
                  <span class="collab-chat-name" style={{ color: nameColor(m.user_id) }}>
                    {m.name}
                  </span>
                  <span class="collab-chat-text">{m.text}</span>
                </div>
              )}
            </For>
          </div>
        </Show>
      </div>

      <style>{`
        .collab-sidebar {
          width: 220px;
          min-width: 220px;
          border-left: 1px solid var(--border, #30363d);
          background: var(--bg-secondary, #161b22);
          display: flex;
          flex-direction: column;
          overflow: hidden;
          font-size: 13px;
        }

        .collab-section {
          padding: 12px;
          border-bottom: 1px solid var(--border, #30363d);
        }

        .collab-chat-section {
          flex: 1;
          border-bottom: none;
          min-height: 0;
          display: flex;
          flex-direction: column;
        }

        .collab-section-title {
          font-size: 11px;
          font-weight: 600;
          text-transform: uppercase;
          letter-spacing: 0.06em;
          color: var(--text-secondary, #8b949e);
          margin: 0 0 8px;
        }

        .collab-empty {
          color: var(--text-secondary, #8b949e);
          font-size: 12px;
          margin: 0;
          font-style: italic;
        }

        .collab-participant-list {
          list-style: none;
          margin: 0;
          padding: 0;
          display: flex;
          flex-direction: column;
          gap: 6px;
        }

        .collab-participant {
          display: flex;
          align-items: center;
          gap: 6px;
        }

        .collab-presence-dot {
          width: 7px;
          height: 7px;
          border-radius: 50%;
          background: #3fb950;
          flex-shrink: 0;
        }

        .collab-participant-name {
          font-weight: 500;
          flex: 1;
          overflow: hidden;
          text-overflow: ellipsis;
          white-space: nowrap;
        }

        .collab-participant-role {
          font-size: 10px;
          color: var(--text-secondary, #8b949e);
          text-transform: capitalize;
          flex-shrink: 0;
        }

        .collab-chat-list {
          flex: 1;
          overflow-y: auto;
          display: flex;
          flex-direction: column;
          gap: 8px;
          padding-right: 2px;
        }

        .collab-chat-entry {
          display: flex;
          flex-direction: column;
          gap: 2px;
          word-break: break-word;
        }

        .collab-chat-name {
          font-weight: 600;
          font-size: 12px;
        }

        .collab-chat-text {
          color: var(--text-primary, #c9d1d9);
          font-size: 12px;
          line-height: 1.4;
        }
      `}</style>
    </div>
  );
}
