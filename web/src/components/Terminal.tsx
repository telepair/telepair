// web/src/components/Terminal.tsx
import { onMount, onCleanup } from 'solid-js';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import '@xterm/xterm/css/xterm.css';

export interface TerminalHandle {
  write(data: string | Uint8Array): void;
  focus(): void;
  dispose(): void;
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

  onMount(() => {
    if (!containerRef) return;

    term = new XTerm({
      cursorBlink: true,
      cursorStyle: 'block',
      fontSize: 14,
      fontFamily: "'Menlo', 'Monaco', 'Courier New', monospace",
      scrollback: 10000,
      theme: {
        background: '#0d1117',
        foreground: '#e6edf3',
        cursor: '#e6edf3',
        selectionBackground: 'rgba(88, 166, 255, 0.3)',
        black: '#484f58',
        red: '#ff7b72',
        green: '#3fb950',
        yellow: '#d29922',
        blue: '#58a6ff',
        magenta: '#bc8cff',
        cyan: '#39c5cf',
        white: '#b1bac4',
        brightBlack: '#6e7681',
        brightRed: '#ffa198',
        brightGreen: '#56d364',
        brightYellow: '#e3b341',
        brightBlue: '#79c0ff',
        brightMagenta: '#d2a8ff',
        brightCyan: '#56d4dd',
        brightWhite: '#f0f6fc',
      },
    });

    fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(containerRef);

    // Expose xterm instance for E2E test buffer access
    (containerRef as any).__xterm = term;

    // Try WebGL renderer for performance
    try {
      term.loadAddon(new WebglAddon());
    } catch {
      // WebGL not available, fall back to canvas
    }

    fitAddon.fit();

    // Forward user input
    term.onData((data) => props.onData(data));

    // Forward resize
    term.onResize(({ cols, rows }) => props.onResize(cols, rows));

    // Auto-fit on container resize (debounced to avoid flooding server with resize messages)
    resizeObserver = new ResizeObserver(() => {
      clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => fitAddon?.fit(), 100);
    });
    resizeObserver.observe(containerRef);

    // Expose handle to parent
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
    });

    term.focus();
  });

  onCleanup(() => {
    clearTimeout(resizeTimer);
    resizeObserver?.disconnect();
    term?.dispose();
  });

  return (
    <div
      ref={containerRef}
      style={{ width: '100%', height: '100%', overflow: 'hidden' }}
    />
  );
}
