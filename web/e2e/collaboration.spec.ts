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
});
