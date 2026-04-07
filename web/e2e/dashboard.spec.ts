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

  test('clicking a target opens the create-session modal and launches', async ({ page }) => {
    // Click the first target card — should now open the confirmation
    // modal instead of navigating immediately. Finding #5 fix: users
    // get a chance to pick Solo vs Collaborative before the PTY spawns.
    await page.locator('.target-card').first().click();

    const dialog = page.getByRole('dialog', { name: 'Start a session' });
    await expect(dialog).toBeVisible();

    // Default mode is Collaborative (multiplexed), matching finding #3
    // fix. Cancel + re-open to verify the modal state resets on each
    // open, then launch for real.
    await expect(dialog.getByRole('radio', { name: /Collaborative/ })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).not.toBeVisible();

    // Re-open and launch
    await page.locator('.target-card').first().click();
    await dialog.waitFor({ state: 'visible' });
    await dialog.getByRole('button', { name: 'Launch' }).click();

    // Should navigate to /session/{uuid}
    await page.waitForURL(/\/session\/.+/);
    await expect(page.locator('.session-page')).toBeVisible();
  });
});
