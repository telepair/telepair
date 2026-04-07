import { test, expect } from '@playwright/test';
import {
  login,
  gotoSession,
  waitForTerminal,
  typeInTerminal,
  waitForTerminalContent,
} from './helpers';

test.describe('Collaboration', () => {
  test('chat message appears in chat panel', async ({ page }) => {
    await gotoSession(page);
    await waitForTerminal(page);

    // Type and send a chat message
    const chatInput = page.locator('.chat-input-row input');
    await chatInput.fill('Hello from E2E!');
    await page.locator('.chat-input-row button').click();

    // The message should appear in the chat panel
    await expect(page.locator('.chat-text').first()).toHaveText('Hello from E2E!');
    await expect(page.locator('.chat-name').first()).toHaveText('admin');
  });

  test('two browser windows share terminal output', async ({ browser }) => {
    // Context A: create a session
    const ctxA = await browser.newContext();
    const pageA = await ctxA.newPage();
    const sessionId = await gotoSession(pageA);
    await waitForTerminal(pageA);

    // Context B: join the same session
    const ctxB = await browser.newContext();
    const pageB = await ctxB.newPage();
    await login(pageB);
    await pageB.goto(`/session/${sessionId}`);
    await waitForTerminal(pageB);

    // Type a command in window A
    const marker = `collab-${Date.now()}`;
    await typeInTerminal(pageA, `echo ${marker}`);

    // Both windows should see the output
    await waitForTerminalContent(pageA, marker);
    await waitForTerminalContent(pageB, marker);

    await ctxA.close();
    await ctxB.close();
  });

  test('operator in solo session sees denial toast and is blocked', async ({ browser }) => {
    // Regression for a real-machine finding: the client-side `canInput`
    // pre-filter used to silently drop operator keystrokes in solo
    // (serialized) sessions. Because the bytes never left the browser,
    // the server never replied with `InputDenied`, and the denial toast
    // never fired — so operators saw a totally dead keyboard with no
    // feedback. The fix synthesises the same toast locally when the
    // pre-filter blocks input.

    // --- Owner: create a solo-mode session via the dashboard modal ----
    const ctxA = await browser.newContext();
    const pageA = await ctxA.newPage();
    await login(pageA);
    await pageA.locator('.target-card').first().click();

    const launchDialog = pageA.getByRole('dialog', { name: 'Start a session' });
    await launchDialog.waitFor({ state: 'visible', timeout: 5_000 });
    await launchDialog.getByRole('radio', { name: /Solo/ }).click();
    await expect(launchDialog.getByRole('radio', { name: /Solo/ })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    await launchDialog.getByRole('button', { name: 'Launch' }).click();
    await pageA.waitForURL(/\/session\/.+/);
    await waitForTerminal(pageA);

    // Owner mints an operator invite for the guest to join through.
    await pageA.locator('button.action-btn', { hasText: 'Invite' }).click();
    const inviteDialog = pageA.locator('.dialog');
    await expect(inviteDialog).toBeVisible();
    await inviteDialog.locator('button.primary').click();
    const urlInput = inviteDialog.locator('.invite-url-row input');
    await expect(urlInput).toBeVisible({ timeout: 5_000 });
    const inviteUrl = await urlInput.inputValue();

    // --- Guest: clean context, redeem as operator ---------------------
    const ctxB = await browser.newContext();
    const pageB = await ctxB.newPage();
    await pageB.goto('/login');
    await pageB.evaluate(() => {
      localStorage.clear();
      sessionStorage.clear();
    });
    const joinPath = new URL(inviteUrl).pathname;
    await pageB.goto(joinPath);
    await pageB.waitForURL(/\/session\/.+/);
    await waitForTerminal(pageB);
    await expect(pageB.locator('.role-badge')).toHaveText('operator');

    // Guest types. The keystroke should be blocked AND a toast should
    // appear explaining why. This is the load-bearing assertion: before
    // the fix, only the silent drop happened.
    await typeInTerminal(pageB, 'echo GUEST_SHOULD_BE_BLOCKED');
    await expect(pageB.locator('li.toast')).toBeVisible({ timeout: 2_000 });
    await expect(pageB.locator('li.toast .toast-text')).toContainText(/Solo mode/i);

    // And the PTY must NOT have echoed the forbidden command — confirm
    // by checking the owner side, which is the PTY's only source of
    // truth. If the command had leaked through, pageA would see it.
    const ownerContent = await pageA.locator('.xterm').textContent();
    expect(ownerContent).not.toContain('GUEST_SHOULD_BE_BLOCKED');

    await ctxA.close();
    await ctxB.close();
  });
});
