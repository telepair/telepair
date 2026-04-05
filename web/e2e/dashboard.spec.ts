import { test, expect } from '@playwright/test';
import { login } from './helpers';

test.describe('Dashboard', () => {
  test.beforeEach(async ({ page }) => {
    await login(page);
  });

  test('shows available targets', async ({ page }) => {
    // At least the default "local-shell" target should be visible
    const targetCards = page.locator('.target-card');
    await expect(targetCards.first()).toBeVisible();
    await expect(page.locator('.target-name').first()).not.toBeEmpty();
  });

  test('clicking a target creates a session and navigates', async ({ page }) => {
    // Click the first target card
    await page.locator('.target-card').first().click();

    // Should navigate to /session/{uuid}
    await page.waitForURL(/\/session\/.+/);
    await expect(page.locator('.session-page')).toBeVisible();
  });
});
