import { defineConfig, devices } from '@playwright/test';

// The v1 stories run against a real `blackbox serve` (Postgres `blackbox_dev`, file artefact
// store) serving the built UI on the same origin; `e2e/serve.mjs` migrates, seeds the demo
// workspace and mints the research key first. Build the UI (`npm run build`) and the binary
// (`cargo build -p bb-app --bin blackbox`) before `npm run test:e2e`.
const port = Number(process.env.E2E_PORT ?? 18797);
const baseURL = `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: './e2e',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 180_000,
  globalTimeout: 900_000,
  expect: { timeout: 15_000 },
  reporter: [['list'], ['html', { open: 'never' }]],
  use: {
    baseURL,
    viewport: { width: 1440, height: 900 },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } } }],
  webServer: {
    command: 'node e2e/serve.mjs',
    url: `${baseURL}/api/v1/status`,
    env: { E2E_PORT: String(port) },
    reuseExistingServer: false,
    timeout: 180_000,
    stdout: 'ignore',
    stderr: 'pipe',
  },
});
