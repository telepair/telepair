import { test, expect } from '@playwright/test';
import { login, getAdminToken } from './helpers';

test.describe('Authentication', () => {
  test('login with valid token redirects to dashboard', async ({ page }) => {
    await login(page);
    await expect(page.getByRole('heading', { name: 'Targets' })).toBeVisible();
  });

  test('login with invalid token shows error', async ({ page }) => {
    await page.goto('/login');
    await page.locator('#token').fill('invalid-token-12345');
    await page.locator('button[type="submit"]').click();

    await expect(page.locator('.error-msg')).toBeVisible();
  });

  test('unauthenticated user redirected to login', async ({ page }) => {
    // Ensure no token in storage
    await page.goto('/login');
    await page.evaluate(() => localStorage.clear());

    await page.goto('/');
    await expect(page).toHaveURL(/\/login/);
  });

  test('logout clears session and redirects to login', async ({ page }) => {
    await login(page);

    // Click the logout button in the topbar
    await page.locator('.topbar button').click();
    await expect(page).toHaveURL(/\/login/);

    // Revisiting dashboard should redirect back to login
    await page.goto('/');
    await expect(page).toHaveURL(/\/login/);
  });
});
