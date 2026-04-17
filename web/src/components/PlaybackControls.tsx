// web/src/components/PlaybackControls.tsx
import { Show, For } from 'solid-js';
import type { JSX } from 'solid-js';

export interface PlaybackControlsProps {
  /** Current playback position in seconds. */
  currentTime: number;
  /** Total duration in seconds. */
  duration: number;
  /** Whether playback is active. */
  isPlaying: boolean;
  /** Current speed multiplier. */
  speed: number;
  /** Whether the controls are disabled (e.g. still loading). */
  disabled?: boolean;

  onPlayPause: () => void;
  /** Called with a seek position in seconds. */
  onSeek: (seconds: number) => void;
  onSpeedChange: (speed: number) => void;
  onFullscreen?: () => void;
}

const SPEED_OPTIONS = [0.5, 1, 2, 4];

/** Format seconds as `mm:ss` (or `h:mm:ss` for long recordings). */
function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) seconds = 0;
  const s = Math.floor(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const mm = String(m).padStart(2, '0');
  const ss = String(sec).padStart(2, '0');
  if (h > 0) return `${h}:${mm}:${ss}`;
  return `${mm}:${ss}`;
}

export default function PlaybackControls(props: PlaybackControlsProps): JSX.Element {
  const progress = () => {
    const d = props.duration;
    if (!d || d <= 0) return 0;
    return Math.min(1, props.currentTime / d) * 100;
  };

  function handleProgressClick(e: MouseEvent) {
    if (props.disabled) return;
    const bar = e.currentTarget as HTMLDivElement;
    const rect = bar.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    props.onSeek(ratio * props.duration);
  }

  return (
    <div class="pb-controls">
      {/* Progress bar */}
      <div
        class="pb-progress-track"
        role="slider"
        aria-label="Seek"
        aria-valuemin={0}
        aria-valuemax={props.duration}
        aria-valuenow={props.currentTime}
        tabIndex={props.disabled ? -1 : 0}
        onClick={handleProgressClick}
        onKeyDown={(e) => {
          if (props.disabled) return;
          const step = 5;
          if (e.key === 'ArrowRight') props.onSeek(Math.min(props.currentTime + step, props.duration));
          if (e.key === 'ArrowLeft') props.onSeek(Math.max(props.currentTime - step, 0));
        }}
      >
        <div class="pb-progress-fill" style={{ width: `${progress()}%` }} />
        <div class="pb-progress-thumb" style={{ left: `${progress()}%` }} />
      </div>

      {/* Bottom control row */}
      <div class="pb-row">
        {/* Play/Pause */}
        <button
          type="button"
          class="pb-play-btn"
          aria-label={props.isPlaying ? 'Pause' : 'Play'}
          disabled={props.disabled}
          onClick={props.onPlayPause}
        >
          {props.isPlaying ? '⏸' : '▶'}
        </button>

        {/* Time display */}
        <span class="pb-time">
          {formatTime(props.currentTime)}
          <span class="pb-time-sep"> / </span>
          {formatTime(props.duration)}
        </span>

        {/* Spacer */}
        <div class="pb-spacer" />

        {/* Speed selector */}
        <div class="pb-speeds" role="group" aria-label="Playback speed">
          <For each={SPEED_OPTIONS}>
            {(s) => (
              <button
                type="button"
                class="pb-speed-btn"
                aria-pressed={props.speed === s}
                disabled={props.disabled}
                onClick={() => props.onSpeedChange(s)}
              >
                {s}x
              </button>
            )}
          </For>
        </div>

        {/* Fullscreen */}
        <Show when={props.onFullscreen}>
          <button
            type="button"
            class="pb-fullscreen-btn"
            aria-label="Fullscreen"
            disabled={props.disabled}
            onClick={props.onFullscreen}
          >
            ⛶
          </button>
        </Show>
      </div>

      <style>{`
        .pb-controls {
          background: #111;
          border-top: 1px solid #2a2a2a;
          padding: 8px 12px 10px;
          display: flex;
          flex-direction: column;
          gap: 8px;
          user-select: none;
        }

        /* Progress track */
        .pb-progress-track {
          position: relative;
          height: 4px;
          background: #333;
          border-radius: 2px;
          cursor: pointer;
          outline: none;
          margin: 4px 0;
        }
        .pb-progress-track:focus-visible {
          outline: 2px solid var(--accent, #58a6ff);
          outline-offset: 2px;
        }
        .pb-progress-track:hover .pb-progress-thumb {
          opacity: 1;
          transform: translate(-50%, -50%) scale(1);
        }
        .pb-progress-fill {
          position: absolute;
          left: 0; top: 0; bottom: 0;
          background: var(--accent, #58a6ff);
          border-radius: 2px;
          pointer-events: none;
          transition: width 0.1s linear;
        }
        .pb-progress-thumb {
          position: absolute;
          top: 50%;
          width: 12px;
          height: 12px;
          background: #fff;
          border-radius: 50%;
          transform: translate(-50%, -50%) scale(0.7);
          opacity: 0;
          pointer-events: none;
          transition: opacity 0.15s, transform 0.15s;
        }

        /* Bottom row */
        .pb-row {
          display: flex;
          align-items: center;
          gap: 10px;
        }
        .pb-spacer { flex: 1; }

        .pb-play-btn {
          background: transparent;
          border: none;
          color: #fff;
          font-size: 18px;
          line-height: 1;
          padding: 2px 6px;
          cursor: pointer;
          border-radius: 4px;
          transition: background 0.15s;
        }
        .pb-play-btn:hover:not(:disabled) { background: rgba(255,255,255,0.1); }
        .pb-play-btn:disabled { opacity: 0.4; cursor: default; }

        .pb-time {
          font-family: var(--font-mono, monospace);
          font-size: 13px;
          color: #ccc;
          white-space: nowrap;
        }
        .pb-time-sep { color: #555; }

        .pb-speeds {
          display: flex;
          gap: 2px;
        }
        .pb-speed-btn {
          background: transparent;
          border: 1px solid #444;
          color: #aaa;
          font-size: 12px;
          padding: 3px 8px;
          border-radius: 4px;
          cursor: pointer;
          transition: all 0.15s;
        }
        .pb-speed-btn:hover:not(:disabled) { border-color: var(--accent, #58a6ff); color: #fff; }
        .pb-speed-btn:disabled { opacity: 0.4; cursor: default; }
        .pb-speed-btn[aria-pressed='true'] {
          background: var(--accent, #58a6ff);
          border-color: var(--accent, #58a6ff);
          color: #000;
          font-weight: 600;
        }

        .pb-fullscreen-btn {
          background: transparent;
          border: none;
          color: #aaa;
          font-size: 16px;
          padding: 2px 6px;
          cursor: pointer;
          border-radius: 4px;
          transition: background 0.15s, color 0.15s;
        }
        .pb-fullscreen-btn:hover:not(:disabled) { background: rgba(255,255,255,0.1); color: #fff; }
        .pb-fullscreen-btn:disabled { opacity: 0.4; cursor: default; }
      `}</style>
    </div>
  );
}
