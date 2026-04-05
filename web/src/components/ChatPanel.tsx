import { createSignal, createEffect, For } from 'solid-js';

export interface ChatMessage {
  user_id: string;
  name: string;
  text: string;
  ts: string;
}

interface ChatPanelProps {
  messages: ChatMessage[];
  onSend: (text: string) => void;
}

export default function ChatPanel(props: ChatPanelProps) {
  const [input, setInput] = createSignal('');
  let messagesEnd: HTMLDivElement | undefined;

  createEffect(() => {
    props.messages.length; // track reactive dependency
    messagesEnd?.scrollIntoView({ behavior: 'smooth' });
  });

  const handleSend = () => {
    const text = input().trim();
    if (!text) return;
    props.onSend(text);
    setInput('');
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const formatTime = (ts: string) => {
    try {
      return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    } catch {
      return '';
    }
  };

  return (
    <div class="chat-panel">
      <h4>Chat</h4>
      <div class="chat-messages">
        <For each={props.messages}>
          {(msg) => (
            <div class="chat-msg">
              <span class="chat-name">{msg.name}</span>
              <span class="chat-time">{formatTime(msg.ts)}</span>
              <p class="chat-text">{msg.text}</p>
            </div>
          )}
        </For>
        <div ref={messagesEnd} />
      </div>
      <div class="chat-input-row">
        <input
          type="text"
          placeholder="Type a message..."
          value={input()}
          onInput={(e) => setInput(e.currentTarget.value)}
          onKeyDown={handleKeyDown}
        />
        <button onClick={handleSend} disabled={!input().trim()}>Send</button>
      </div>
      <style>{`
        .chat-panel { display: flex; flex-direction: column; height: 100%; }
        .chat-panel h4 { font-size: 12px; font-weight: 600; color: var(--text-secondary); text-transform: uppercase; margin-bottom: 8px; padding: 0 4px; }
        .chat-messages { flex: 1; overflow-y: auto; display: flex; flex-direction: column; gap: 8px; padding: 4px; min-height: 0; }
        .chat-msg { font-size: 13px; }
        .chat-name { font-weight: 600; margin-right: 6px; }
        .chat-time { color: var(--text-secondary); font-size: 11px; }
        .chat-text { margin-top: 2px; word-break: break-word; }
        .chat-input-row { display: flex; gap: 6px; padding: 8px 4px 4px; border-top: 1px solid var(--border); }
        .chat-input-row input { flex: 1; font-family: var(--font-sans); font-size: 13px; }
        .chat-input-row button { font-size: 13px; padding: 6px 12px; }
      `}</style>
    </div>
  );
}
