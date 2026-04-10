import { test, expect } from '@playwright/test';
import { gotoSession, waitForTerminal } from './helpers';

/**
 * E2E coverage for the Stage-5d session-detail audit dialog.
 *
 * The user-visible promise is "every closed session in the history view
 * has a clickable detail surface that shows what happened on it". The
 * underlying API is `GET /api/sessions/{id}/audit`, which the dialog
 * calls and renders newest-first. This spec exercises the full
 * round-trip: create a real session, close it via the UI, click the
 * row in the Closed tab, and verify the timeline carries at least the
 * three rows we know the backend writes for the create + close flow:
 * `session.created`, `participant.joined`, and `session.closed`.
 *
 * Why drive this through the UI rather than hitting the endpoint
 * directly: the spec needs to guard against a regression in *any* layer
 * of the dialog wiring — Dashboard.tsx click handler, the resource
 * fetch, the `eventLabel` lookup, and the i18n keys — and a unit test
 * on the component alone wouldn't catch a broken Dashboard hook-up.
 */
test.describe('Session detail audit timeline', () => {
  test('owner-closed session shows the create + close events', async ({ page }) => {
    const sessionId = await gotoSession(page);
    await waitForTerminal(page);

    // Close from the UI so the test exercises the same `Owner` close
    // path a real human triggers — this is what stamps `session.closed`
    // into the audit table with the matching reason.
    const closeBtn = page.locator('button.action-btn.danger');
    await closeBtn.click();
    await closeBtn.click();
    await expect(page.getByText('This session has ended.')).toBeVisible({
      timeout: 5_000,
    });

    // Back to dashboard, flip to the Closed tab, find the row.
    await page.locator('.back-btn').click();
    await expect(page).toHaveURL('/');
    await page.getByRole('tab', { name: 'Closed' }).click();
    const row = page.locator('.session-row', { hasText: sessionId });
    await expect(row).toHaveCount(1, { timeout: 5_000 });

    // Open the detail dialog.
    await row.click();
    const dialog = page.locator('.session-detail-dialog');
    await expect(dialog).toBeVisible({ timeout: 5_000 });

    // The session id and target name are surfaced in the meta block
    // so the user can confirm at a glance which row they're looking at.
    await expect(dialog.locator('.detail-id')).toHaveText(sessionId);

    // Timeline must have rendered something — the resource fetch is
    // backed by a live HTTP round-trip and the create flow guarantees
    // at least these three events. The strict label match also doubles
    // as an i18n smoke test: a missing dictionary key would surface
    // here as the raw `session.created` string instead of the English
    // copy below.
    const timeline = page.locator('[data-testid="session-detail-timeline"]');
    await expect(timeline).toBeVisible();
    await expect(timeline.locator('.timeline-row')).toHaveCount(3, {
      timeout: 5_000,
    });
    await expect(timeline.getByText('Session created')).toBeVisible();
    await expect(timeline.getByText('Participant joined')).toBeVisible();
    await expect(timeline.getByText('Session closed')).toBeVisible();

    // The close row carries a `{reason: 'owner'}` detail blob — the
    // best-effort summary should surface it inline before the user
    // even opens the JSON drawer.
    const closeRow = timeline.locator(
      '.timeline-row[data-event-type="session.closed"]',
    );
    await expect(closeRow.locator('.timeline-summary')).toContainText('owner');

    // JSON drawer is opt-in. Click the toggle on the close row and
    // assert the raw payload landed.
    await closeRow.locator('.timeline-detail-toggle').click();
    await expect(closeRow.locator('.timeline-detail-json')).toBeVisible();
    await expect(closeRow.locator('.timeline-detail-json')).toContainText('owner');

    // Closing the dialog clears it from the DOM (Solid `Show` unmounts
    // when the signal flips back to null) — assert the modal is gone
    // so the test doesn't pass with a stuck overlay.
    await dialog.locator('.detail-close').click();
    await expect(dialog).toHaveCount(0);
  });
});
