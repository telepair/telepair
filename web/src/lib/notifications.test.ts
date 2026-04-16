// web/src/lib/notifications.test.ts
import { describe, it, expect, beforeEach, vi } from 'vitest';

let permissionState: NotificationPermission = 'default';
let requestResult: NotificationPermission = 'granted';
const instances: Array<{ title: string; body?: string; onclick: (() => void) | null; close: () => void }> = [];

vi.stubGlobal('Notification', class {
  static get permission() { return permissionState; }
  static requestPermission = vi.fn(async () => {
    permissionState = requestResult;
    return requestResult;
  });
  title: string;
  body?: string;
  onclick: (() => void) | null = null;
  close = vi.fn();
  constructor(title: string, options?: { body?: string }) {
    this.title = title;
    this.body = options?.body;
    instances.push(this as any);
  }
});

const { isSupported, requestPermission, notify } = await import('./notifications');

beforeEach(() => {
  permissionState = 'default';
  requestResult = 'granted';
  instances.length = 0;
  vi.clearAllMocks();
});

describe('isSupported', () => {
  it('returns true when Notification is in window', () => {
    expect(isSupported()).toBe(true);
  });
});

describe('requestPermission', () => {
  it('returns the permission result', async () => {
    const result = await requestPermission();
    expect(result).toBe('granted');
    expect(Notification.requestPermission).toHaveBeenCalled();
  });

  it('returns denied when user denies', async () => {
    requestResult = 'denied';
    const result = await requestPermission();
    expect(result).toBe('denied');
  });
});

describe('notify', () => {
  it('creates a Notification with title and body', () => {
    permissionState = 'granted';
    notify('telepair', 'Hello world');
    expect(instances).toHaveLength(1);
    expect(instances[0].title).toBe('telepair');
    expect(instances[0].body).toBe('Hello world');
  });

  it('does not create Notification when permission is not granted', () => {
    permissionState = 'denied';
    notify('telepair', 'Hello');
    expect(instances).toHaveLength(0);
  });

  it('sets onclick to focus window and close notification', () => {
    permissionState = 'granted';
    const mockFocus = vi.spyOn(window, 'focus').mockImplementation(() => {});
    notify('telepair', 'test');
    expect(instances[0].onclick).toBeTypeOf('function');
    instances[0].onclick!();
    expect(mockFocus).toHaveBeenCalled();
    expect(instances[0].close).toHaveBeenCalled();
    mockFocus.mockRestore();
  });

  it('truncates body longer than 100 chars', () => {
    permissionState = 'granted';
    const longText = 'a'.repeat(150);
    notify('telepair', longText);
    expect(instances[0].body).toBe('a'.repeat(99) + '…');
  });
});
