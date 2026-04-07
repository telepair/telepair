import { defineConfig } from '@playwright/test';
import path from 'path';

export default defineConfig({
  testDir: './e2e',
  timeout: 30_000,
  retries: 0,
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
    command: './target/release/telepair --web-dir web/dist',
    port: 7700,
    cwd: path.resolve(import.meta.dirname, '..'),
    reuseExistingServer: true,
    timeout: 120_000,
  },
});
