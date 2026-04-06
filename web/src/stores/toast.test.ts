import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { toast } from './toast';

beforeEach(() => {
  toast.clear();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('toast.info', () => {
  it('adds a toast to the list', () => {
    toast.info('hello');
    const list = toast.list();
    expect(list.length).toBe(1);
    expect(list[0].text).toBe('hello');
    expect(list[0].variant).toBe('info');
    expect(list[0].dismissible).toBe(true);
  });

  it('auto-dismisses after the default duration (3000ms)', () => {
    toast.info('hi');
    expect(toast.list().length).toBe(1);
    vi.advanceTimersByTime(2999);
    expect(toast.list().length).toBe(1);
    vi.advanceTimersByTime(1);
    expect(toast.list().length).toBe(0);
  });

  it('returns a stable id', () => {
    const a = toast.info('a');
    const b = toast.info('b');
    expect(a).not.toBe(b);
  });
});

describe('toast.success / toast.warning', () => {
  it('success uses the success variant and dismisses at 3000ms', () => {
    toast.success('ok');
    expect(toast.list()[0].variant).toBe('success');
    vi.advanceTimersByTime(3000);
    expect(toast.list().length).toBe(0);
  });

  it('warning uses the warning variant and dismisses at 5000ms', () => {
    toast.warning('careful');
    expect(toast.list()[0].variant).toBe('warning');
    vi.advanceTimersByTime(4999);
    expect(toast.list().length).toBe(1);
    vi.advanceTimersByTime(1);
    expect(toast.list().length).toBe(0);
  });
});

describe('toast.error', () => {
  it('is sticky by default', () => {
    toast.error('boom');
    vi.advanceTimersByTime(60_000);
    expect(toast.list().length).toBe(1);
    expect(toast.list()[0].variant).toBe('error');
  });
});

describe('toast.dismiss', () => {
  it('removes a toast by id', () => {
    const id = toast.info('x');
    toast.dismiss(id);
    expect(toast.list().length).toBe(0);
  });

  it('is a no-op for unknown ids', () => {
    toast.info('x');
    toast.dismiss('nonexistent');
    expect(toast.list().length).toBe(1);
  });

  it('cancels the pending auto-dismiss timer', () => {
    const id = toast.info('x');
    toast.dismiss(id);
    // Adding another toast afterwards should not be affected by a stale timer.
    toast.info('y');
    vi.advanceTimersByTime(2999);
    expect(toast.list().length).toBe(1);
    expect(toast.list()[0].text).toBe('y');
  });
});

describe('toast dedupe via stable id', () => {
  it('replaces the existing toast when a new one uses the same id', () => {
    toast.info('first', { id: 'progress' });
    toast.success('second', { id: 'progress' });
    expect(toast.list().length).toBe(1);
    expect(toast.list()[0].text).toBe('second');
    expect(toast.list()[0].variant).toBe('success');
  });

  it('resets the auto-dismiss timer on replace', () => {
    toast.info('first', { id: 'progress', duration: 1000 });
    vi.advanceTimersByTime(800);
    toast.info('second', { id: 'progress', duration: 1000 });
    vi.advanceTimersByTime(999);
    expect(toast.list().length).toBe(1);
    vi.advanceTimersByTime(2);
    expect(toast.list().length).toBe(0);
  });
});

describe('toast custom options', () => {
  it('respects a custom duration', () => {
    toast.info('x', { duration: 1500 });
    vi.advanceTimersByTime(1499);
    expect(toast.list().length).toBe(1);
    vi.advanceTimersByTime(1);
    expect(toast.list().length).toBe(0);
  });

  it('treats duration 0 as sticky', () => {
    toast.info('x', { duration: 0 });
    vi.advanceTimersByTime(60_000);
    expect(toast.list().length).toBe(1);
  });

  it('treats negative duration as sticky', () => {
    toast.info('x', { duration: -1 });
    vi.advanceTimersByTime(60_000);
    expect(toast.list().length).toBe(1);
  });

  it('allows disabling dismissible', () => {
    toast.info('x', { dismissible: false });
    expect(toast.list()[0].dismissible).toBe(false);
  });

  it('stores an action', () => {
    const fn = vi.fn();
    toast.error('fail', { action: { label: 'Retry', onClick: fn } });
    const t = toast.list()[0];
    expect(t.action?.label).toBe('Retry');
    t.action?.onClick();
    expect(fn).toHaveBeenCalledOnce();
  });
});

describe('toast.clear', () => {
  it('removes all toasts and cancels all timers', () => {
    toast.info('a');
    toast.warning('b');
    toast.error('c');
    expect(toast.list().length).toBe(3);
    toast.clear();
    expect(toast.list().length).toBe(0);
    // Ensure no stray timers fire into a new toast.
    toast.info('fresh');
    vi.advanceTimersByTime(2999);
    expect(toast.list().length).toBe(1);
  });
});
