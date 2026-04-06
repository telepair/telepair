// web/src/stores/toast.ts
import { createSignal } from 'solid-js';

export type ToastVariant = 'info' | 'success' | 'warning' | 'error';

export type ToastAction = {
  label: string;
  onClick: () => void;
};

export type ToastOptions = {
  /** Milliseconds before auto-dismiss. 0 or negative = sticky. Defaults per variant. */
  duration?: number;
  /** Whether the user can manually close the toast. Default: true. */
  dismissible?: boolean;
  /** Optional action button rendered next to the text. */
  action?: ToastAction;
  /** Stable id used for dedupe — a push with the same id replaces the previous one. */
  id?: string;
};

export type Toast = {
  id: string;
  variant: ToastVariant;
  text: string;
  dismissible: boolean;
  action?: ToastAction;
};

const DEFAULT_DURATION: Record<ToastVariant, number> = {
  info: 3000,
  success: 3000,
  warning: 5000,
  error: 0, // sticky until dismissed
};

const [toasts, setToasts] = createSignal<Toast[]>([]);
const timers = new Map<string, ReturnType<typeof setTimeout>>();
let nextAutoId = 1;

function push(variant: ToastVariant, text: string, opts: ToastOptions = {}): string {
  const id = opts.id ?? `auto-${nextAutoId++}`;
  // Dedupe: if a toast with this id already exists, remove it (and its timer) first.
  clearTimer(id);
  setToasts((prev) => prev.filter((t) => t.id !== id));

  const next: Toast = {
    id,
    variant,
    text,
    dismissible: opts.dismissible ?? true,
    action: opts.action,
  };
  setToasts((prev) => [...prev, next]);

  const duration = opts.duration ?? DEFAULT_DURATION[variant];
  if (duration > 0) {
    const handle = setTimeout(() => dismiss(id), duration);
    timers.set(id, handle);
  }
  return id;
}

function clearTimer(id: string) {
  const h = timers.get(id);
  if (h !== undefined) {
    clearTimeout(h);
    timers.delete(id);
  }
}

function dismiss(id: string) {
  clearTimer(id);
  setToasts((prev) => prev.filter((t) => t.id !== id));
}

function clear() {
  for (const h of timers.values()) clearTimeout(h);
  timers.clear();
  setToasts([]);
}

export const toast = {
  list: toasts,
  info: (text: string, opts?: ToastOptions) => push('info', text, opts),
  success: (text: string, opts?: ToastOptions) => push('success', text, opts),
  warning: (text: string, opts?: ToastOptions) => push('warning', text, opts),
  error: (text: string, opts?: ToastOptions) => push('error', text, opts),
  dismiss,
  clear,
};
