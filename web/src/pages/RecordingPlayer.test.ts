import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { captureShareTokenFromHash } from './RecordingPlayer';

// These tests pin the privacy contract of the fragment-token
// capture helper used by anonymous recording share links:
//   1. The token round-trips from `location.hash` into the caller.
//   2. `history.replaceState` is called synchronously so the secret
//      does not linger in the address bar, browser history, or
//      `document.referrer` on the next navigation.
//   3. Hostile / malformed fragments yield `undefined` (no throw,
//      no partial token, no accidental leak of a nearby value like
//      `#t=30s`).
//   4. `replaceState` failures are swallowed — a sandboxed iframe
//      or throttled history API must not crash the player.
//
// The helper is exported from `RecordingPlayer.tsx` purely so it
// can be exercised here without booting xterm, the router, and the
// lazy `api` module; see its jsdoc for the rationale.

function setLocationHash(hash: string) {
  // jsdom's `window.location.hash = '...'` actually mutates the
  // URL, so we can just assign. `pathname` / `search` keep their
  // defaults from the jsdom test URL (`about:blank` → `/`).
  window.location.hash = hash;
}

describe('captureShareTokenFromHash', () => {
  let replaceStateSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    // Reset the URL between tests — jsdom persists mutations
    // across `it` blocks within the same describe. Spy AFTER
    // the reset so the reset call itself doesn't show up in
    // the spy's call log.
    window.history.replaceState(null, '', '/');
    replaceStateSpy = vi.spyOn(window.history, 'replaceState');
  });

  afterEach(() => {
    replaceStateSpy.mockRestore();
  });

  it('returns undefined when no fragment is present', () => {
    expect(captureShareTokenFromHash()).toBeUndefined();
    expect(replaceStateSpy).not.toHaveBeenCalled();
  });

  it('returns undefined for a fragment without a token field', () => {
    setLocationHash('#t=30s');
    expect(captureShareTokenFromHash()).toBeUndefined();
    // No token to scrub — leave history untouched.
    expect(replaceStateSpy).not.toHaveBeenCalled();
  });

  it('extracts a token from a plain `#token=...` fragment', () => {
    setLocationHash('#token=abcdef1234');
    expect(captureShareTokenFromHash()).toBe('abcdef1234');
    expect(replaceStateSpy).toHaveBeenCalledTimes(1);
  });

  it('extracts a token from a multi-field fragment', () => {
    setLocationHash('#t=30s&token=abcdef&utm=x');
    expect(captureShareTokenFromHash()).toBe('abcdef');
    expect(replaceStateSpy).toHaveBeenCalledTimes(1);
  });

  it('scrubs the fragment from the URL after extraction', () => {
    setLocationHash('#token=s3cret');
    captureShareTokenFromHash();
    // `replaceState` was called with a url sans fragment; inspect the
    // 3rd argument. jsdom's spy preserves the original call args.
    const [, , newUrl] = replaceStateSpy.mock.calls[0];
    expect(String(newUrl)).not.toContain('#');
    expect(String(newUrl)).not.toContain('token');
  });

  it('swallows replaceState errors without throwing', () => {
    setLocationHash('#token=s3cret');
    replaceStateSpy.mockImplementation(() => {
      throw new Error('history throttled');
    });
    // Must not throw: a sandboxed iframe or throttled history API
    // degrades cleanly instead of crashing the player page. The
    // token is still returned (the caller holds it in a closure)
    // so playback proceeds; only the URL-bar scrub is degraded.
    expect(() => captureShareTokenFromHash()).not.toThrow();
    expect(captureShareTokenFromHash()).toBe('s3cret');
  });
});
