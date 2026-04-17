// web/src/components/EventTimeline.tsx
import { For, Show, createMemo } from 'solid-js';
import type { JSX } from 'solid-js';
import type { CastEvent } from '../lib/playback';

export interface EventTimelineProps {
  /** All parsed events from the cast. Only non-output events are rendered. */
  events: CastEvent[];
  /** Total duration of the recording in seconds. */
  duration: number;
}

/** Color for each non-output event type. */
function markerColor(type: string): string {
  switch (type) {
    case 'r': return '#d2a8ff'; // resize — yellow-ish purple
    case 'j': return '#3fb950'; // join — green
    case 'l': return '#f78166'; // leave — red
    case 'c': return '#58a6ff'; // chat — blue
    default:  return '#8b949e'; // unknown — muted
  }
}

/** Tooltip label for each event type. */
function markerLabel(type: string): string {
  switch (type) {
    case 'r': return 'Resize';
    case 'j': return 'Participant joined';
    case 'l': return 'Participant left';
    case 'c': return 'Chat message';
    default:  return `Event (${type})`;
  }
}

export default function EventTimeline(props: EventTimelineProps): JSX.Element {
  // Only render non-output events. Skip type 'o' (terminal output).
  const markers = createMemo(() => {
    const d = props.duration;
    if (d <= 0) return [];
    return props.events
      .filter((e) => e.type !== 'o')
      .map((e) => ({
        ...e,
        pct: Math.min(100, Math.max(0, (e.time / d) * 100)),
        color: markerColor(e.type),
        label: markerLabel(e.type),
      }));
  });

  return (
    <Show when={markers().length > 0}>
      <div class="evt-timeline" aria-hidden="true">
        <For each={markers()}>
          {(m) => (
            <div
              class="evt-marker"
              title={`${m.label} at ${m.time.toFixed(1)}s`}
              style={{
                left: `${m.pct}%`,
                background: m.color,
              }}
            />
          )}
        </For>

        <style>{`
          .evt-timeline {
            position: absolute;
            inset: 0;
            pointer-events: none;
          }

          .evt-marker {
            position: absolute;
            top: 0;
            bottom: 0;
            width: 2px;
            transform: translateX(-50%);
            opacity: 0.75;
            border-radius: 1px;
          }
        `}</style>
      </div>
    </Show>
  );
}
