import { defineConfig } from '@playwright/test';
import path from 'path';
import { E2E_DATA_DIR } from './e2e/data-dir';

export default defineConfig({
  testDir: './e2e',
  timeout: 30_000,
  retries: process.env.CI ? 1 : 0,
  workers: 1, // serial — tests share server state
  use: {
    baseURL: 'http://localhost:7700',
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
    // Pin every browser context to English so the i18n provider's
    // navigator.language detector resolves to 'en' regardless of the
    // host machine's locale. All existing assertions use English copy.
    locale: 'en-US',
  },
  webServer: {
    // Reuse the release binary produced by `make build-rust` (also the
    // artifact that `make all` / CI build earlier in the pipeline), so a
    // full run doesn't pay for a second cargo compile in the dev profile.
    // Standalone `npm run e2e` invocations therefore expect the release
    // binary to exist — `make e2e` handles that via its build-rust dep.
    //
    // Wrap the binary in a shell so we can wipe the dedicated data dir
    // *before* the server boots — it must be empty when telepair
    // generates the admin token, otherwise tests inherit stale state
    // from the previous run (or, worse, from manual QA via the user's
    // real `~/.telepair`). This wipe deliberately runs as part of the
    // server command rather than `globalSetup`, because Playwright
    // launches the webServer first and only then runs globalSetup —
    // the reverse order would delete the token immediately after the
    // server wrote it.
    command: `rm -rf '${E2E_DATA_DIR}' && ./target/release/telepair --web-dir web/dist`,
    port: 7700,
    cwd: path.resolve(import.meta.dirname, '..'),
    // Always start a fresh server so the data dir wipe above actually
    // applies. Reusing a stale server (possibly bound to the user's
    // real `~/.telepair`) would silently undo the isolation.
    reuseExistingServer: false,
    timeout: 120_000,
    env: {
      // `webServer.env` REPLACES the parent env rather than extending
      // it, so spread `process.env` first or the binary will spawn
      // without PATH/HOME and silently fail before binding the port.
      ...(process.env as Record<string, string>),
      TELEPAIR_DATA_DIR: E2E_DATA_DIR,
    },
  },
});
