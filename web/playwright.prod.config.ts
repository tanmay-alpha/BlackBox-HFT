import { defineConfig, devices } from '@playwright/test';

import { WEB_URL } from './e2e-prod/env';

// Read-only, anonymous acceptance against the deployed product (Vercel UI + Render API).
// No webServer; generous timeouts cover a Render cold start. `npm run test:prod`.
// `PROD_WEB_URL` / `PROD_API_URL` (origins) run the same specs against another stack, e.g. a
// local `blackbox serve --web-dir web/dist` on one origin (see `e2e-prod/env.ts`).
export default defineConfig({
  testDir: './e2e-prod',
  fullyParallel: false,
  workers: 1,
  retries: 1,
  timeout: 240_000,
  globalTimeout: 1_200_000,
  expect: { timeout: 60_000 },
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report-prod' }]],
  use: {
    baseURL: WEB_URL,
    viewport: { width: 1440, height: 900 },
    navigationTimeout: 120_000,
    actionTimeout: 30_000,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } } }],
});
