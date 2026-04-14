import { test, expect } from '@playwright/test';
import { gotoSession, login, waitForTerminal } from './helpers';

/**
 * E2E coverage for the Stage-6 admin target management page:
 * `GET /api/admin/targets` + `POST /api/admin/targets/reload`,
 * surfaced in the UI as `/admin/targets`.
 *
 * The non-admin 403 / unauthenticated 401 branches are already
 * covered by the Rust integration tests in
 * `crates/telepair-gateway/tests/admin_targets_test.rs` (both the
 * list endpoint and the reload endpoint). Driving those failure
 * branches from Playwright would need a second user seed path, and
 * the dev server only exposes the admin token from
 * `~/.telepair/admin_token`. The UI-side risk we actually care about
 * — the admin gear icon, the target card layout, the reload button,
 * and the deep-link into the filtered dashboard sessions tab — is
 * all admin-only, so the admin path covers everything the Rust
 * suite cannot.
 */
test.describe('Admin targets page', () => {
  test('admin sees the gear link in the dashboard topbar', async ({ page }) => {
    await login(page);
    // The gear link is only rendered once whoami confirms the user
    // is an admin. Since login seeds the identity cache on success,
    // the link must be visible synchronously after the dashboard
    // first paint.
    const link = page.getByTestId('admin-targets-link');
    await expect(link).toBeVisible();
    await expect(link).toHaveAttribute('href', '/admin/targets');
  });

  test('admin targets page lists targets and hot-reloads', async ({ page }) => {
    await login(page);
    await page.getByTestId('admin-targets-link').click();
    await expect(page).toHaveURL('/admin/targets');

    // The card grid must render at least the built-in `local-shell`
    // target — TargetEngine always injects it regardless of whether
    // the operator configured a yaml file. Using a raw CSS selector
    // inside the grid (not a test id) keeps the assertion aligned
    // with the real DOM the user sees.
    const grid = page.getByTestId('admin-targets-grid');
    await expect(grid).toBeVisible({ timeout: 5_000 });
    const localCard = grid.locator('.target-card[data-target-name="local-shell"]');
    await expect(localCard).toBeVisible();

    // Reload button must be present and not stuck in the reloading
    // state on mount. Clicking it either:
    //   (a) reloads the configured targets.yaml if the dev server
    //       was started with one, or
    //   (b) returns 400 `no_targets_path` if it wasn't. In a local
    //       `npm run e2e` run there's no targets.yaml path wired
    //       up — the admin.db doesn't set one — so the failure
    //       toast is what the test should see.
    // The assertion accepts either outcome: one of the two toast
    // texts must show up. That keeps this test stable regardless of
    // how the dev harness is seeded.
    const reloadBtn = page.getByTestId('admin-targets-reload-button');
    await expect(reloadBtn).toBeEnabled();
    await reloadBtn.click();

    // Toast is a global region with role="status". We match ANY of
    // the expected copy strings so either branch passes. The
    // validate-first flow surfaces "Validation failed:" when the
    // server has no targets path or the file is missing on disk;
    // the success paths show "Reloaded" or "No changes detected".
    const toast = page.locator('[role="status"]');
    await expect(toast).toContainText(
      /Reloaded \d+ targets|No changes detected|Validation failed|configure one and restart|malformed/,
      { timeout: 5_000 },
    );
  });

  test('deep link to filtered sessions tab from an active target', async ({ page }) => {
    // First make sure there's at least one active session — the
    // deep-link chip only becomes clickable when `active_sessions > 0`.
    const sessionId = await gotoSession(page);
    await waitForTerminal(page);

    // Back to the dashboard so we can pivot through the admin page.
    await page.goto('/');
    await page.getByTestId('admin-targets-link').click();
    await expect(page).toHaveURL('/admin/targets');

    // The session we just started uses `local-shell` as its target
    // (gotoSession clicks the first card, which is the built-in
    // default). The admin page must render a clickable chip on that
    // card because active_sessions is now >= 1.
    const localCard = page.locator(
      '.target-card[data-target-name="local-shell"]',
    );
    await expect(localCard).toBeVisible({ timeout: 5_000 });
    const chip = page.getByTestId('admin-targets-sessions-link-local-shell');
    await expect(chip).toHaveAttribute('data-active', 'true');
    await chip.click();

    // We should land on the dashboard with the target pre-filtered
    // AND the Active tab selected. The URL carries both query
    // params and the filter chip surfaces the target name.
    await expect(page).toHaveURL(
      /\/\?target=local-shell&status=active|\/\?status=active&target=local-shell/,
    );
    const filterChip = page.getByTestId('session-target-filter-chip');
    await expect(filterChip).toBeVisible({ timeout: 5_000 });
    await expect(filterChip).toContainText('local-shell');

    // And the session list must contain the row we just created —
    // the list is unfiltered on the session id, so we match by id.
    const row = page.locator('.session-row', { hasText: sessionId });
    await expect(row).toHaveCount(1, { timeout: 5_000 });
  });
});
