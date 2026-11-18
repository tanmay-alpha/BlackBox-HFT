// Production acceptance: the deployed product, anonymous and read-only. Selectors and flows mirror
// `e2e/explore.spec.ts` and `e2e/anonymous.spec.ts`. Keep requests few (anonymous egress budget).

import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { API_URL as API, IS_PRODUCTION, WEB_URL as ORIGIN } from './env';

const QUESTION = 'What changes when a market-making strategy receives and executes orders more slowly?';
const DEMO_ID = 'cost_of_latency';
const BOOK_RUN = '253bbc23b342f373';
const OTHER_RUN = 'd6a57aed972e18d0';

// Production evidence goes to the docs; screenshots of any other target stay in test-results.
const webDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SHOTS = IS_PRODUCTION ? resolve(webDir, '..', 'docs', 'rebuild', 'img') : join(webDir, 'test-results', 'prod-shots');

interface Pair {
  order_id: number;
  decision_ts: string | number;
}
interface Comparison {
  id: string;
  run_a: string;
  run_b: string;
  metrics: { key: string; a: number | null; b: number | null }[];
  alignment: { first_divergence: Pair | null } | null;
}
interface Stored {
  comparison: Comparison;
  fast: { runId: string; name: string };
  slow: { runId: string; name: string };
}

const ns = (v: string | number) => Number(v);
const num = (x: number, digits: number) =>
  Math.abs(x).toLocaleString('en-US', { minimumFractionDigits: digits, maximumFractionDigits: digits });

async function api<T>(request: APIRequestContext, path: string): Promise<T> {
  const res = await request.get(`${API}/api/v1${path}`, { headers: { Origin: ORIGIN }, timeout: 120_000 });
  expect(res.ok(), `GET ${path} -> ${res.status()}`).toBe(true);
  return (await res.json()) as T;
}

let stored: Stored | null = null;
async function experiment(request: APIRequestContext): Promise<Stored> {
  if (stored) return stored;
  const demos = await api<{ id: string; runs: { run_id: string | null }[] }[]>(request, '/demos');
  const ids = (demos.find((d) => d.id === DEMO_ID)?.runs ?? []).flatMap((r) => (r.run_id ? [r.run_id] : []));
  expect(ids.length, 'both latency runs are stored').toBe(2);
  const list = await api<{ items: { id: string; run_a: string; run_b: string }[] }>(request, '/comparisons?limit=50');
  const summary = list.items.find((c) => ids.includes(c.run_a) && ids.includes(c.run_b));
  expect(summary, 'the latency comparison is stored').toBeDefined();
  const comparison = await api<Comparison>(request, `/comparisons/${summary!.id}`);
  const both = await Promise.all(
    ids.map(async (runId) => {
      const run = await api<{ config: { execution: { order_latency: { entry_ns: string | number } } } }>(request, `/runs/${runId}`);
      const entryNs = ns(run.config.execution.order_latency.entry_ns);
      return { runId, entryNs, name: `${entryNs / 1e6} ms` };
    }),
  );
  both.sort((x, y) => x.entryNs - y.entryNs);
  stored = { comparison, fast: both[0], slow: both[1] };
  return stored;
}

function metricOf(s: Stored, key: string, which: 'fast' | 'slow'): number {
  const row = s.comparison.metrics.find((m) => m.key === key);
  expect(row, `metric ${key}`).toBeTruthy();
  return row![s.comparison.run_a === s[which].runId ? 'a' : 'b'] as number;
}

async function shot(page: Page, name: string): Promise<void> {
  mkdirSync(SHOTS, { recursive: true });
  await page.screenshot({ path: join(SHOTS, `prod-${name}.png`) });
}

/** Navigates and records navigation → `ready` visible as an annotation. */
async function timed(page: Page, url: string, label: string, ready: () => Promise<void>): Promise<void> {
  const t0 = Date.now();
  await page.goto(url);
  await ready();
  test.info().annotations.push({ type: 'timing', description: `${label}: ${Date.now() - t0} ms` });
}

