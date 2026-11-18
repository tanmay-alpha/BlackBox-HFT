// Loading states (PHASE6 C1/C2): while a workspace's code chunk loads, the shell stays mounted and
// the workspace area shows a busy loading frame in the box the workspace then fills; the order
// book replay shows its (true) page heading while its first window loads. Requests are held with
// `page.route` until the assertions on the loading state have run, then released.

import type { Page } from '@playwright/test';

import { expect, test } from './fixtures';
import { demoIds } from './measure';

/** Holds every request matching `pattern` until `release()` is called. */
async function hold(page: Page, pattern: RegExp | ((url: URL) => boolean)) {
  let release!: () => void;
  const gate = new Promise<void>((r) => (release = r));
  let held = 0;
  await page.route(pattern, async (route) => {
    held += 1;
    await gate;
    await route.continue();
  });
  return { release, held: () => held };
}

const box = async (page: Page, selector: string) => {
  const b = await page.locator(selector).first().boundingBox();
  expect(b, `${selector} has a box`).not.toBeNull();
  return b!;
};

test('a cold load keeps the shell and shows a busy workspace frame while the chunk loads', async ({ page }) => {
  const chunk = await hold(page, /\/assets\/WorkbenchPage-[^/]*\.js$/);
  await page.goto('/sim');
  const loading = page.locator('#main [aria-busy="true"]');
  await expect(loading).toBeVisible();
  await expect(loading.getByRole('status')).toContainText('Loading workspace');
  await expect(page.getByRole('navigation', { name: 'Workspaces' })).toBeVisible();
  await expect(page.getByTestId('access-badge')).toBeVisible();
  await expect.poll(() => chunk.held()).toBeGreaterThan(0);
  const before = await box(page, '#main > .ws');

  chunk.release();
  await expect(page.getByRole('heading', { name: 'Job queue' })).toBeVisible();
  await expect(page.locator('#main [aria-busy="true"]')).toHaveCount(0);
  // The workspace fills the same box the loading frame held.
  expect(await box(page, '#main > .ws')).toEqual(before);
});

test('navigating to a workspace whose chunk is not loaded yet shows the loading frame, not an empty area', async ({ page }) => {
  await page.goto('/sim');
  await expect(page.getByRole('heading', { name: 'Job queue' })).toBeVisible();
  const chunk = await hold(page, /\/assets\/DatasetsPage-[^/]*\.js$/);
  await page.getByRole('navigation', { name: 'Workspaces' }).getByRole('link', { name: 'Datasets' }).click();
  await expect(page).toHaveURL(/\/data$/);
  await expect(page.locator('#main [aria-busy="true"]').getByRole('status')).toContainText('Loading workspace');
  await expect.poll(() => chunk.held()).toBeGreaterThan(0);
  chunk.release();
  await expect(page.locator('#main [aria-busy="true"]')).toHaveCount(0);
  await expect(page.locator('#main .ws-toolbar[role="toolbar"]')).toBeVisible();
});

test('the order book replay shows its heading while the first window loads', async ({ page, request }) => {
  const { run } = await demoIds(request);
  // The first window's timeline (the zero-event probe for the run span is let through).
  const windowReq = await hold(page, (url) => url.pathname.endsWith(`/runs/${run}/timeline`) && url.searchParams.get('max_events') !== '0');
  await page.goto(`/book/runs/${run}`);
  const title = page.getByRole('heading', { level: 1, name: /^Order book replay · / });
  // The heading can render before the window request is sent: wait until it is held, then check
  // that the heading and the loading status are shown while it is.
  await expect.poll(() => windowReq.held()).toBeGreaterThan(0);
  await expect(title).toBeVisible();
  await expect(page.locator('#main').getByRole('status').filter({ hasText: /Loading/ }).first()).toBeVisible();
  await expect(page.getByRole('heading', { level: 1 })).toHaveCount(1);
  const before = await box(page, '.term-head');

  windowReq.release();
  await expect(page.locator('.heatmap-wrap canvas')).toBeVisible();
  await expect(page.getByRole('heading', { level: 1 })).toHaveCount(1);
  await expect(title).toBeVisible();
  // The heading row does not move when the window arrives.
  expect((await box(page, '.term-head')).y).toBe(before.y);
});
