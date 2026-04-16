// web/src/lib/storage.ts
//
// Safe localStorage accessors. localStorage can throw in private
// browsing, sandboxed iframes, or when the quota is exceeded. These
// helpers treat any failure as "no storage" and keep going rather
// than crashing the app.

export function safeGet(key: string): string | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function safeSet(key: string, value: string): void {
  try {
    if (typeof localStorage !== 'undefined') localStorage.setItem(key, value);
  } catch {
    // quota / private mode
  }
}

export function safeRemove(key: string): void {
  try {
    if (typeof localStorage !== 'undefined') localStorage.removeItem(key);
  } catch {
    // ignore
  }
}