const openExplore = (page: Page, label = '/') =>
  timed(page, '/', label, async () => {
    await expect(page.getByRole('heading', { level: 1, name: QUESTION })).toBeVisible();
  });

// No page errors or console errors on any page.
test.beforeEach(({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(`console: ${m.text()}`);
  });
  (page as Page & { __errors?: string[] }).__errors = errors;
});
test.afterEach(({ page }) => {
  expect((page as Page & { __errors?: string[] }).__errors ?? [], 'no page/console errors').toEqual([]);
});

test('latency question: stored numbers, first divergence, forensics and back', async ({ page, request }) => {
  const s = await experiment(request);
  const first = s.comparison.alignment?.first_divergence;
  expect(first, 'the stored comparison has a first divergence').toBeTruthy();

  await openExplore(page);
  const results = page.locator('.ex-results-wide');
  for (const k of ['fast', 'slow'] as const) {
    const row = results.locator('tbody tr', { hasText: s[k].name }).first();
    await expect(row).toContainText(num(metricOf(s, 'fill_count', k), 0));
    await expect(row).toContainText(num(metricOf(s, 'fees', k), 2));
    await expect(row).toContainText(num(metricOf(s, 'net_pnl', k), 2));
  }
  await shot(page, 'explore');

  await page.getByRole('button', { name: /^First divergence/ }).click();
  await expect(page).toHaveURL(new RegExp(`[?&]order=${first!.order_id}(&|$)`));
  const heading = page.getByRole('heading', { name: `Order ${first!.order_id} in both scenarios` });
  await expect(heading).toBeFocused();
  const life = page.locator('.ex-life-table');
  await expect(life.locator('thead')).toContainText(s.fast.name);
  await expect(life.locator('thead')).toContainText(s.slow.name);
  await shot(page, 'first-divergence');

  const link = page.getByRole('link', { name: `Full forensics for this order (${s.fast.name}) →` });
  await link.scrollIntoViewIfNeeded();
  const viewUrl = page.url();
  await link.click();
  await expect(page).toHaveURL(new RegExp(`/forensics/${s.fast.runId}/orders/${first!.order_id}$`));
  await expect(page.locator('.error-box')).toHaveCount(0);
  await shot(page, 'forensics');
  await page.goBack();
  await expect(page).toHaveURL(viewUrl);
  await expect(heading).toBeVisible();
  await expect(life).toBeVisible();
});

test('book replay window loads without an error box', async ({ page }) => {
  await timed(page, `/book/runs/${BOOK_RUN}`, 'book', async () => {
    await expect(page.locator('canvas').first()).toBeVisible();
  });
  await expect(page.locator('.error-box')).toHaveCount(0);
  await shot(page, 'book');
});

for (const [name, url, ready] of [
  ['experiments', '/experiments', (p: Page) => expect(p.getByTestId('access-badge')).toContainText('DEMO')],
  [
    'compare-set',
    `/compare/set?runs=${BOOK_RUN},${OTHER_RUN}&demo=${DEMO_ID}`,
    (p: Page) => expect(p.getByText(BOOK_RUN.slice(0, 12)).first()).toBeVisible(),
  ],
  ['system', '/system', (p: Page) => expect(p.getByTestId('system-build')).toBeVisible()],
] as const) {
  test(`${name} renders`, async ({ page }) => {
    await timed(page, url, name, () => ready(page));
    await expect(page.locator('.error-box')).toHaveCount(0);
    await shot(page, name);
  });
}

for (const [width, height] of [
  [768, 1024],
  [390, 844],
] as const) {
  test(`no horizontal overflow at ${width}x${height}`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await openExplore(page, `/ @${width}`);
    const sw = await page.evaluate(() => document.documentElement.scrollWidth);
    expect(sw).toBe(width);
    await shot(page, `explore-${width}`);
  });
}
