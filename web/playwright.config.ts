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
  },
  webServer: {
    command: 'cargo run -- --web-dir web/dist',
    port: 7700,
    cwd: path.resolve(import.meta.dirname, '..'),
    reuseExistingServer: true,
    timeout: 120_000,
  },
});
