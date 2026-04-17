import { test, expect } from '@playwright/test';
import {
  getAdminToken,
  gotoSession,
  login,
  typeInTerminal,
  waitForTerminal,
} from './helpers';

// These tests run serially and depend on each other's side effects:
// test (1) asserts the empty state before any recording exists, then
// tests (2)-(4) create + stop recordings that tests (5)-(6) consume.
// Playwright's `.describe.serial` stops the block on the first failure,
// which keeps the later tests from running against a corrupted list
// view rather than piling on cascading failures.
test.describe.serial('Recording feature', () => {
  test('recordings page shows empty state before any recording exists', async ({ page }) => {
    await login(page);
    await page.goto('/recordings');

    await expect(page.getByRole('heading', { name: 'Recordings' })).toBeVisible();
    await expect(page.getByText('No recordings found')).toBeVisible();
    await expect(page.getByText(/Start a session and click/)).toBeVisible();
  });

  test('session owner sees REC indicator + Stop button while a recording is active', async ({
    page,
    request,
  }) => {
    const sessionId = await gotoSession(page);
    await waitForTerminal(page);
    const token = getAdminToken();

    // Driving recording via the REST API keeps the test honest about
    // what the UI is wired to react to (the RecordingStarted WS frame),
    // instead of poking Solid signals directly.
    const start = await request.post(`/api/sessions/${sessionId}/recording/start`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(start.ok()).toBe(true);

    const indicator = page.locator('.rec-indicator');
    await expect(indicator).toBeVisible({ timeout: 5_000 });
    await expect(indicator.locator('.rec-label')).toHaveText('REC');
    await expect(page.getByRole('button', { name: 'Stop recording' })).toBeVisible();

    // Generate a small amount of PTY output so the writer flushes a
    // non-trivial .cast file. The player test later relies on this.
    await typeInTerminal(page, 'echo hello-recording');
    await page.waitForTimeout(300);

    const stop = await request.post(`/api/sessions/${sessionId}/recording/stop`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(stop.ok()).toBe(true);

    await expect(indicator).not.toBeVisible({ timeout: 5_000 });
    // Share Rec button only appears after the recording has finalised —
    // it's bound to `recordingId() && !isRecording()` in Session.tsx.
    await expect(page.getByRole('button', { name: 'Share Rec' })).toBeVisible();
  });

  test('owner can open the Share Recording dialog from the session topbar', async ({
    page,
    request,
  }) => {
    // Fresh session + recording so the dialog has its own context; the
    // per-test page resets auth state anyway, and we don't want to rely
    // on the session ID from the previous test bleeding through.
    const sessionId = await gotoSession(page);
    await waitForTerminal(page);
    const token = getAdminToken();

    const start = await request.post(`/api/sessions/${sessionId}/recording/start`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(start.ok()).toBe(true);
    await page.waitForTimeout(200);
    const stop = await request.post(`/api/sessions/${sessionId}/recording/stop`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(stop.ok()).toBe(true);

    await expect(page.getByRole('button', { name: 'Share Rec' })).toBeVisible({ timeout: 5_000 });
    await page.getByRole('button', { name: 'Share Rec' }).click();

    const dialog = page.getByRole('dialog', { name: 'Share recording' });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole('heading', { name: 'Share Recording' })).toBeVisible();
    await expect(dialog.getByRole('button', { name: /Create Share Link/ })).toBeVisible();
    await expect(dialog.getByText('No share links yet.')).toBeVisible();

    await dialog.getByRole('button', { name: 'Close' }).click();
    await expect(dialog).not.toBeVisible();
  });

  test('recordings list shows completed recordings with a working status filter', async ({
    page,
  }) => {
    await login(page);
    await page.goto('/recordings');

    const firstCard = page.locator('.recording-card').first();
    await expect(firstCard).toBeVisible({ timeout: 5_000 });

    // At least one row finalised to "completed" from the earlier tests.
    const completedCards = page.locator('.recording-card[data-status="completed"]');
    expect(await completedCards.count()).toBeGreaterThan(0);
    await expect(completedCards.first().locator('.rec-badge.badge-completed')).toHaveText(
      'Completed',
    );

    // Filter tab: "Recording" (no live recordings at this point) must
    // flip the list to its empty-for-filter state without erasing the
    // tab bar.
    await page.getByRole('tab', { name: 'Recording' }).click();
    await expect(page.getByText(/No recordings with status "recording"/)).toBeVisible();

    // Back to "Completed" — the same card(s) should reappear.
    await page.getByRole('tab', { name: 'Completed' }).click();
    await expect(page.locator('.recording-card[data-status="completed"]').first()).toBeVisible();
  });

  test('clicking Play on a recording navigates to the player and renders controls', async ({
    page,
  }) => {
    await login(page);
    await page.goto('/recordings');

    const card = page.locator('.recording-card[data-status="completed"]').first();
    await expect(card).toBeVisible({ timeout: 5_000 });
    const recId = (await card.locator('.rec-id').innerText()).trim();

    await card.getByRole('button', { name: /Play/ }).click();
    await page.waitForURL(new RegExp(`/recordings/${recId}$`));

    // Player page: terminal container, metadata panel, and the
    // playback controls bar should all mount once the .cast fetch
    // completes. The progress "slider" role comes from
    // PlaybackControls.tsx's aria-labelled div.
    await expect(page.locator('.terminal-container')).toBeVisible({ timeout: 10_000 });
    await expect(page.getByRole('heading', { name: 'Recording Info' })).toBeVisible();
    await expect(page.locator('.pb-controls')).toBeVisible();
    await expect(page.getByRole('slider', { name: 'Seek' })).toBeVisible();
    // Play/Pause button — starts in the "Play" label state because the
    // player does not auto-play.
    await expect(page.getByRole('button', { name: 'Play' })).toBeVisible();
  });

  test('anonymous share link plays back without an account', async ({
    page,
    request,
    browser,
  }) => {
    // Spin up a fresh session + recording so we can mint a share for
    // this specific run rather than hunting for an existing one.
    const sessionId = await gotoSession(page);
    await waitForTerminal(page);
    const token = getAdminToken();

    expect(
      (await request.post(`/api/sessions/${sessionId}/recording/start`, {
        headers: { Authorization: `Bearer ${token}` },
      })).ok(),
    ).toBe(true);
    await typeInTerminal(page, 'echo anon-share');
    await page.waitForTimeout(300);
    expect(
      (await request.post(`/api/sessions/${sessionId}/recording/stop`, {
        headers: { Authorization: `Bearer ${token}` },
      })).ok(),
    ).toBe(true);

    // Pull the recording id from the list endpoint — easier than
    // fishing it out of the WS frame.
    const list = await request.get('/api/recordings', {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(list.ok()).toBe(true);
    const rows = (await list.json()) as Array<{ id: string; session_id: string }>;
    const rec = rows.find((r) => r.session_id === sessionId);
    expect(rec, 'recording row for the session').toBeTruthy();
    const recordingId = rec!.id;

    const mint = await request.post(`/api/recordings/${recordingId}/shares`, {
      headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
      data: { max_uses: 0 },
    });
    expect(mint.ok()).toBe(true);
    const { token: shareToken } = (await mint.json()) as { token: string };

    // Open the share URL in a fresh, unauthenticated context — this
    // exercises both the `/recordings/:id/play` route and the
    // AuthGuard bypass that anonymous viewers depend on.
    const anon = await browser.newContext();
    const anonPage = await anon.newPage();
    await anonPage.goto(
      `/recordings/${recordingId}/play?token=${encodeURIComponent(shareToken)}`,
    );
    await expect(anonPage.locator('.terminal-container')).toBeVisible({ timeout: 10_000 });
    await expect(anonPage.getByRole('button', { name: 'Play' })).toBeVisible();
    // Critically: an anonymous viewer must NOT have been bounced to
    // /login by the AuthGuard before the player mounted.
    expect(anonPage.url()).not.toContain('/login');
    await anon.close();
  });

  test('revoking a share token blocks subsequent playback', async ({ request }) => {
    const token = getAdminToken();
    // Reuse one of the recordings completed by earlier tests so we
    // can keep this test independent of any specific session id.
    const list = await request.get('/api/recordings', {
      headers: { Authorization: `Bearer ${token}` },
    });
    const rows = (await list.json()) as Array<{ id: string }>;
    expect(rows.length, 'at least one recording from prior tests').toBeGreaterThan(0);
    const recordingId = rows[0].id;

    const mint = await request.post(`/api/recordings/${recordingId}/shares`, {
      headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
      data: { max_uses: 0 },
    });
    expect(mint.ok()).toBe(true);
    const { token: shareToken, share } = (await mint.json()) as {
      token: string;
      share: { token_sha256: string };
    };

    // Pre-revoke: anonymous fetch with the token must work.
    const ok = await request.get(
      `/api/recordings/${recordingId}/data?token=${encodeURIComponent(shareToken)}`,
    );
    expect(ok.ok()).toBe(true);

    // Revoke via the SHA-256 path (the URL no longer carries the
    // raw secret) and confirm the server actually deletes the row.
    const revoke = await request.delete(
      `/api/recordings/${recordingId}/shares/${share.token_sha256}`,
      { headers: { Authorization: `Bearer ${token}` } },
    );
    expect(revoke.status()).toBe(204);

    // Post-revoke: anonymous fetch with the same token must fail.
    const denied = await request.get(
      `/api/recordings/${recordingId}/data?token=${encodeURIComponent(shareToken)}`,
    );
    expect(denied.ok()).toBe(false);
    expect([400, 401, 403, 404]).toContain(denied.status());
  });
});
