// web/src/lib/notifications.ts

const MAX_BODY_LENGTH = 100;

export function isSupported(): boolean {
  return typeof window !== 'undefined' && 'Notification' in window;
}

export async function requestPermission(): Promise<NotificationPermission> {
  if (!isSupported()) return 'denied';
  return Notification.requestPermission();
}

export function notify(title: string, body: string): void {
  if (!isSupported() || Notification.permission !== 'granted') return;
  const truncated = body.length > MAX_BODY_LENGTH
    ? body.slice(0, MAX_BODY_LENGTH - 1) + '…'
    : body;
  const n = new Notification(title, { body: truncated });
  n.onclick = () => {
    window.focus();
    n.close();
  };
}
