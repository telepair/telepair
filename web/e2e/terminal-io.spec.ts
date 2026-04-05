import { test } from '@playwright/test';
import {
  gotoSession,
  waitForTerminal,
  typeInTerminal,
  waitForTerminalContent,
} from './helpers';

test.describe('Terminal I/O', () => {
  test('typed command produces visible output', async ({ page }) => {
    await gotoSession(page);
    await waitForTerminal(page);

    await typeInTerminal(page, 'echo hello-from-e2e');
    await waitForTerminalContent(page, 'hello-from-e2e');
  });

  test('pwd displays a filesystem path', async ({ page }) => {
    await gotoSession(page);
    await waitForTerminal(page);

    await typeInTerminal(page, 'pwd');
    // The output should contain an absolute path (starts with /)
    await waitForTerminalContent(page, '/');
  });
});
