// Shared by the anonymous production specs: the API reader, the paced page load and a health
// fixture. Every spec stays read-only (GET only, no keys) and loads few pages: Vercel's bot
// checkpoint challenges automated clients after roughly a hundred loads.

import { test as base, expect, type APIRequestContext, type Page } from '@playwright/test';

import { API_URL, WEB_URL } from './env';

export { expect };

/** Minimum pause before every full page load. */
const PAUSE_MS = 2_000;

/** `GET /api/v1{path}` with the UI's origin, as the browser sends it; asserts 200. */
export async function api<T>(request: APIRequestContext, path: string): Promise<T> {
  const res = await request.get(`${API_URL}/api/v1${path}`, { headers: { Origin: WEB_URL }, timeout: 120_000 });
  expect(res.status(), `GET /api/v1${path}`).toBe(200);
  return (await res.json()) as T;
}

/** A full page load after the pause, recorded as a `page-load` annotation (the load budget). */
export async function load(page: Page, url: string): Promise<void> {
  await page.waitForTimeout(PAUSE_MS);
  test.info().annotations.push({ type: 'page-load', description: url });
  await page.goto(url);
}

// Every page: no console errors, uncaught exceptions, HTTP errors or failed requests, except the
// statuses (`allowStatus`) and failed requests (`allowFailed`) a test provokes on purpose.
export const test = base.extend<{ health: void; allowStatus: number[]; allowFailed: boolean }>({
  allowStatus: [[], { option: true }],
  allowFailed: [false, { option: true }],
  health: [
    async ({ context, allowStatus, allowFailed }, use) => {
      const errors: string[] = [];
      context.on('page', (page) => {
        page.on('console', (m) => {
          // A refused request logs "Failed to load resource"; its status is judged below.
          if (m.type() === 'error' && !/Failed to load resource/.test(m.text())) errors.push(`console: ${m.text()}`);
        });
        page.on('pageerror', (e) => errors.push(`page: ${e.message}`));
      });
      context.on('requestfailed', (r) => {
        const why = r.failure()?.errorText ?? '';
        if (!allowFailed && !/ERR_ABORTED|NS_BINDING_ABORTED/.test(why)) errors.push(`${r.method()} ${r.url()}: ${why}`);
      });
      context.on('response', (r) => {
        if (r.status() >= 400 && !allowStatus.includes(r.status())) errors.push(`${r.status()} ${r.url()}`);
      });
      await use();
      expect(errors, 'no console errors, uncaught exceptions, HTTP errors or failed requests').toEqual([]);
    },
    { auto: true },
  ],
});
