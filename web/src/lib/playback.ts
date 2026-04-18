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
 * Playback is timer-driven by a single pumping `setTimeout` chain:
 * at any moment at most one timer is pending, and when it fires it
 * dispatches the current event and re-schedules itself for the next
 * one. This keeps memory and pause/seek costs O(1) regardless of
 * cast length — a 1-hour recording with ~60k events used to pre-
 * register 60k `setTimeout` handles inside `play()` and walk the
 * full array on every `pause()` / `seek()` / `setSpeed()`, which
 * both pinned a lot of timer-queue memory and made future
 * scrubbing features impractical. The per-event delay formula
 * `(event.time - elapsedTime()) / speed * 1000` is unchanged.
 *
 * Seeking still replays all events from the beginning (a terminal
 * is a stateful display device — you cannot jump into the middle
 * without replaying prior state) and is fully synchronous.
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
  /**
   * Single pending timer driving the event pump. `null` whenever the
   * engine is not actively waiting for the next event (paused,
   * seeking, ended, disposed). Keeping exactly one live handle —
   * instead of the pre-registered array the old implementation used
   * — keeps both the timer queue and every cancel path O(1) even
   * for hour-long casts.
   */
  private pumpTimer: ReturnType<typeof setTimeout> | null = null;
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

    this.scheduleNext();
    this.startTicker();
  }

  /** Pause playback. Current position is preserved for resuming. */
  pause(): void {
    if (this.state !== 'playing') return;
    // Capture how far we have advanced before clearing the pump.
    this.currentTime = this.elapsedTime();
    this.state = 'paused';
    this.clearPump();
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
    this.clearPump();
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
      this.clearPump();
      this.stopTicker();
    }
    this.speed = multiplier;
    if (this.state === 'playing') {
      this.playStartWall = Date.now();
      this.playStartTime = this.currentTime;
      this.scheduleNext();
      this.startTicker();
    }
  }

  /** Release all timers. Safe to call multiple times. */
  dispose(): void {
    this.clearPump();
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
   * Schedule the single next event in the pump. Called on every
   * transition that resumes playback (`play`, `setSpeed` while
   * playing) and re-called from the timer callback itself after
   * each dispatch, so only one `setTimeout` is pending at any time.
   *
   * Delay is derived from the engine-time distance to the next
   * event, translated through the current speed multiplier. An
   * already-elapsed event (non-positive delta, e.g. clock drift or
   * a long synchronous task eating into the schedule) is still
   * queued with `delayMs = 0` rather than dispatched inline so we
   * never starve the event loop with a tight dispatch burst.
   */
  private scheduleNext(): void {
    if (this.nextIndex >= this.events.length) {
      // No more events — mark ended immediately so play() callers
      // that load an already-drained engine still observe onComplete.
      this.state = 'ended';
      this.stopTicker();
      this.onComplete?.();
      return;
    }
    const event = this.events[this.nextIndex];
    const delayMs = Math.max(0, ((event.time - this.elapsedTime()) / this.speed) * 1000);
    this.pumpTimer = setTimeout(() => {
      this.pumpTimer = null;
      // State may have flipped between scheduling and firing (e.g.
      // pause() or dispose()). Bail out cleanly.
      if (this.state !== 'playing') return;
      this.currentTime = event.time;
      this.nextIndex++;
      this.dispatch(event);

      if (this.nextIndex >= this.events.length) {
        this.state = 'ended';
        this.stopTicker();
        this.onComplete?.();
        return;
      }
      // Recurse: queue the next event. Cheap because the queue
      // depth is exactly 1 — no O(N) registration storm here.
      this.scheduleNext();
    }, delayMs);
  }

  private clearPump(): void {
    if (this.pumpTimer !== null) {
      clearTimeout(this.pumpTimer);
      this.pumpTimer = null;
    }
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
