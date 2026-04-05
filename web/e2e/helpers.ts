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
  await page.locator('#token').fill(token);
  await page.locator('button[type="submit"]').click();
  await page.waitForURL('/');
}
