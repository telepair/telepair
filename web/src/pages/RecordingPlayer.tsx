// web/src/pages/RecordingPlayer.tsx
import { createSignal, onMount, onCleanup, Show } from 'solid-js';
import { useParams } from '@solidjs/router';
import { Terminal as XTerm } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import { api, errorMessage } from '../lib/api';
import { formatBytes, formatDate } from '../lib/format';
import type { Recording } from '../lib/protocol';
import { PlaybackEngine } from '../lib/playback';
import type { ParticipantPayload, ChatPayload } from '../lib/playback';
import PlaybackControls from '../components/PlaybackControls';
import Banner from '../components/Banner';
import CollabSidebar from '../components/CollabSidebar';
import type { CollabChatMessage } from '../components/CollabSidebar';
import EventTimeline from '../components/EventTimeline';

/**
 * Read the share token out of `location.hash` (legacy paths used a
 * query string; see `ShareRecordingDialog` for why fragments are
 * preferable) and strip the fragment via `history.replaceState`.
 *
 * Returning `undefined` for both the missing-hash and malformed-hash
 * cases lets the caller treat "no token" and "bad token" identically
 * — the server will 401 either way, and the UI surfaces the same
 * "Recording not available" banner in both branches.
 *
 * Runs synchronously at render time so no byte of the token leaks to
 * `document.referrer` on any subsequent navigation — if we deferred
 * to `onMount` a user could right-click → Copy Link first.
 */
function captureShareTokenFromHash(): string | undefined {
  if (typeof window === 'undefined') return undefined;
  const hash = window.location.hash;
  if (!hash || hash.length < 2) return undefined;
  // Strip the leading `#` and parse as a flat querystring so future
  // additions (e.g. `#t=12s&token=…`) don't break the extraction.
  const params = new URLSearchParams(hash.slice(1));
  const token = params.get('token') ?? undefined;
  if (!token) return undefined;
  // Scrub the fragment so the secret doesn't linger in the URL bar,
  // `document.referrer`, or the shared-with-bookmark state. Using
  // `replaceState` (not `pushState`) keeps the SPA history stack
  // clean — a share viewer pressing Back should land wherever they
  // came from, not on the same page without a token.
  try {
    const cleanUrl = `${window.location.pathname}${window.location.search}`;
    window.history.replaceState(window.history.state, '', cleanUrl);
  } catch {
    // Sandboxed iframes, file:// URLs, or browsers with history
    // throttling can reject replaceState. The token is still held
    // only in our closure and never sent to the server in a query
    // string, so a failure to scrub is degraded but not catastrophic.
  }
  return token;
}

