import { test as base, expect, type Page } from '@playwright/test';
import { mkdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const webDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');

/** The research key `serve.mjs` minted for this run. */
export function researchKey(): string {
  return readFileSync(join(webDir, '..', 'data', 'e2e-ui', 'e2e-key.txt'), 'utf8').trim();
}

const SHOTS = join(webDir, 'test-results', 'ui-v1');

/** A full-viewport screenshot into `web/test-results/ui-v1/`. */
export async function shot(page: Page, name: string): Promise<void> {
  mkdirSync(SHOTS, { recursive: true });
  await page.screenshot({ path: join(SHOTS, `${name}.png`) });
}

/** Enters the key through the access dialog and waits for RESEARCH mode. */
export async function signIn(page: Page): Promise<void> {
  await page.getByTestId('access-badge').click();
  const dialog = page.getByRole('dialog', { name: 'Research access' });
  await dialog.getByLabel('API key').fill(researchKey());
  await dialog.getByRole('button', { name: 'Use key' }).click();
  await expect(page.getByTestId('access-badge')).toContainText('RESEARCH');
}

// An automatic fixture covers every page: no console errors, uncaught exceptions, failed requests
// or HTTP errors, except statuses a test declares it expects.
export const test = base.extend<{ browserHealth: void; allowStatus: number[] }>({
  allowStatus: [[], { option: true }],
  browserHealth: [
    async ({ context, allowStatus }, use) => {
      const errors: string[] = [];
      context.on('page', (page) => {
        page.on('console', (message) => {
          if (message.type() === 'error' && !/Failed to load resource/.test(message.text())) errors.push(`console: ${message.text()}`);
        });
        page.on('pageerror', (error) => errors.push(`page: ${error.message}`));
      });
      context.on('requestfailed', (request) => {
        const why = request.failure()?.errorText ?? '';
        if (!/ERR_ABORTED|NS_BINDING_ABORTED/.test(why)) errors.push(`${request.method()} ${request.url()}: ${why}`);
      });
      context.on('response', (response) => {
        if (response.status() >= 400 && !allowStatus.includes(response.status())) errors.push(`${response.status()} ${response.url()}`);
      });
      await use();
      expect(errors, 'No console errors, uncaught exceptions, HTTP errors, or failed browser requests').toEqual([]);
    },
    { auto: true },
  ],
});

export { expect };
