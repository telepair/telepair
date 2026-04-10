import { test, expect } from '@playwright/test';
import { gotoSession, waitForTerminal } from './helpers';

/**
 * E2E coverage for the invite management dialog — list existing
 * invites and revoke them.
 *
 * The "create invite" flow is already exercised by
 * `invite-guest.spec.ts`; this spec focuses on the new management
 * surface: seeing the invites you've already minted and pulling one
 * of them out of circulation without needing to shut down the whole
 * session.
 *
 * The dialog currently closes entirely after a successful create (it
 * returns to the dashboard), so "mint two invites then revoke one"
 * means opening the dialog, creating, closing, re-opening, creating,
 * closing, re-opening — each reopen refreshes the management list
 * from the backend. That's the flow these tests drive.
 */
test.describe('Invite management (list + revoke)', () => {
  /** Open the invite dialog and return its locator. */
  async function openInviteDialog(page: import('@playwright/test').Page) {
    await page.locator('button.action-btn', { hasText: 'Invite' }).click();
    const dialog = page.locator('.dialog');
    await expect(dialog).toBeVisible();
    return dialog;
  }

  test('owner sees each minted invite and can revoke one', async ({ page }) => {
    await gotoSession(page);
    await waitForTerminal(page);

    // First mint ---------------------------------------------------
    let dialog = await openInviteDialog(page);
    // Empty state — no invites yet.
    await expect(dialog.locator('[data-testid="invite-row"]')).toHaveCount(0);

    await dialog.locator('button.primary').click();
    await expect(dialog.locator('.invite-url-row input')).toBeVisible({
      timeout: 5_000,
    });
    await dialog.getByRole('button', { name: 'Done' }).click();
    await expect(dialog).toBeHidden();

    // Second mint --------------------------------------------------
    dialog = await openInviteDialog(page);
    // The reload-on-open fetches the freshly-minted row.
    await expect(dialog.locator('[data-testid="invite-row"]')).toHaveCount(1, {
      timeout: 5_000,
    });

    await dialog.locator('button.primary').click();
    await expect(dialog.locator('.invite-url-row input')).toBeVisible({
      timeout: 5_000,
    });
    await dialog.getByRole('button', { name: 'Done' }).click();
    await expect(dialog).toBeHidden();

    // Revoke ------------------------------------------------------
    dialog = await openInviteDialog(page);
    const rows = dialog.locator('[data-testid="invite-row"]');
    await expect(rows).toHaveCount(2, { timeout: 5_000 });

    // Each row exposes an 8-char token prefix — the owner's stable
    // per-row label.
    const firstPrefix = await dialog
      .locator('[data-testid="invite-prefix"]')
      .first()
      .textContent();
    expect(firstPrefix?.trim()).toHaveLength(8);

    // Two-step revoke on the first row. Pre-confirm → confirm.
    const firstRow = rows.first();
    await firstRow.locator('[data-testid="invite-revoke"]').click();
    await firstRow.locator('[data-testid="invite-revoke-confirm"]').click();

    // Optimistic drop + refresh should converge to one row.
    await expect(rows).toHaveCount(1, { timeout: 5_000 });

    // Durability: close and re-open the dialog; the revoke must
    // survive the round-trip.
    await page.locator('.dialog-backdrop').click({ position: { x: 5, y: 5 } });
    await expect(dialog).toBeHidden();

    dialog = await openInviteDialog(page);
    await expect(dialog.locator('[data-testid="invite-row"]')).toHaveCount(1, {
      timeout: 5_000,
    });

    // Pending-revoke reset: click "Revoke" on the remaining row to
    // enter the confirm-state, then close the dialog WITHOUT
    // confirming. Re-opening must drop the pending state — the
    // row comes back showing "Revoke", not "Confirm revoke?". A
    // half-started confirmation leaking across closes was the
    // v0.1.1 bug this assertion pins.
    const remaining = dialog.locator('[data-testid="invite-row"]').first();
    await remaining.locator('[data-testid="invite-revoke"]').click();
    await expect(
      remaining.locator('[data-testid="invite-revoke-confirm"]'),
    ).toBeVisible();
    await page.locator('.dialog-backdrop').click({ position: { x: 5, y: 5 } });
    await expect(dialog).toBeHidden();

    dialog = await openInviteDialog(page);
    const reopened = dialog.locator('[data-testid="invite-row"]').first();
    await expect(reopened).toBeVisible({ timeout: 5_000 });
    // The confirm button must NOT be visible — the pending token
    // from before the close should have been cleared.
    await expect(
      reopened.locator('[data-testid="invite-revoke-confirm"]'),
    ).toHaveCount(0);
    // The normal "Revoke" entry button is back in the default state.
    await expect(reopened.locator('[data-testid="invite-revoke"]')).toBeVisible();
  });

  test('revoked invite link no longer redeems', async ({ browser }) => {
    // Owner side: create a session, mint an invite, note the link,
    // then revoke it through the management dialog.
    const ownerCtx = await browser.newContext();
    const ownerPage = await ownerCtx.newPage();

    await gotoSession(ownerPage);
    await waitForTerminal(ownerPage);

    // Mint an invite and capture the share URL.
    let dialog = await openInviteDialog(ownerPage);
    await dialog.locator('button.primary').click();
    const urlInput = dialog.locator('.invite-url-row input');
    await expect(urlInput).toBeVisible({ timeout: 5_000 });
    const inviteUrl = await urlInput.inputValue();
    await dialog.getByRole('button', { name: 'Done' }).click();
    await expect(dialog).toBeHidden();

    // Re-open and revoke the one row that exists.
    dialog = await openInviteDialog(ownerPage);
    const rows = dialog.locator('[data-testid="invite-row"]');
    await expect(rows).toHaveCount(1, { timeout: 5_000 });
    await rows.first().locator('[data-testid="invite-revoke"]').click();
    await rows.first().locator('[data-testid="invite-revoke-confirm"]').click();
    await expect(rows).toHaveCount(0, { timeout: 5_000 });

    // Guest side: try the revoked link. The redeem handler must now
    // reject it — the join page surfaces the failure in-place.
    const guestCtx = await browser.newContext();
    const guestPage = await guestCtx.newPage();
    await guestPage.goto('/login');
    await guestPage.evaluate(() => {
      localStorage.clear();
      sessionStorage.clear();
    });

    const joinPath = new URL(inviteUrl).pathname;
    await guestPage.goto(joinPath);

    // The join page surfaces the error in-place; we should stay on
    // /join and see the error message rather than being handed a
    // working session.
    await expect(guestPage.locator('.error-msg')).toBeVisible({ timeout: 5_000 });
    expect(guestPage.url()).toContain('/join/');

    await ownerCtx.close();
    await guestCtx.close();
  });
});
