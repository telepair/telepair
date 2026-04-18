import { describe, expect, it } from 'vitest';
import { shouldNotify } from './Session';

// `shouldNotify` is the three-gate AND that decides whether an
// incoming chat / peer-joined event raises a browser notification.
// Each gate is there for a specific UX reason (user preference
// kill-switch, don't-notify-when-tab-focused, don't-notify-yourself)
// and a regression in any one of them is a real-world annoyance that
// won't show up in e2e tests. Exercising the matrix here is the
// cheapest way to pin the contract — the component itself is hard
// to boot without mocking the router, WebSocket, and xterm.
//
// `shouldNotifyFromStores` (the live-wire version that reads the
// terminalSettings / auth / document stores) is covered by
// inspection; the pure helper is the one that can drift.

const BASE = {
  notificationsEnabled: true,
  visibilityState: 'hidden' as DocumentVisibilityState,
  currentUserId: 'me',
};

describe('shouldNotify', () => {
  it('notifies for a peer message with all gates passing', () => {
    expect(shouldNotify('peer', BASE)).toBe(true);
  });

  it('does not notify when preferences disable it', () => {
    expect(shouldNotify('peer', { ...BASE, notificationsEnabled: false })).toBe(false);
  });

  it('does not notify when the tab is visible', () => {
    expect(shouldNotify('peer', { ...BASE, visibilityState: 'visible' })).toBe(false);
  });

  it('does not notify for messages from the current user', () => {
    expect(shouldNotify('me', BASE)).toBe(false);
  });

  it('treats prerender / unloaded visibilityState as not-visible (still notifies)', () => {
    // `prerender` and `unloaded` are valid non-visible states per the
    // Page Visibility API. The user isn't looking at the tab so the
    // notification should still fire — the visibility gate is a
    // positive check for `visible`, not a whitelist of `hidden`.
    expect(shouldNotify('peer', { ...BASE, visibilityState: 'prerender' as DocumentVisibilityState }))
      .toBe(true);
  });

  it('notifies when currentUserId is null (anonymous viewer)', () => {
    // An unauthenticated recording viewer has `currentUserId === null`.
    // Peer messages must still notify them — the sender id can never
    // match `null`, so the self-gate is trivially satisfied.
    expect(shouldNotify('peer', { ...BASE, currentUserId: null })).toBe(true);
  });

  it('suppresses self-notification even when currentUserId is empty string', () => {
    // Paranoia: avoid the classic `'' === undefined` trap. Only an
    // exact match should suppress.
    expect(shouldNotify('', { ...BASE, currentUserId: '' })).toBe(false);
    expect(shouldNotify('peer', { ...BASE, currentUserId: '' })).toBe(true);
  });
});
