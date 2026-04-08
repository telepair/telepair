/**
 * Human Usability Simulation
 *
 * Walks through the full product as a first-time user would:
 * login → dashboard → create session → terminal I/O → collaboration →
 * invite flow → close session → re-enter.
 *
 * Every meaningful UX state is captured as a screenshot attached to
 * the Playwright report so the result is a visual walkthrough.
 */
import { test, expect } from '@playwright/test';
import { login, gotoSession, waitForTerminal, typeInTerminal, waitForTerminalContent } from './helpers';

// ─── helpers ─────────────────────────────────────────────────────────────────

async function snap(testInfo: any, page: any, label: string) {
  const shot = await page.screenshot({ fullPage: true });
  await testInfo.attach(label, { body: shot, contentType: 'image/png' });
}

// ─── 1. First-time visitor: redirect to login ──────────────────────────────
test('01 · unauthenticated visitor lands on login page', async ({ page }, testInfo) => {
  await page.goto('/login');
  await page.evaluate(() => {
    localStorage.clear();
    sessionStorage.clear();
  });
  await page.goto('/');
  await expect(page).toHaveURL(/\/login/);

  const heading = page.locator('h1, h2, [class*="title"], [class*="heading"]').first();
  await expect(heading).toBeVisible();
  await snap(testInfo, page, '01-login-page');
});

// ─── 2. Wrong token → error message ───────────────────────────────────────
test('02 · wrong token shows inline error', async ({ page }, testInfo) => {
  await page.goto('/login');
  await page.locator('#token').fill('definitely-wrong-token');
  await snap(testInfo, page, '02-token-filled');

  await page.locator('button[type="submit"]').click();

  const error = page.locator('.error-msg');
  await expect(error).toBeVisible({ timeout: 5_000 });
  await snap(testInfo, page, '02-error-shown');
});

// ─── 3. Correct token → dashboard ─────────────────────────────────────────
test('03 · correct token redirects to dashboard with targets', async ({ page }, testInfo) => {
  await login(page);

  await expect(page.getByRole('heading', { name: 'Targets' })).toBeVisible();
  await snap(testInfo, page, '03-dashboard-logged-in');

  // At least one target card visible
  const firstCard = page.locator('.target-card').first();
  await expect(firstCard).toBeVisible();
  const targetName = await page.locator('.target-name').first().innerText();
  expect(targetName.length).toBeGreaterThan(0);
  await snap(testInfo, page, '03-dashboard-targets');
});

// ─── 4. Create session modal ─────────────────────────────────────────────
test('04 · target card opens create-session modal', async ({ page }, testInfo) => {
  await login(page);
  await page.locator('.target-card').first().click();

  const dialog = page.getByRole('dialog', { name: 'Start a session' });
  await expect(dialog).toBeVisible({ timeout: 5_000 });
  await snap(testInfo, page, '04-create-session-modal');

  // Default is Collaborative
  await expect(dialog.getByRole('radio', { name: /Collaborative/ })).toHaveAttribute(
    'aria-checked',
    'true',
  );
  await snap(testInfo, page, '04-collaborative-default');
});

// ─── 5. Switch to Solo mode ────────────────────────────────────────────────
test('05 · user switches to Solo mode in modal', async ({ page }, testInfo) => {
  await login(page);
  await page.locator('.target-card').first().click();

  const dialog = page.getByRole('dialog', { name: 'Start a session' });
  await dialog.waitFor({ state: 'visible' });

  await dialog.getByRole('radio', { name: /Solo/ }).click();
  await expect(dialog.getByRole('radio', { name: /Solo/ })).toHaveAttribute('aria-checked', 'true');
  await snap(testInfo, page, '05-solo-mode-selected');

  // Cancel and reopen — modal state should reset
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(dialog).not.toBeVisible();
  await page.locator('.target-card').first().click();
  await dialog.waitFor({ state: 'visible' });

  // Should reset to Collaborative
  await expect(dialog.getByRole('radio', { name: /Collaborative/ })).toHaveAttribute(
    'aria-checked',
    'true',
  );
  await snap(testInfo, page, '05-modal-reset-to-collaborative');
});

