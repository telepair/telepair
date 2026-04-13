import { readFileSync } from 'fs';
import { join } from 'path';
import { homedir } from 'os';
import type { Page } from '@playwright/test';

const TOKEN_PATH = join(
  process.env.TELEPAIR_TEST_HOME || homedir(),
  '.telepair',
  'admin_token',
);

export function getAdminToken(): string {
  return readFileSync(TOKEN_PATH, 'utf-8').trim();
}

/** Log in via the UI and wait for dashboard. */
export async function login(page: Page): Promise<void> {
  const token = getAdminToken();
  await page.goto('/login');
  // Switch to the token tab (the login page defaults to email mode).
  await page.getByRole('tab', { name: /admin token/i }).click();
  await page.locator('#token').fill(token);
  await page.locator('button[type="submit"]').click();
  await page.waitForURL('/');
}

/** Login, create a session via target card, return the session ID.
 *
 * Clicking a target card no longer navigates directly — it opens the
 * `CreateSessionDialog` modal (confirmation + input-mode picker) and
 * the user has to click "Launch" to actually start the session. The
 * E2E helper drives that flow end-to-end so every test keeps working
 * against the new dashboard UX.
 */
export async function gotoSession(page: Page): Promise<string> {
  await login(page);
  // Click the first *global* target card (button.target-card). User-owned
  // targets use a different wrapper structure (.user-target-card > .target-card-body).
  await page.locator('button.target-card').first().click();
  // Wait for the modal to appear, then click Launch. Targeting by
  // role="dialog" + accessible name is more robust than a CSS class.
  const dialog = page.getByRole('dialog', { name: 'Start a session' });
  await dialog.waitFor({ state: 'visible', timeout: 5_000 });
  await dialog.getByRole('button', { name: 'Launch' }).click();
  await page.waitForURL(/\/session\/.+/);
  return page.url().split('/session/')[1];
}

/** Wait until the terminal is rendered and WebSocket reports connected. */
export async function waitForTerminal(page: Page): Promise<void> {
  await page.locator('.xterm').waitFor({ state: 'visible', timeout: 10_000 });
  await page.locator('.status-dot[data-status="connected"]').waitFor({ state: 'visible', timeout: 10_000 });
}

/** Read all non-empty lines from the xterm buffer via the __xterm test hook. */
export async function getTerminalContent(page: Page): Promise<string> {
  return page.evaluate(() => {
    const container = document.querySelector('.terminal-container > div');
    const term = (container as any)?.__xterm;
    if (!term) return '';
    const buffer = term.buffer.active;
    const lines: string[] = [];
    for (let i = 0; i < buffer.length; i++) {
      const line = buffer.getLine(i);
      if (line) lines.push(line.translateToString(true));
    }
    while (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();
    return lines.join('\n');
  });
}

/** Click on the terminal to focus it, type a command, then press Enter. */
export async function typeInTerminal(page: Page, command: string): Promise<void> {
  await page.locator('.xterm').click();
  await page.keyboard.type(command, { delay: 30 });
  await page.keyboard.press('Enter');
}

/** Poll the terminal buffer until it contains the expected text. */
export async function waitForTerminalContent(
  page: Page,
  expected: string,
  timeout = 10_000,
): Promise<string> {
  const deadline = Date.now() + timeout;
  let content = '';
  while (Date.now() < deadline) {
    content = await getTerminalContent(page);
    if (content.includes(expected)) return content;
    await page.waitForTimeout(200);
  }
  throw new Error(
    `Terminal did not contain "${expected}" within ${timeout}ms.\nGot:\n${content}`,
  );
}