export default function RecordingPlayer() {
  const params = useParams<{ id: string }>();

  // Extract the share token from the URL fragment (#token=…) and
  // immediately scrub it from the browser history. Fragments don't
  // traverse HTTP — so reverse-proxy and gateway access logs can
  // never see them — but they *do* linger in `document.referrer`,
  // browser history, and the address bar until we clear them.
  // Capturing once at mount (before any `await`) + `replaceState`
  // leaves the player with the token in a closure but nothing
  // user-observable after paint.
  const capturedToken = captureShareTokenFromHash();
  const shareToken = () => capturedToken;

  // ── State ──────────────────────────────────────────────────────────────────
  const [recording, setRecording] = createSignal<Recording | null>(null);
  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal('');
  const [isPlaying, setIsPlaying] = createSignal(false);
  const [currentTime, setCurrentTime] = createSignal(0);
  const [duration, setDuration] = createSignal(0);
  const [speed, setSpeed] = createSignal(1);

  // ── Collab state ───────────────────────────────────────────────────────────
  const [participants, setParticipants] = createSignal<ParticipantPayload[]>([]);
  const [chatMessages, setChatMessages] = createSignal<CollabChatMessage[]>([]);

  // ── Refs ───────────────────────────────────────────────────────────────────
  let terminalContainer: HTMLDivElement | undefined;
  let playerContainer: HTMLDivElement | undefined;
  let term: XTerm | undefined;
  let engine: PlaybackEngine | undefined;
  // Abort latch: initPlayer has multiple `await` points (metadata
  // fetch, data download, microtask yield). If the component
  // unmounts mid-flight we must not touch DOM / xterm / signals
  // afterward, and we must dispose any engine/term that was built
  // before the abort.
  let destroyed = false;

  onMount(async () => {
    await initPlayer();
  });

  onCleanup(() => {
    destroyed = true;
    engine?.dispose();
    term?.dispose();
  });

  async function initPlayer() {
    setLoading(true);
    setError('');

    try {
      // Fetch recording metadata — skip for anonymous share tokens since
      // the data endpoint itself is what we need; metadata 403s for anon.
      let rec: Recording | null = null;
      if (!shareToken()) {
        rec = await api.getRecording(params.id);
        if (destroyed) return;
        setRecording(rec);
      }

      // Fetch the asciicast data file. `api.fetchRecordingData` picks
      // the right auth path internally: `X-Share-Token` header for
      // anonymous share viewers, bearer for owners/admins. Neither
      // path puts the share secret in the URL, so reverse-proxy
      // access logs don't capture it.
      const castContent = await api.fetchRecordingData(params.id, shareToken());
      if (destroyed) return;

      // ── Parse the cast first ─────────────────────────────────────────────
      // Parsing the asciicast header gives us the authoritative
      // recorded dimensions even on the anonymous share path (where
      // the metadata endpoint 403s) — the asciicast v2 spec requires
      // `width`/`height` in the header line, and `RecordingService`
      // populates them from the live PTY size. Driving xterm from
      // these dimensions keeps every replay byte-identical to the
      // original session: the recorded output stream embeds cursor
      // positioning escapes that depend on those exact dimensions, so
      // a mismatch (e.g. fitting the terminal to the container)
      // smears wraps and miscounts cursor moves.
      engine = new PlaybackEngine();
      engine.load(castContent);

      // Reveal the player layout *before* we reach for the terminal
      // container ref. The container lives inside `<Show when={!loading() && !error()}>`,
      // so its ref callback only fires once `loading()` flips to false —
      // checking it any earlier always yields undefined and throws the
      // bogus "Terminal container missing" below. A microtask yield
      // lets Solid flush the render triggered by setLoading before
      // xterm tries to mount.
      setLoading(false);
      await Promise.resolve();
      if (destroyed) return;

      // ── Initialise xterm ────────────────────────────────────────────────
      if (!terminalContainer) throw new Error('Terminal container missing');

      // Header dims are authoritative; rec metadata is a fallback for
      // the unlikely case of a header that lacks them; 80×24 is the
      // last-resort default. Never call `fit()` — it would resize the
      // terminal grid to the container and corrupt cursor math during
      // replay.
      const initialWidth = engine.header.width || rec?.width || 80;
      const initialHeight = engine.header.height || rec?.height || 24;

      term = new XTerm({
        disableStdin: true,
        cursorBlink: false,
        cols: initialWidth,
        rows: initialHeight,
        scrollback: 10000,
        theme: {
          background: '#0d1117',
          foreground: '#c9d1d9',
          cursor: '#c9d1d9',
        },
      });
      term.open(terminalContainer);

      setDuration(engine.duration);

      engine.onOutput = (data) => term?.write(data);
      // A recorded resize event must move the terminal grid to the new
      // logical size — fitting to the container would override that
      // and break cursor math for every byte that follows.
      engine.onResize = (cols, rows) => {
        term?.resize(cols, rows);
      };
      engine.onTimeUpdate = (t) => setCurrentTime(t);
      engine.onComplete = () => {
        setIsPlaying(false);
        setCurrentTime(engine!.duration);
      };

      // ── Collab callbacks ─────────────────────────────────────────────────
      engine.onParticipantJoin = (payload: ParticipantPayload) => {
        setParticipants((prev) => {
          if (prev.some((p) => p.user_id === payload.user_id)) {
            return prev.map((p) => p.user_id === payload.user_id ? { ...p, ...payload } : p);
          }
          return [...prev, payload];
        });
      };
      engine.onParticipantLeave = (payload: { user_id: string }) => {
        setParticipants((prev) => prev.filter((p) => p.user_id !== payload.user_id));
      };
      engine.onChat = (payload: ChatPayload) => {
        // currentTime at callback invocation is the event time
        setChatMessages((prev) => [
          ...prev,
          { ...payload, time: engine!.currentTime },
        ]);
      };
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }

  // ── Controls ───────────────────────────────────────────────────────────────

  function handlePlayPause() {
    if (!engine) return;
    if (isPlaying()) {
      engine.pause();
      setIsPlaying(false);
    } else {
      engine.play();
      setIsPlaying(true);
    }
  }

  function handleSeek(seconds: number) {
    if (!engine) return;
    // Seeking replays from the beginning. xterm's `clear()` only
    // wipes the active viewport — it pushes existing rows into the
    // scrollback buffer, so a subsequent replay would stack on top
    // and the user would see doubled history when scrolling up.
    // `reset()` wipes both viewport and scrollback, which is what
    // we want for a "rewind and play again" semantic.
    term?.reset();
    // Reset collab state so participants / chat are rebuilt from the seek point.
    setParticipants([]);
    setChatMessages([]);
    engine.seek(seconds);
    setCurrentTime(seconds);
    // If the engine was playing before seek(), it resumes automatically.
    setIsPlaying(engine.state === 'playing');
  }

  function handleSpeedChange(s: number) {
    setSpeed(s);
    engine?.setSpeed(s);
  }

  function handleFullscreen() {
    if (!playerContainer) return;
    if (!document.fullscreenElement) {
      playerContainer.requestFullscreen?.();
    } else {
      document.exitFullscreen?.();
    }
  }

  return (
    <div class="player-page">
      <header class="topbar">
        <div class="topbar-left">
          <Show when={!shareToken()}>
            <a class="back-link" href="/recordings">&#8592; Recordings</a>
          </Show>
          <h1>
            <Show when={recording()} fallback={<span>Recording Playback</span>}>
              {(rec) => <span>Recording: {rec().id}</span>}
            </Show>
          </h1>
        </div>
      </header>

      <Show when={error()}>
        <Banner variant="error" onDismiss={() => setError('')}>
          {error()}
        </Banner>
      </Show>

      <Show when={loading()}>
        <div class="loading-overlay">
          <p class="muted">Loading recording…</p>
        </div>
      </Show>

      <Show when={!loading() && !error()}>
        <div class="player-layout">
          {/* Main area: terminal + controls */}
          <div class="player-main">
            <div class="player-wrapper" ref={playerContainer}>
              <div class="terminal-container" ref={terminalContainer} />
              {/* EventTimeline overlays the progress bar area */}
              <div class="pb-progress-overlay">
                <EventTimeline events={engine?.events ?? []} duration={duration()} />
              </div>
              <PlaybackControls
                currentTime={currentTime()}
                duration={duration()}
                isPlaying={isPlaying()}
                speed={speed()}
                disabled={!engine}
                onPlayPause={handlePlayPause}
                onSeek={handleSeek}
                onSpeedChange={handleSpeedChange}
                onFullscreen={handleFullscreen}
              />
            </div>

            {/* Metadata panel */}
            <Show when={recording()}>
              {(rec) => (
                <div class="meta-panel">
                  <h2>Recording Info</h2>
                  <dl class="meta-dl">
                    <dt>Session ID</dt>
                    <dd class="mono">{rec().session_id}</dd>

                    <dt>Status</dt>
                    <dd>{rec().status}</dd>

                    <dt>Duration</dt>
                    <dd>
                      {rec().duration_ms != null
                        ? `${(rec().duration_ms! / 1000).toFixed(1)}s`
                        : '--'}
                    </dd>

                    <dt>File size</dt>
                    <dd>{formatBytes(rec().file_size)}</dd>

                    <dt>Dimensions</dt>
                    <dd>{rec().width}×{rec().height}</dd>

                    <dt>Events</dt>
                    <dd>{rec().event_count.toLocaleString()}</dd>

                    <dt>Started</dt>
                    <dd>{formatDate(rec().started_at)}</dd>

                    <Show when={rec().completed_at}>
                      <dt>Completed</dt>
                      <dd>{formatDate(rec().completed_at!)}</dd>
                    </Show>

                    <Show when={rec().expires_at}>
                      <dt>Expires</dt>
                      <dd class="warn">{formatDate(rec().expires_at!)}</dd>
                    </Show>
                  </dl>
                </div>
              )}
            </Show>
          </div>

          {/* CollabSidebar */}
          <CollabSidebar
            participants={participants()}
            chatMessages={chatMessages()}
            currentTime={currentTime()}
          />
        </div>
      </Show>

      <style>{`
        .player-page { min-height: 100vh; display: flex; flex-direction: column; }

        .topbar {
          display: flex;
          align-items: center;
          padding: 12px 24px;
          border-bottom: 1px solid var(--border);
          background: var(--bg-secondary);
        }
        .topbar-left { display: flex; align-items: center; gap: 16px; }
        .topbar h1 { font-size: 16px; font-weight: 600; }
        .back-link {
          font-size: 13px;
          color: var(--text-secondary);
          text-decoration: none;
          transition: color 0.15s;
        }
        .back-link:hover { color: var(--text-primary); }

        .loading-overlay {
          flex: 1;
          display: flex;
          align-items: center;
          justify-content: center;
          padding: 48px;
        }
        .muted { color: var(--text-secondary); font-size: 14px; }

        /* Outer layout: main area (terminal + meta) + right sidebar */
        .player-layout {
          flex: 1;
          display: flex;
          flex-direction: row;
          overflow: hidden;
        }

        /* Left column: stacks terminal wrapper + meta panel vertically */
        .player-main {
          flex: 1;
          display: flex;
          flex-direction: column;
          min-width: 0;
          overflow-y: auto;
        }

        /* The terminal sits inside a black wrapper — it should fill the width
           and show a fixed-height terminal viewport above the controls bar. */
        .player-wrapper {
          background: #0d1117;
          display: flex;
          flex-direction: column;
          min-height: 0;
          position: relative;
        }
        .terminal-container {
          flex: 1;
          min-height: 400px;
          overflow: hidden;
          padding: 8px;
        }

        /* EventTimeline overlay sits inside the pb-controls progress track.
           We position a thin relative wrapper just before PlaybackControls
           renders its own .pb-progress-track, so the overlay aligns exactly. */
        .pb-progress-overlay {
          position: relative;
          height: 4px;
          margin: 4px 12px 0;
          pointer-events: none;
        }

        /* When fullscreened, the player wrapper fills the screen. */
        .player-wrapper:fullscreen,
        .player-wrapper:-webkit-full-screen {
          width: 100vw;
          height: 100vh;
        }
        .player-wrapper:fullscreen .terminal-container,
        .player-wrapper:-webkit-full-screen .terminal-container {
          flex: 1;
        }

        /* Metadata panel */
        .meta-panel {
          padding: 20px 24px;
          border-top: 1px solid var(--border);
          background: var(--bg-secondary);
          box-sizing: border-box;
        }
        .meta-panel h2 {
          font-size: 14px;
          font-weight: 600;
          color: var(--text-secondary);
          margin-bottom: 12px;
        }
        .meta-dl {
          display: grid;
          grid-template-columns: 120px 1fr;
          gap: 6px 16px;
          font-size: 13px;
        }
        .meta-dl dt { color: var(--text-secondary); }
        .meta-dl dd { color: var(--text-primary); margin: 0; }
        .mono { font-family: var(--font-mono); font-size: 12px; }
        .warn { color: var(--warning, #d29922); }
      `}</style>
    </div>
  );
}
