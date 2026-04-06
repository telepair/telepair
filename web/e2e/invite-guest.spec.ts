import { test, expect } from '@playwright/test';
import { gotoSession, waitForTerminal } from './helpers';

/**
 * E2E regression for the "invite guests without pre-auth" flow.
 *
 * Before the fix: Join.tsx bounced unauthenticated visitors to
 * /login, which made every invite link unusable for anyone who
 * hadn't already been handed an admin token. The whole point of
 * "share a link to collaborate" was broken.
 *
 * After the fix: the backend's /invite/redeem endpoint accepts
 * anonymous callers, mints a guest user, and returns a fresh token.
 * Join.tsx stores that token and drops the visitor straight into
 * the session.
 */
test.describe('Invite flow (anonymous guest)', () => {
  test('anonymous visitor joins via invite link and lands in the session', async ({ browser }) => {
    // --- Owner side: create a session and an invite link ------------------
    const ownerCtx = await browser.newContext();
    const ownerPage = await ownerCtx.newPage();

    await gotoSession(ownerPage);
    await waitForTerminal(ownerPage);

    await ownerPage.locator('button.action-btn', { hasText: 'Invite' }).click();
    const dialog = ownerPage.locator('.dialog');
    await expect(dialog).toBeVisible();
    await dialog.locator('button.primary').click();

    const urlInput = dialog.locator('.invite-url-row input');
    await expect(urlInput).toBeVisible({ timeout: 5_000 });
    const inviteUrl = await urlInput.inputValue();
    expect(inviteUrl).toContain('/join/');

    // --- Guest side: fresh context, NO token, open the link --------------
    // A clean incognito-style context guarantees there is no cached
    // admin token in localStorage — the scenario we care about is a
    // brand-new visitor.
    const guestCtx = await browser.newContext();
    const guestPage = await guestCtx.newPage();

    // Sanity: the guest really starts with nothing.
    await guestPage.goto('/login');
    await guestPage.evaluate(() => localStorage.clear());

    // Extract just the path+token part so Playwright stays on the
    // test baseURL instead of hitting whatever host the backend
    // printed into the invite.
    const joinPath = new URL(inviteUrl).pathname;
    await guestPage.goto(joinPath);

    // Expected outcome: the redeem handler runs, stores the freshly
    // minted guest token, and replaces the current history entry
    // with the session URL.
    await guestPage.waitForURL(/\/session\/.+/, { timeout: 10_000 });

    // We should NOT be bounced back to /login (the old bug).
    expect(guestPage.url()).not.toContain('/login');

    // The session page must render its terminal for the guest too.
    await waitForTerminal(guestPage);

    // And the backend must have handed us a token — otherwise a page
    // reload would snap us back to /login.
    const storedToken = await guestPage.evaluate(() =>
      localStorage.getItem('telepair_token'),
    );
    expect(storedToken).toBeTruthy();
    expect(storedToken!.length).toBeGreaterThan(16);

    // --- Owner sees the new peer ----------------------------------------
    // Two participant rows: owner (admin) + the freshly minted guest.
    const ownerParticipantRows = ownerPage.locator('.participant-row');
    await expect(ownerParticipantRows).toHaveCount(2, { timeout: 10_000 });

    await ownerCtx.close();
    await guestCtx.close();
  });

  test('visiting an invalid invite link shows an error, not a login wall', async ({ browser }) => {
    // Anonymous visitor hits a bogus token. The page should surface
    // the problem in-place rather than kicking them to /login (which
    // would have been the old "pending_invite" behaviour).
    const ctx = await browser.newContext();
    const page = await ctx.newPage();

    await page.goto('/login');
    await page.evaluate(() => localStorage.clear());

    await page.goto('/join/definitely-not-a-real-token');

    // Error message appears on the join page itself.
    await expect(page.locator('.error-msg')).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('.error-msg')).toContainText(/invalid|expired/i);

    // We stayed on /join, not bounced to /login.
    expect(page.url()).toContain('/join/');

    await ctx.close();
  });
});
