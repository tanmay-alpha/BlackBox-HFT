import { defineConfig, devices } from '@playwright/test';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const webDir = dirname(fileURLToPath(import.meta.url));
const binary = resolve(webDir, process.env.BLACKBOX_BIN ?? `../target/release/blackbox${process.platform === 'win32' ? '.exe' : ''}`);
// Workers inherit this value; only the runner creates a directory. Never reuse the local data/.
const dataDir = process.env.BLACKBOX_E2E_DATA_DIR ?? mkdtempSync(join(tmpdir(), 'blackbox-e2e-'));
process.env.BLACKBOX_E2E_DATA_DIR = dataDir;
const port = 18787;
const baseURL = `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: './e2e',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 90_000,
  globalTimeout: 180_000,
  expect: { timeout: 10_000 },
  reporter: [['list'], ['html', { open: 'never' }]],
  metadata: { dataDir },
  globalTeardown: './e2e/global-teardown.ts',
  use: {
    baseURL,
    viewport: { width: 1440, height: 1000 },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: `"${binary}" --data-dir "${dataDir}" demo --addr 127.0.0.1:${port} --web-dir dist`,
    cwd: webDir,
    url: `${baseURL}/api/health`,
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
