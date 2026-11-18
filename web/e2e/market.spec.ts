// Market Data Workbench, order book step (anonymous, synthetic demo): it opens on a curated
// example instead of an early empty book, every curated example is linkable, all panes describe
// the one selected source event, J/K step events, the raw tables stay in the expert view, and the
// step fits 1440×900, 1024×768 and 390×844 with the synthetic provenance always visible. The
// `browserHealth` fixture fails any story with a console error or unexpected HTTP error.

import type { Page } from '@playwright/test';

import { expect, test } from './fixtures';

const ROOT = '/market/l3-demo';

interface Example {
  id: string;
  title: string;
  event_index: number;
  rule: string;
}

async function examples(page: Page): Promise<Example[]> {
  const res = await page.request.get('/api/v1/market/datasets/l3-demo/examples');
  expect(res.status()).toBe(200);
  // The endpoint's documented shape (web/openapi.json `MarketExamples`).
  const body: { items: Example[] } = await res.json();
  return body.items;
}

const fmt = (n: number) => n.toLocaleString('en-US');

async function ladderReady(page: Page, index: number): Promise<void> {
  await expect(page).toHaveURL(new RegExp(`[?&]event=${index}(&|$)`));
  await expect(page.locator('.mk-cmp-event .mk-event-facts')).toContainText(`#${fmt(index)}`);
  await expect(page.locator('.mk-ladder')).toBeVisible();
}

test('the book step opens on the first curated example, not an empty early book', async ({ page }) => {
  const items = await examples(page);
  expect(items.map((x) => x.id)).toEqual(['cancel-ahead', 'partial-execution', 'replace', 'same-timestamp', 'level-change']);
  await page.goto(`${ROOT}?tab=book`);
  await ladderReady(page, items[0].event_index);
  await expect(page.getByRole('heading', { name: 'What each view records' })).toBeVisible();
  await expect(page.locator('.mk-ladder tbody tr.mk-r-bid')).not.toHaveCount(0);
  await expect(page.locator('.mk-ladder tbody tr.mk-r-ask')).not.toHaveCount(0);
});

test('every curated example is linkable and all panes describe that one event', async ({ page }) => {
  for (const x of await examples(page)) {
    await page.goto(`${ROOT}?example=${x.id}`);
    await ladderReady(page, x.event_index);
    const nav = page.getByRole('navigation', { name: 'Curated examples' });
    await expect(nav.getByRole('link', { name: new RegExp(x.title) })).toHaveAttribute('aria-current', 'true');
    await expect(nav).toContainText(x.rule);
    // The event's own order is marked in the synchronized queue of the level it changed.
    await expect(page.locator('.mk-queue tr.mk-hl').first()).toBeVisible();
    // The ladder's changed rows and the L2 statements come from the same response: each Δ shown
    // on a changed row appears verbatim in the L2 statements.
    const statements = (await page.locator('.mk-obs-list dd').allInnerTexts()).join('\n');
    for (const delta of await page.locator('.mk-ladder tr.mk-ladder-hit .mk-delta').allInnerTexts()) {
      if (delta.trim() !== '') expect(statements).toContain(`(${delta.trim()})`);
    }
  }
});

test('J and K step to the next and previous source event; the expert view keeps the raw tables', async ({ page }) => {
  const [first] = await examples(page);
  await page.goto(`${ROOT}?tab=book&event=${first.event_index}`);
  await ladderReady(page, first.event_index);
  await page.locator('.mk-ladder').click();
  await page.keyboard.press('j');
  await ladderReady(page, first.event_index + 1);
  await page.keyboard.press('k');
  await ladderReady(page, first.event_index);
  await page.locator('.mk-expert > summary').click();
  await expect(page.locator('.mk-raw table.mk-l3').first()).toBeVisible();
  await expect(page.locator('.mk-raw table.mk-l2')).toBeVisible();
});

for (const [width, height] of [
  [1440, 900],
  [1024, 768],
  [390, 844],
] as const) {
  test.describe(`${width}×${height}`, () => {
    test.use({ viewport: { width, height } });

    test('fits without horizontal overflow and keeps the synthetic provenance visible', async ({ page }) => {
      const [, partial] = await examples(page);
      await page.goto(`${ROOT}?example=${partial.id}`);
      await ladderReady(page, partial.event_index);
      await expect(page.getByText('synthetic \u2014 generated, not market data').first()).toBeVisible();
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      expect(overflow).toBeLessThanOrEqual(0);
      const ladder = await page.locator('.mk-ladder').boundingBox();
      expect(ladder).not.toBeNull();
      expect((ladder?.x ?? 0) + (ladder?.width ?? 0)).toBeLessThanOrEqual(width);
    });
  });
}
