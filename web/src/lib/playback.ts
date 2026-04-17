/**
 * PlaybackEngine — asciicast v2 parser and playback controller.
 *
 * Asciicast v2 format (NDJSON):
 *   Line 1: JSON header  { version, width, height, timestamp, … }
 *   Lines 2+: JSON event [time, type, data]
 *             where type is one of:
 *               "o" — terminal output (string)
 *               "r" — terminal resize ("<cols>x<rows>")
 *               "j" — participant join  (JSON string)
 *               "l" — participant leave (JSON string)
 *               "c" — chat message      (JSON string)
 *
 * Playback is timer-driven: each event is scheduled with a
 * `setTimeout` whose delay is `(event.time - currentTime) / speed * 1000`.
 * Seeking replays all output events from the beginning (a terminal is a
 * stateful display device — you cannot jump into the middle without
 * replaying prior state).
 */

export interface CastHeader {
  version: number;
  width: number;
  height: number;
  timestamp?: number;
  title?: string;
  env?: Record<string, string>;
  [key: string]: unknown;
}

export interface CastEvent {
  time: number;
  type: string;
  data: string;
}

export type PlaybackState = 'idle' | 'playing' | 'paused' | 'ended';

export interface ParticipantPayload {
  user_id: string;
  name: string;
  role?: string;
}

export interface ChatPayload {
  user_id: string;
  name: string;
  text: string;
}

export class PlaybackEngine {
  // ── Parsed content ──────────────────────────────────────────────────────────
  header!: CastHeader;
  events: CastEvent[] = [];

  // ── Playback state ──────────────────────────────────────────────────────────
  state: PlaybackState = 'idle';
  currentTime = 0;
  speed = 1;

  // ── Callbacks ───────────────────────────────────────────────────────────────
  onOutput?: (data: string) => void;
  onResize?: (cols: number, rows: number) => void;
  onParticipantJoin?: (payload: ParticipantPayload) => void;
  onParticipantLeave?: (payload: { user_id: string }) => void;
  onChat?: (payload: ChatPayload) => void;
  onComplete?: () => void;
  onTimeUpdate?: (time: number) => void;

  // ── Internal scheduling ─────────────────────────────────────────────────────
  /** Index of the next event to be scheduled. */
  private nextIndex = 0;
  /** Wall-clock time (ms) when the current play segment started. */
  private playStartWall = 0;
  /** Engine time (seconds) at which the current play segment started. */
  private playStartTime = 0;
  /** Currently pending timer handles. */
  private timers: ReturnType<typeof setTimeout>[] = [];
  /** Timer for the `onTimeUpdate` ticker (100 ms interval). */
  private tickTimer: ReturnType<typeof setInterval> | null = null;

  // ── Public API ──────────────────────────────────────────────────────────────

  /**
   * Parse an asciicast v2 string (NDJSON). Clears any previous content
   * and resets playback to the beginning.
   */
  load(castContent: string): void {
    this.dispose();
    const lines = castContent.split('\n').filter((l) => l.trim().length > 0);
    if (lines.length === 0) throw new Error('Empty cast content');

    this.header = JSON.parse(lines[0]) as CastHeader;
    this.events = lines.slice(1).map((line) => {
      const [time, type, data] = JSON.parse(line) as [number, string, string];
      return { time, type, data };
    });

    this.state = 'idle';
    this.currentTime = 0;
    this.nextIndex = 0;
  }

  /** Total duration in seconds (time of last event). */
  get duration(): number {
    if (this.events.length === 0) return 0;
    return this.events[this.events.length - 1].time;
  }

  /**
   * Start or resume playback from `currentTime`. If already playing,
   * this is a no-op.
   */
  play(): void {
    if (this.state === 'playing') return;
    if (this.state === 'ended') {
      // Restart from beginning.
      this.currentTime = 0;
      this.nextIndex = 0;
    }

    this.state = 'playing';
    this.playStartWall = Date.now();
    this.playStartTime = this.currentTime;

    this.scheduleRemaining();
    this.startTicker();
  }

  /** Pause playback. Current position is preserved for resuming. */
  pause(): void {
    if (this.state !== 'playing') return;
    // Capture how far we have advanced before clearing timers.
    this.currentTime = this.elapsedTime();
    this.state = 'paused';
    this.clearTimers();
    this.stopTicker();
  }

