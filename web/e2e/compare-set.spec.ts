// Phase 6 A1/A2 (docs/rebuild/PHASE6_RELIABILITY.md): the curated execution_validation experiment
// on Experiment Comparison -> Set, against the local stack `serve.mjs` seeded with `demo seed
// --wait` before `serve` started.
//
// * The overview reads no run by id (no bundle read): runs come from the verified run list with
//   the experiment's configurations, pairs from the stored comparisons.
// * Every run's relation to the baseline is told apart: three aligned comparisons, two
//   metrics-only ones across datasets, and the depleted risk-adverse run compared with the
//   depleted baseline instead. An anonymous visitor is offered no (locked) create control.
// * A busy server is retried a bounded number of times; a comparison still refused is shown as
//   unavailable with Retry, never as missing, and Retry completes the page.

import type { Route } from '@playwright/test';

import { expect, shot, test } from './fixtures';

const DETAIL_RUN = /\/api\/v1\/runs\/[0-9a-f]{16}(?:[?/]|$)/;
const DETAIL_COMPARISON = /\/api\/v1\/comparisons\/[0-9a-f-]{36}$/;

test('execution_validation: every pair classified from stored comparisons, no bundle read', async ({ page }) => {
  const api: string[] = [];
  page.on('request', (r) => {
    if (r.url().includes('/api/v1/')) api.push(new URL(r.url()).pathname + new URL(r.url()).search);
  });
  await page.goto('/compare/set?demo=execution_validation');
  const table = page.getByRole('grid', { name: 'Run set' });
  await expect(table).toBeVisible();
  const states = page.locator('[data-pair-state]');
  await expect(states).toHaveCount(6);
  await expect(page.locator('[data-pair-state="aligned"]')).toHaveCount(3);
  await expect(page.locator('[data-pair-state="metrics_only"]')).toHaveCount(2);
  const elsewhere = page.locator('[data-pair-state="elsewhere"]');
  await expect(elsewhere).toHaveCount(1);
  await expect(elsewhere).toContainText('compared with');
  await expect(elsewhere.getByRole('link')).toHaveAttribute('href', /\/compare\/pair\?a=[0-9a-f]{16}&b=[0-9a-f]{16}/);
  await expect(page.locator('[data-pair-state="missing"], [data-pair-state="unlisted"], [data-pair-state="busy"]')).toHaveCount(0);

  // A2: no "N runs have no stored comparison" and no create control, locked or not.
  const runSet = page.locator('section.panel').filter({ has: page.getByRole('heading', { name: 'Run set', level: 2 }) });
  await expect(page.getByText(/no stored comparison with the baseline \(A =/)).toHaveCount(0);
  await expect(page.getByRole('button', { name: /missing comparison/ })).toHaveCount(0);
  await expect(runSet.locator('[data-locked="true"]')).toHaveCount(0);
  await expect(page.getByText('Compared elsewhere', { exact: true })).toBeVisible();

  // The pair panels show what was stored: aligned pairs with a first divergence or none, the
  // metrics-only pairs without alignment.
  await expect(page.getByRole('heading', { name: 'Key metrics' })).toBeVisible();
  await expect(page.getByText(/Status metrics only: no per-order alignment/)).toHaveCount(2);
  await expect(page.getByText(/Status metrics_only:/)).toHaveCount(0);

  // A1: nothing was read by id (each such read verifies a whole bundle on the server).
  expect(api.filter((p) => DETAIL_RUN.test(p)), api.join('\n')).toEqual([]);
  await shot(page, 'p6-compare-set-execution-validation');
});

test.describe('busy server', () => {
  test.use({ allowStatus: [503] });

  test('bounded retries, then unavailable with Retry, never missing', async ({ page }) => {
    const attempts = new Map<string, number>();
    const order: string[] = [];
    let refuse = true;
    const busy = (route: Route) =>
      route.fulfill({
        status: 503,
        headers: { 'Content-Type': 'application/json', 'Retry-After': '1' },
        body: JSON.stringify({ error: { code: 'unavailable', message: 'the server is busy; retry shortly', details: { reason: 'busy' } } }),
      });
    await page.route(
      (url) => DETAIL_COMPARISON.test(url.pathname),
      async (route) => {
        const id = new URL(route.request().url()).pathname.split('/').pop() ?? '';
        if (!attempts.has(id)) order.push(id);
        const n = (attempts.get(id) ?? 0) + 1;
        attempts.set(id, n);
        // The first comparison read stays busy until Retry; the second is busy once only.
        if ((id === order[0] && refuse) || (id === order[1] && n === 1)) return busy(route);
        return route.continue();
      },
    );
    await page.goto('/compare/set?demo=execution_validation');
    await expect(page.locator('[data-pair-state]')).toHaveCount(6);
    const refused = page.locator('[data-pair-state="busy"]');
    await expect(refused).toHaveCount(1);
    await expect(refused).toContainText('unavailable: server busy');
    expect(attempts.get(order[0])).toBe(3);
    expect(attempts.get(order[1])).toBe(2);
    await expect(page.locator('[data-pair-state="aligned"]')).toHaveCount(2);
    await expect(page.locator('[data-pair-state="missing"]')).toHaveCount(0);
    await expect(page.getByText(/1 of 6 stored comparisons with the baseline could not be read \(the server was busy; 3 attempts each\)/)).toBeVisible();
    await expect(page.getByRole('cell', { name: 'unavailable (server busy)' }).first()).toBeVisible();
    await shot(page, 'p6-compare-set-busy');

    refuse = false;
    await page.getByRole('button', { name: 'Retry' }).first().click();
    await expect(page.locator('[data-pair-state="aligned"]')).toHaveCount(3);
    await expect(page.locator('[data-pair-state="busy"]')).toHaveCount(0);
    await expect(page.getByText(/could not be read/)).toHaveCount(0);
  });
});