// ─── 6. Launch session and verify terminal ─────────────────────────────────
test('06 · launch session → terminal connects', async ({ page }, testInfo) => {
  await gotoSession(page);
  await snap(testInfo, page, '06-session-page-loading');

  await waitForTerminal(page);
  await snap(testInfo, page, '06-terminal-connected');

  await expect(page.locator('.role-badge')).toHaveText('Owner');
  await expect(page.locator('.role-badge')).toHaveAttribute('data-role', 'owner');
  await snap(testInfo, page, '06-role-badge-owner');
});

// ─── 7. Participants panel ─────────────────────────────────────────────────
test('07 · participants panel shows admin as owner', async ({ page }, testInfo) => {
  await gotoSession(page);
  await waitForTerminal(page);

  const participantRow = page.locator('.participant-row');
  await expect(participantRow.first()).toBeVisible({ timeout: 10_000 });
  await expect(page.locator('.participant-name').first()).toHaveText('admin');
  await expect(page.locator('.participant-role').first()).toHaveText('Owner');
  await snap(testInfo, page, '07-participants-panel');
});

// ─── 8. Sidebar toggle ─────────────────────────────────────────────────────
test('08 · sidebar toggles on / off', async ({ page }, testInfo) => {
  await gotoSession(page);
  await waitForTerminal(page);

  await expect(page.locator('.sidebar')).toBeVisible();
  await snap(testInfo, page, '08-sidebar-visible');

  await page.locator('button.action-btn', { hasText: /Sidebar/ }).click();
  await expect(page.locator('.sidebar')).not.toBeVisible();
  await snap(testInfo, page, '08-sidebar-hidden');

  await page.locator('button.action-btn', { hasText: /Sidebar/ }).click();
  await expect(page.locator('.sidebar')).toBeVisible();
  await snap(testInfo, page, '08-sidebar-shown-again');
});

// ─── 9. Terminal input → output ────────────────────────────────────────────
test('09 · terminal: type multiple commands and see output', async ({ page }, testInfo) => {
  await gotoSession(page);
  await waitForTerminal(page);
  await snap(testInfo, page, '09-terminal-ready');

  // echo
  await typeInTerminal(page, 'echo "Hello Telepair"');
  await waitForTerminalContent(page, 'Hello Telepair');
  await snap(testInfo, page, '09-echo-output');

  // pwd
  await typeInTerminal(page, 'pwd');
  await waitForTerminalContent(page, '/');
  await snap(testInfo, page, '09-pwd-output');

  // whoami
  await typeInTerminal(page, 'whoami');
  await page.waitForTimeout(500);
  await snap(testInfo, page, '09-whoami-output');

  // ls with colours
  await typeInTerminal(page, 'ls -la --color=never');
  await page.waitForTimeout(800);
  await snap(testInfo, page, '09-ls-output');
});

// ─── 10. Chat collaboration ────────────────────────────────────────────────
test('10 · chat message sent and displayed', async ({ page }, testInfo) => {
  await gotoSession(page);
  await waitForTerminal(page);

  const chatInput = page.locator('.chat-input-row input');
  await chatInput.fill('Hello from usability test!');
  await snap(testInfo, page, '10-chat-typed');

  await page.locator('.chat-input-row button').click();

  await expect(page.locator('.chat-text').first()).toHaveText('Hello from usability test!');
  await expect(page.locator('.chat-name').first()).toHaveText('admin');
  await snap(testInfo, page, '10-chat-message-sent');
});

