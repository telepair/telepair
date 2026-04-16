// web/src/components/Terminal.tsx
import { onMount, onCleanup, createEffect } from 'solid-js';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import '@xterm/xterm/css/xterm.css';
import { terminalSettings } from '../stores/settings';
import { themes, fontCss } from '../lib/terminal-themes';

export interface TerminalHandle {
  write(data: string | Uint8Array): void;
  focus(): void;
  dispose(): void;
  /** Toggle read-only mode. When true: keyboard events are consumed
   *  before xterm processes them, `onData` is suppressed, and the
   *  cursor stops blinking so the textarea visibly reflects the lock.
   *  Used for viewer-role demotion so the user doesn't face a dead
   *  prompt with no feedback. */
  setReadOnly(flag: boolean): void;
  /** Fit-computed size; read by the parent to size the initial
   *  `SessionJoin` frame so the server PTY spawns at the right dims. */
  readonly cols: number;
  readonly rows: number;
}

interface TerminalProps {
  onData: (data: string) => void;
  onResize: (cols: number, rows: number) => void;
  ref?: (handle: TerminalHandle) => void;
}

export default function Terminal(props: TerminalProps) {
  let containerRef: HTMLDivElement | undefined;
  let term: XTerm | undefined;
  let fitAddon: FitAddon | undefined;
  let resizeObserver: ResizeObserver | undefined;
  let resizeTimer: ReturnType<typeof setTimeout> | undefined;
  let onVisibilityChange: (() => void) | undefined;
  // `readOnly` is set via the exposed handle and read inside xterm
  // callbacks; keeping it outside the xterm option bag means we avoid
  // touching internal xterm APIs that differ across minor versions.
  let readOnly = false;

  onMount(() => {
    if (!containerRef) return;

    const s = terminalSettings();
    term = new XTerm({
      cursorBlink: s.cursorBlink,
      cursorStyle: s.cursorStyle,
      fontSize: s.fontSize,
      fontFamily: fontCss(s.fontFamily),
      scrollback: 10000,
      theme: themes[s.theme],
    });

    fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(containerRef);

    // Expose xterm instance for E2E test buffer access
    (containerRef as any).__xterm = term;

    // Try WebGL renderer for performance. Keep a reference so we can
    // rebuild its texture atlas once the bundled Nerd Font finishes
    // loading — otherwise the first paint uses fallback-font glyph
    // metrics and prompts render with shifted Nerd Font icons.
    let webglAddon: WebglAddon | undefined;
    try {
      webglAddon = new WebglAddon();
      term.loadAddon(webglAddon);
    } catch {
      // WebGL not available, fall back to canvas
    }

    // When the async webfont lands, purge cached glyph textures and
    // refit so character cells are re-measured against the real font.
    if (typeof document !== 'undefined' && document.fonts?.load) {
      document.fonts
        .load(`${s.fontSize}px "JetBrainsMono Nerd Font Mono"`)
        .then(() => {
          webglAddon?.clearTextureAtlas();
          fitAddon?.fit();
        })
        .catch(() => {
          // Font failed to load — keep the fallback stack, nothing to do
        });
    }

    // Forward user input — but honour the read-only latch so demoted
    // viewers can't leak keystrokes to the server even if the parent
    // forgets to gate at the `handleData` layer. Belt-and-braces with
    // `attachCustomKeyEventHandler` below: the key handler stops xterm
    // from echoing / interpreting the event, and this guard stops any
    // alternate data path (e.g. clipboard paste) from emitting too.
    term.onData((data) => {
      if (readOnly) return;
      props.onData(data);
    });

    // Register onResize BEFORE fit() so the initial 80×24 → real-size
    // event actually reaches the parent. fit() fires onResize
    // synchronously; registering after means the parent never sees the
    // first snap and the server PTY stays at 80×24.
    term.onResize(({ cols, rows }) => props.onResize(cols, rows));

    fitAddon.fit();

    // Auto-fit on container resize (debounced to avoid flooding server with resize messages)
    resizeObserver = new ResizeObserver(() => {
      clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => fitAddon?.fit(), 100);
    });
    resizeObserver.observe(containerRef);

    // Re-fit when the tab returns to the foreground. While the tab is
    // backgrounded the browser throttles `requestAnimationFrame`, so
    // xterm's renderer can miss layout ticks triggered by e.g. a
    // sidebar toggle that happened during the background window. On
    // re-show, the cell grid is painted against the last-known size
    // and the terminal looks visibly tiny until the next real resize.
    // A one-shot `fit()` on visibility-change snaps it back. Only fire
    // when the tab is actually visible so the handler is a no-op on
    // the hide leg of the toggle.
    onVisibilityChange = () => {
      if (document.visibilityState === 'visible') {
        fitAddon?.fit();
      }
    };
    document.addEventListener('visibilitychange', onVisibilityChange);

    props.ref?.({
      write(data: string | Uint8Array) {
        term?.write(data);
      },
      focus() {
        term?.focus();
      },
      dispose() {
        term?.dispose();
      },
      setReadOnly(flag: boolean) {
        readOnly = flag;
        if (!term) return;
        // xterm 6.x has no first-class read-only mode; the custom key
        // handler returns false to abort processing of every
        // KeyboardEvent so the user's keystrokes never reach xterm's
        // writer or fire `onData`. Returning true (the install-time
        // default) lets the event through.
        term.attachCustomKeyEventHandler(() => !flag);
        term.options.cursorBlink = !flag;
        containerRef?.classList.toggle('terminal-readonly', flag);
      },
      get cols() { return term?.cols ?? 80; },
      get rows() { return term?.rows ?? 24; },
    });

    term.focus();
  });

  // Reactively apply settings changes at runtime without recreating
  // the xterm instance. Each property assignment triggers xterm's
  // internal renderer update. Hoisted to component scope (not nested
  // in onMount) per SolidJS convention; the `term` / `fitAddon` guard
  // handles the pre-mount window. The `mounted` flag skips the first
  // run because onMount already initialised the terminal with the
  // current settings — a redundant fit() would send a duplicate resize
  // frame to the server PTY.
  let mounted = false;
  createEffect(() => {
    const s = terminalSettings();
    if (!term || !fitAddon) return;
    if (!mounted) { mounted = true; return; }
    term.options.theme = themes[s.theme];
    term.options.fontSize = s.fontSize;
    term.options.fontFamily = fontCss(s.fontFamily);
    term.options.cursorStyle = s.cursorStyle;
    if (!readOnly) {
      term.options.cursorBlink = s.cursorBlink;
    }
    fitAddon.fit();
  });

  onCleanup(() => {
    clearTimeout(resizeTimer);
    resizeObserver?.disconnect();
    if (onVisibilityChange) {
      document.removeEventListener('visibilitychange', onVisibilityChange);
    }
    term?.dispose();
  });

  return (
    <div
      ref={containerRef}
      style={{ width: '100%', height: '100%', overflow: 'hidden' }}
    />
  );
}
