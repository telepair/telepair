import { defineConfig } from 'vite';
import solid from 'vite-plugin-solid';

export default defineConfig({
  plugins: [solid()],
  test: {
    environment: 'jsdom',
    exclude: ['e2e/**', 'node_modules/**'],
    // Locks the i18n locale to `en` before any test imports the
    // provider. See `src/test-setup.ts` for the rationale — without
    // this, a developer with `navigator.language === 'zh-CN'` would
    // see every UI assertion fail against Chinese strings.
    setupFiles: ['./src/test-setup.ts'],
  },
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://localhost:7700',
      '/ws': {
        target: 'ws://localhost:7700',
        ws: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    target: 'esnext',
  },
});
