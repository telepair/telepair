// web/src/components/RecordingIndicator.tsx
import { Show } from 'solid-js';
import type { JSX } from 'solid-js';

export interface RecordingIndicatorProps {
  /** Whether a recording is currently active. */
  isRecording: boolean;
  /** Whether the current user is the session owner (shows Stop button). */
  isOwner: boolean;
  /** Called when the owner clicks Stop. */
  onStop: () => void;
}

export default function RecordingIndicator(props: RecordingIndicatorProps): JSX.Element {
  return (
    <Show when={props.isRecording}>
      <div class="rec-indicator" role="status" aria-label="Recording in progress">
        <span class="rec-dot" aria-hidden="true" />
        <span class="rec-label">REC</span>
        <Show when={props.isOwner}>
          <button type="button" class="rec-stop-btn" onClick={props.onStop} aria-label="Stop recording">
            Stop
          </button>
        </Show>
      </div>

      <style>{`
        .rec-indicator {
          display: inline-flex;
          align-items: center;
          gap: 6px;
          padding: 3px 8px;
          border-radius: 12px;
          background: rgba(248, 81, 73, 0.12);
          border: 1px solid rgba(248, 81, 73, 0.4);
        }

        .rec-dot {
          width: 8px;
          height: 8px;
          border-radius: 50%;
          background: #f85149;
          animation: rec-pulse 1.2s ease-in-out infinite;
          flex-shrink: 0;
        }

        @keyframes rec-pulse {
          0%, 100% { opacity: 1; }
          50%       { opacity: 0.3; }
        }

        .rec-label {
          font-size: 11px;
          font-weight: 700;
          letter-spacing: 0.08em;
          color: #f85149;
        }

        .rec-stop-btn {
          font-size: 11px;
          font-weight: 600;
          padding: 1px 7px;
          border-radius: 8px;
          border: 1px solid rgba(248, 81, 73, 0.5);
          background: transparent;
          color: #f85149;
          cursor: pointer;
          transition: background 0.15s, color 0.15s;
          line-height: 1.4;
        }

        .rec-stop-btn:hover {
          background: #f85149;
          color: #fff;
          border-color: #f85149;
        }
      `}</style>
    </Show>
  );
}