  /**
   * Seek to `timeSeconds`. Replays every event up to and including the
   * target so the terminal state machine, the recorded resize history,
   * AND the collab sidebar (participants + chat) are all consistent
   * with what the viewer would have seen by playing from time 0 to
   * the target. If currently playing, playback resumes from the new
   * position.
   *
   * Why every type, not just `o`/`r`: the sidebar is rebuilt from
   * `j`/`l`/`c` events, and the player clears its participant + chat
   * state before calling `seek()` (so a backward seek does not double
   * up). Without replaying `j`/`l`/`c` here, every seek would empty
   * the sidebar even though the underlying recording carries the
   * data needed to reconstruct it. Each event is dispatched with
   * `currentTime` set to its own recorded time so that downstream
   * handlers (e.g. chat-message timestamps) receive the original
   * value rather than the seek target.
   */
  seek(timeSeconds: number): void {
    const wasPlaying = this.state === 'playing';
    this.clearTimers();
    this.stopTicker();

    const target = Math.max(0, Math.min(timeSeconds, this.duration));
    this.currentTime = 0;
    this.nextIndex = 0;

    for (const event of this.events) {
      if (event.time > target) break;
      this.nextIndex++;
      this.currentTime = event.time;
      this.dispatch(event);
    }
    // Pin currentTime to the seek target so the UI shows the requested
    // position rather than the time of the last replayed event.
    this.currentTime = target;

    if (wasPlaying) {
      this.state = 'paused'; // will be overridden by play()
      this.play();
    }
  }

  /**
   * Set the playback speed multiplier (e.g. 0.5, 1, 2, 4). If
   * currently playing, reschedules pending events at the new rate.
   */
  setSpeed(multiplier: number): void {
    if (multiplier <= 0) throw new Error('Speed must be positive');
    if (this.state === 'playing') {
      // Snapshot current position before changing speed.
      this.currentTime = this.elapsedTime();
      this.clearTimers();
      this.stopTicker();
    }
    this.speed = multiplier;
    if (this.state === 'playing') {
      this.playStartWall = Date.now();
      this.playStartTime = this.currentTime;
      this.scheduleRemaining();
      this.startTicker();
    }
  }

  /** Release all timers. Safe to call multiple times. */
  dispose(): void {
    this.clearTimers();
    this.stopTicker();
    this.state = 'idle';
  }

  // ── Internal helpers ────────────────────────────────────────────────────────

  /** Engine time (seconds) at the current wall-clock moment. */
  private elapsedTime(): number {
    const wallMs = Date.now() - this.playStartWall;
    return this.playStartTime + (wallMs / 1000) * this.speed;
  }

  /**
   * Schedule all events from `nextIndex` onward relative to the play
   * segment start. Already-elapsed events (delay ≤ 0) are fired
   * immediately via a zero-delay timeout so the call stack stays clean.
   */
  private scheduleRemaining(): void {
    for (let i = this.nextIndex; i < this.events.length; i++) {
      const event = this.events[i];
      const delayMs = Math.max(
        0,
        ((event.time - this.playStartTime) / this.speed) * 1000
          - (Date.now() - this.playStartWall),
      );
      const handle = setTimeout(() => {
        if (this.state !== 'playing') return;
        this.currentTime = event.time;
        this.dispatch(event);

        // Check if this was the last event.
        if (i === this.events.length - 1) {
          this.state = 'ended';
          this.stopTicker();
          this.onComplete?.();
        }
      }, delayMs);
      this.timers.push(handle);
    }
    this.nextIndex = this.events.length;
  }

  private clearTimers(): void {
    for (const h of this.timers) clearTimeout(h);
    this.timers = [];
    this.nextIndex = this.findNextIndex();
  }

  /** Find the index of the first event whose time is > currentTime. */
  private findNextIndex(): number {
    for (let i = 0; i < this.events.length; i++) {
      if (this.events[i].time > this.currentTime) return i;
    }
    return this.events.length;
  }

  private startTicker(): void {
    if (this.tickTimer !== null) return;
    this.tickTimer = setInterval(() => {
      if (this.state === 'playing') {
        this.onTimeUpdate?.(this.elapsedTime());
      }
    }, 100);
  }

  private stopTicker(): void {
    if (this.tickTimer !== null) {
      clearInterval(this.tickTimer);
      this.tickTimer = null;
    }
  }

  /** Dispatch a single event to the appropriate callback. */
  private dispatch(event: CastEvent): void {
    switch (event.type) {
      case 'o':
        this.onOutput?.(event.data);
        break;
      case 'r':
        this.applyResize(event.data);
        break;
      case 'j':
        try {
          const payload = JSON.parse(event.data) as ParticipantPayload;
          this.onParticipantJoin?.(payload);
        } catch {
          // Malformed payload — skip silently.
        }
        break;
      case 'l':
        try {
          const payload = JSON.parse(event.data) as { user_id: string };
          this.onParticipantLeave?.(payload);
        } catch {
          // Malformed payload — skip silently.
        }
        break;
      case 'c':
        try {
          const payload = JSON.parse(event.data) as ChatPayload;
          this.onChat?.(payload);
        } catch {
          // Malformed payload — skip silently.
        }
        break;
      default:
        // Unknown event type — ignore for forward-compatibility.
        break;
    }
  }

  /** Parse a resize event data string "<cols>x<rows>" and fire the callback. */
  private applyResize(data: string): void {
    const [colsStr, rowsStr] = data.split('x');
    const cols = parseInt(colsStr, 10);
    const rows = parseInt(rowsStr, 10);
    if (!isNaN(cols) && !isNaN(rows)) {
      this.onResize?.(cols, rows);
    }
  }
}