// ─── 11. Two windows share terminal ────────────────────────────────────────
test('11 · two users share terminal in real time', async ({ browser }, testInfo) => {
  const ctxA = await browser.newContext();
  const pageA = await ctxA.newPage();
  const sessionId = await gotoSession(pageA);
  await waitForTerminal(pageA);
  await snap(testInfo, pageA, '11-owner-connected');

  const ctxB = await browser.newContext();
  const pageB = await ctxB.newPage();
  await (await import('./helpers')).login(pageB);
  await pageB.goto(`/session/${sessionId}`);
  await waitForTerminal(pageB);
  await snap(testInfo, pageA, '11-both-connected-ownerView');
  await snap(testInfo, pageB, '11-both-connected-guestView');

  const marker = `shared-${Date.now()}`;
  await typeInTerminal(pageA, `echo ${marker}`);

  await waitForTerminalContent(pageA, marker);
  await waitForTerminalContent(pageB, marker);
  await snap(testInfo, pageA, '11-output-synced-owner');
  await snap(testInfo, pageB, '11-output-synced-guest');

  await ctxA.close();
  await ctxB.close();
});

// ─── 12. Invite flow: anonymous guest joins ────────────────────────────────
test('12 · anonymous guest joins via invite link', async ({ browser }, testInfo) => {
  const ownerCtx = await browser.newContext();
  const ownerPage = await ownerCtx.newPage();
  await gotoSession(ownerPage);
  await waitForTerminal(ownerPage);

  // Generate invite
  await ownerPage.locator('button.action-btn', { hasText: 'Invite' }).click();
  const dialog = ownerPage.locator('.dialog');
  await expect(dialog).toBeVisible();
  await snap(testInfo, ownerPage, '12-invite-dialog-open');

  await dialog.locator('button.primary').click();
  const urlInput = dialog.locator('.invite-url-row input');
  await expect(urlInput).toBeVisible({ timeout: 5_000 });
  const inviteUrl = await urlInput.inputValue();
  await snap(testInfo, ownerPage, '12-invite-link-generated');

  // Guest: fresh context, no token
  const guestCtx = await browser.newContext();
  const guestPage = await guestCtx.newPage();
  await guestPage.goto('/login');
  await guestPage.evaluate(() => {
    localStorage.clear();
    sessionStorage.clear();
  });

  const joinPath = new URL(inviteUrl).pathname;
  await guestPage.goto(joinPath);
  await guestPage.waitForURL(/\/session\/.+/, { timeout: 10_000 });
  await waitForTerminal(guestPage);
  await snap(testInfo, guestPage, '12-guest-in-session');

  // Owner sees 2 participants
  const ownerRows = ownerPage.locator('.participant-row');
  await expect(ownerRows).toHaveCount(2, { timeout: 10_000 });
  await snap(testInfo, ownerPage, '12-two-participants-visible');

  await ownerCtx.close();
  await guestCtx.close();
});

// ─── 13. Invalid invite link ───────────────────────────────────────────────
test('13 · invalid invite link shows error, not login wall', async ({ browser }, testInfo) => {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();

  await page.goto('/login');
  await page.evaluate(() => {
    localStorage.clear();
    sessionStorage.clear();
  });

  await page.goto('/join/this-token-does-not-exist');
  await expect(page.locator('.error-msg')).toBeVisible({ timeout: 5_000 });
  await expect(page.locator('.error-msg')).toContainText(/invalid|expired/i);
  expect(page.url()).toContain('/join/');
  await snap(testInfo, page, '13-invalid-invite-error');

  await ctx.close();
});

