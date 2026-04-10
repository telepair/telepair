import { test, expect } from '@playwright/test';
import { gotoSession, waitForTerminal } from './helpers';

/**
 * E2E coverage for the Stage-4 session-history view: the Dashboard
 * grows filter chips (Active / Closed / All) and each row now carries
 * a status/reason chip + duration column. This spec walks the whole
 * lifecycle — create a session, close it from the session page, then
 * flip through the chips and verify the row surfaces in the right
 * buckets with the right label.
 *
 * We deliberately close the session from the UI rather than the REST
 * API so the test covers the same `CloseReason::Owner` path a human
 * would trigger. The two-step confirm (first click arms, second
 * actually closes) is the existing Session.tsx flow — the test drives
 * both clicks to match it.
 */
test.describe('Session history', () => {
  test('owner-closed session lands on Closed tab with correct chip', async ({ page }) => {
    const sessionId = await gotoSession(page);
    await waitForTerminal(page);

    // Close the session via the UI. First click arms; second click
    // actually hits the DELETE endpoint.
    //
    // The locator is the `.danger` variant of the action button (the
    // Close button is the only red one) rather than `hasText:/Close/`
    // — the text changes to "Click again to close" after the first
    // click, and a case-sensitive `/Close/` filter would stop matching
    // it after the flip.
    const closeBtn = page.locator('button.action-btn.danger');
    await closeBtn.click();
    await expect(closeBtn).toContainText('Click again');
    await closeBtn.click();

    // Banner confirms the close fired; from there the user can go
    // back to the dashboard via the "Back to Dashboard" button or
    // the top-bar back arrow.
    await expect(page.getByText('This session has ended.')).toBeVisible({
      timeout: 5_000,
    });
    await page.locator('.back-btn').click();
    await expect(page).toHaveURL('/');

    // The default tab is Active — the just-closed session must be
    // gone from it.
    const activeTab = page.getByRole('tab', { name: 'Active' });
    await expect(activeTab).toHaveAttribute('aria-selected', 'true');
    const row = page.locator('.session-row', { hasText: sessionId });
    await expect(row).toHaveCount(0);

    // Flip to Closed — the row should be there with the Owner chip.
    await page.getByRole('tab', { name: 'Closed' }).click();
    await expect(row).toHaveCount(1, { timeout: 5_000 });
    await expect(row).toHaveAttribute('data-status', 'closed');
    const chip = row.locator('.session-reason');
    await expect(chip).toHaveAttribute('data-reason', 'owner');
    await expect(chip).toContainText('Closed by owner');

    // Duration chip is present and non-empty — the row closed within
    // a second of creation so we're happy with any formatted value
    // (`0s`, `1s`, etc). The test asserts the element exists rather
    // than the exact string so the flaky timing ceiling doesn't bite.
    await expect(row.locator('.session-duration')).toBeVisible();

    // Closed rows now open the audit-timeline detail dialog (added in
    // Stage 5d) — they no longer race into the live session page. The
    // URL stays on the dashboard; the dialog backdrop is what appears.
    // Stage 6 started syncing the Sessions tab filter into the query
    // string (`?status=closed` here) so deep links to a specific tab
    // survive reload — that's why we match `/?status=closed` instead
    // of the bare `/`. We close the dialog so the rest of the test
    // isn't blocked by an open modal.
    await row.click();
    await expect(page).toHaveURL('/?status=closed');
    const detail = page.getByText('Session details');
    await expect(detail).toBeVisible({ timeout: 5_000 });
    await page.locator('button.detail-close').click();
    await expect(detail).toHaveCount(0);

    // Flip to All — the same row is still there. This also
    // regression-guards the store against the "All filter drops
    // closed rows" bug path.
    await page.getByRole('tab', { name: 'All' }).click();
    await expect(row).toHaveCount(1, { timeout: 5_000 });
  });
});
