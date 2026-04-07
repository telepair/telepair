import { test, expect } from '@playwright/test';
import { gotoSession } from './helpers';

test.describe('Session page', () => {
  test('shows terminal and connected status', async ({ page }) => {
    await gotoSession(page);

    // Terminal container renders xterm
    await expect(page.locator('.terminal-container')).toBeVisible();
    await expect(page.locator('.xterm')).toBeVisible({ timeout: 10_000 });

    // Status dot shows connected
    await expect(page.locator('.status-dot[data-status="connected"]')).toBeVisible({
      timeout: 10_000,
    });
  });

  test('displays owner role badge', async ({ page }) => {
    await gotoSession(page);

    const badge = page.locator('.role-badge');
    await expect(badge).toHaveText('Owner');
    await expect(badge).toHaveAttribute('data-role', 'owner');
  });

  test('sidebar toggles on and off', async ({ page }) => {
    await gotoSession(page);

    // Sidebar should be visible by default
    await expect(page.locator('.sidebar')).toBeVisible();

    // Click "Hide Sidebar"
    await page.locator('button.action-btn', { hasText: /Sidebar/ }).click();
    await expect(page.locator('.sidebar')).not.toBeVisible();

    // Click "Show Sidebar"
    await page.locator('button.action-btn', { hasText: /Sidebar/ }).click();
    await expect(page.locator('.sidebar')).toBeVisible();
  });

  test('invite dialog opens and creates link', async ({ page }) => {
    await gotoSession(page);

    // Click "Invite" button (only visible to owner)
    await page.locator('button.action-btn', { hasText: 'Invite' }).click();

    // Dialog appears
    const dialog = page.locator('.dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.locator('h3')).toHaveText('Invite to Session');

    // Default role is operator
    await expect(dialog.locator('.role-btn.active')).toContainText('Operator');

    // Create invite link
    await dialog.locator('button.primary').click();

    // Invite URL should appear
    const urlInput = dialog.locator('.invite-url-row input');
    await expect(urlInput).toBeVisible({ timeout: 5_000 });
    const url = await urlInput.inputValue();
    expect(url).toContain('/join/');
  });

  test('back button returns to dashboard', async ({ page }) => {
    await gotoSession(page);

    await page.locator('.back-btn').click();
    await expect(page).toHaveURL('/');
    await expect(page.getByRole('heading', { name: 'Targets' })).toBeVisible();
  });

  test('participants list shows current user', async ({ page }) => {
    await gotoSession(page);

    // Wait for WS to connect and populate participants
    const participantRow = page.locator('.participant-row');
    await expect(participantRow.first()).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.participant-name').first()).toHaveText('admin');
    await expect(page.locator('.participant-role').first()).toHaveText('Owner');
  });
});