// ─── 14. Solo mode blocks operator input with toast ───────────────────────
test('14 · solo mode: operator input blocked with feedback toast', async ({ browser }, testInfo) => {
  const ctxA = await browser.newContext();
  const pageA = await ctxA.newPage();
  await login(pageA);
  await pageA.locator('.target-card').first().click();

  const launchDialog = pageA.getByRole('dialog', { name: 'Start a session' });
  await launchDialog.waitFor({ state: 'visible' });
  await launchDialog.getByRole('radio', { name: /Solo/ }).click();
  await launchDialog.getByRole('button', { name: 'Launch' }).click();
  await pageA.waitForURL(/\/session\/.+/);
  await waitForTerminal(pageA);
  await snap(testInfo, pageA, '14-solo-session-owner');

  // Mint operator invite
  await pageA.locator('button.action-btn', { hasText: 'Invite' }).click();
  const inviteDialog = pageA.locator('.dialog');
  await inviteDialog.locator('button.primary').click();
  const urlInput = inviteDialog.locator('.invite-url-row input');
  await expect(urlInput).toBeVisible({ timeout: 5_000 });
  const inviteUrl = await urlInput.inputValue();

  const ctxB = await browser.newContext();
  const pageB = await ctxB.newPage();
  await pageB.goto('/login');
  await pageB.evaluate(() => { localStorage.clear(); sessionStorage.clear(); });
  await pageB.goto(new URL(inviteUrl).pathname);
  await pageB.waitForURL(/\/session\/.+/);
  await waitForTerminal(pageB);
  await expect(pageB.locator('.role-badge')).toHaveText('Operator');
  await snap(testInfo, pageB, '14-guest-operator-view');

  // Operator types → should be blocked with toast
  await typeInTerminal(pageB, 'echo SHOULD_NOT_EXECUTE');
  await expect(pageB.locator('li.toast')).toBeVisible({ timeout: 2_000 });
  await expect(pageB.locator('li.toast .toast-text')).toContainText(/Solo mode/i);
  await snap(testInfo, pageB, '14-solo-block-toast');

  await ctxA.close();
  await ctxB.close();
});

// ─── 15. Owner closes session ─────────────────────────────────────────────
test('15 · owner closes session via topbar', async ({ page }, testInfo) => {
  await gotoSession(page);
  await waitForTerminal(page);
  await snap(testInfo, page, '15-before-close');

  // Step 1: first click arms the button (label: "Close")
  const closeBtn = page.locator('button.action-btn.danger').first();
  await expect(closeBtn).toBeVisible({ timeout: 5_000 });
  await closeBtn.click();
  await snap(testInfo, page, '15-close-armed');

  // Step 2: button now reads "Click again to close" — click to confirm
  await expect(closeBtn).toHaveText('Click again to close', { timeout: 3_000 });
  await closeBtn.click();
  await snap(testInfo, page, '15-close-confirmed');

  // Server broadcasts SESSION_CLOSED → Banner div appears
  // "This session has ended." — page stays on /session/{id}, no redirect
  const endedBanner = page.locator('.banner').filter({ hasText: /This session has ended/i });
  await expect(endedBanner).toBeVisible({ timeout: 5_000 });
  await snap(testInfo, page, '15-ended-banner-visible');
});

// ─── 16. Back button returns to dashboard ────────────────────────────────
test('16 · back button navigates to dashboard', async ({ page }, testInfo) => {
  await gotoSession(page);
  await waitForTerminal(page);
  await snap(testInfo, page, '16-in-session');

  await page.locator('.back-btn').click();
  await expect(page).toHaveURL('/');
  await expect(page.getByRole('heading', { name: 'Targets' })).toBeVisible();
  await snap(testInfo, page, '16-back-on-dashboard');
});

// ─── 17. Logout clears auth ────────────────────────────────────────────────
test('17 · logout clears token and redirects to login', async ({ page }, testInfo) => {
  await login(page);
  await snap(testInfo, page, '17-logged-in');

  await page.getByRole('button', { name: 'Logout' }).click();
  await expect(page).toHaveURL(/\/login/);
  await snap(testInfo, page, '17-after-logout');

  // Token must be gone — revisit dashboard → redirected
  await page.goto('/');
  await expect(page).toHaveURL(/\/login/);
  await snap(testInfo, page, '17-redirect-after-logout');
});
