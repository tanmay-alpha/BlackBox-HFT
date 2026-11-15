// Anonymous visitor on the public latency experiment (`/`): the question, the stored answer (every
// number checked against the API), the jump to the first divergence (URL, focus, lifecycle of
// both scenarios, book at the jump time), the assumptions, the forensics round trip (state and
// scroll restored), narrow viewports without horizontal overflow, and a keyboard-only path.

import type { APIRequestContext, Page } from '@playwright/test';

import { expect, test } from './fixtures';

const QUESTION = 'What changes when a market-making strategy receives and executes orders more slowly?';
const DEMO_ID = 'cost_of_latency';

interface DemoRun {
  run_id: string | null;
  label: string;
}
interface Pair {
  order_id: number;
  decision_ts: string | number;
  a: { exch_arrival_ts: string | number | null } | null;
  b: { exch_arrival_ts: string | number | null } | null;
}
interface MetricRow {
  key: string;
  a: number | null;
  b: number | null;
}
interface Comparison {
  id: string;
  run_a: string;
  run_b: string;
  metrics: MetricRow[];
  alignment: { first_divergence: Pair | null } | null;
}
interface Scenario {
  runId: string;
  name: string;
  netPnl: number;
}
interface Stored {
  comparison: Comparison;
  fast: Scenario;
  slow: Scenario;
}

const ns = (v: string | number) => Number(v);
/** Plain seconds as the page prints them (`0.100000 s`). */
const sec = (v: string | number, digits: number) => `${(ns(v) / 1e9).toFixed(digits)} s`;
/** Thousands-grouped fixed decimals as the page prints magnitudes. */
const num = (x: number, digits: number) => Math.abs(x).toLocaleString('en-US', { minimumFractionDigits: digits, maximumFractionDigits: digits });

let stored: Stored | null = null;

/**
 * The stored latency experiment, read from the API the page uses. `serve.mjs` runs
 * `demo seed --wait` before `serve` starts, so the demo runs, curated comparisons and reports are
 * stored before the server builds its anonymous demo snapshot.
 */
async function experiment(request: APIRequestContext): Promise<Stored> {
  if (stored) return stored;
  const demos = (await (await request.get('/api/v1/demos')).json()) as { id: string; runs: DemoRun[] }[];
  const runs = demos.find((d) => d.id === DEMO_ID)?.runs ?? [];
  const ids = runs.flatMap((r) => (r.run_id === null ? [] : [r.run_id]));
  expect(ids.length, 'both latency runs are stored').toBe(2);
  const list = (await (await request.get('/api/v1/comparisons?limit=50')).json()) as { items: { id: string; run_a: string; run_b: string }[] };
  const summary = list.items.find((c) => ids.includes(c.run_a) && ids.includes(c.run_b));
  expect(summary, 'the curated latency comparison is stored').toBeDefined();
  const comparison = (await (await request.get(`/api/v1/comparisons/${summary!.id}`)).json()) as Comparison;

  const scenario = async (runId: string): Promise<Scenario & { entryNs: number }> => {
    const run = (await (await request.get(`/api/v1/runs/${runId}`)).json()) as {
      metrics: { net_pnl: number };
      config: { execution: { order_latency: { entry_ns: string | number } } };
    };
    const entryNs = ns(run.config.execution.order_latency.entry_ns);
    return { runId, entryNs, name: `${entryNs / 1e6} ms`, netPnl: run.metrics.net_pnl };
  };
  const both = await Promise.all(ids.map(scenario));
  both.sort((x, y) => x.entryNs - y.entryNs);
  stored = { comparison, fast: both[0], slow: both[1] };
  return stored;
}

/** The comparison's metric for one scenario (`a`/`b` are comparison sides, not fast/slow). */
function metricOf(s: Stored, key: string, which: 'fast' | 'slow'): number {
  const row = s.comparison.metrics.find((m) => m.key === key);
  expect(row, `metric ${key}`).toBeTruthy();
  const side = s.comparison.run_a === s[which].runId ? 'a' : 'b';
  return row![side] as number;
}

async function openExplore(page: Page): Promise<void> {
  await page.goto('/');
  await expect(page.getByRole('heading', { level: 1, name: QUESTION })).toBeVisible();
  await expect(page.locator('.ex-chart-host canvas').first()).toBeVisible();
}

const scrollerTop = (page: Page) => page.locator('.ex').evaluate((el) => el.scrollTop);

async function noHorizontalOverflow(page: Page): Promise<void> {
  const o = await page.evaluate(() => {
    const ex = document.querySelector('.ex');
    return {
      doc: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      page: ex ? ex.scrollWidth - ex.clientWidth : 0,
    };
  });
  expect(o).toEqual({ doc: 0, page: 0 });
}

test('guided journey: answer from the API, first divergence, assumptions, forensics and back', async ({ page, request }) => {
  const s = await experiment(request);
  const first = s.comparison.alignment?.first_divergence;
  expect(first, 'the stored comparison has a first divergence').toBeTruthy();

  await openExplore(page);
  await expect(page.getByText('Synthetic data', { exact: true })).toBeVisible();
  await expect(page.getByText('No market impact', { exact: true })).toBeVisible();

  // The answer and the result table carry the stored numbers.
  const answer = page.locator('.ex-answer');
  for (const k of ['fast', 'slow'] as const) {
    await expect(answer).toContainText(`at ${s[k].name}`, { ignoreCase: true });
    await expect(answer).toContainText(num(s[k].netPnl, 2));
  }
  await expect(answer).toContainText('A smaller loss is still a loss');
  const results = page.locator('.ex-results-wide');
  for (const k of ['fast', 'slow'] as const) {
    const row = results.locator('tbody tr', { hasText: s[k].name }).first();
    await expect(row).toContainText(num(metricOf(s, 'fill_count', k), 0));
    await expect(row).toContainText(num(metricOf(s, 'fees', k), 2));
    await expect(row).toContainText(num(metricOf(s, 'net_pnl', k), 2));
  }

  // First divergence: URL, focus, both scenarios in the lifecycle, book at the jump time.
  await page.getByRole('button', { name: /^First divergence/ }).click();
  await expect(page).toHaveURL(new RegExp(`[?&]order=${first!.order_id}(&|$)`));
  await expect(page).toHaveURL(new RegExp(`[?&]t=${ns(first!.decision_ts)}(&|$)`));
  const heading = page.getByRole('heading', { name: `Order ${first!.order_id} in both scenarios` });
  await expect(heading).toBeFocused();
  const life = page.locator('.ex-life-table');
  await expect(life.locator('thead')).toContainText(s.fast.name);
  await expect(life.locator('thead')).toContainText(s.slow.name);
  const arrival = life.locator('tbody tr', { hasText: 'Order reaches the exchange' });
  const fastSide = s.comparison.run_a === s.fast.runId ? first!.a : first!.b;
  const slowSide = s.comparison.run_a === s.fast.runId ? first!.b : first!.a;
  if (fastSide?.exch_arrival_ts != null) await expect(arrival).toContainText(sec(fastSide.exch_arrival_ts, 6));
  if (slowSide?.exch_arrival_ts != null) await expect(arrival).toContainText(sec(slowSide.exch_arrival_ts, 6));
  await expect(page.locator('.ex-time')).toContainText(sec(first!.decision_ts, 3));
  const bookAt = await page.locator('.ex-ladder .ex-num').first().innerText();
  expect(Number.parseFloat(bookAt)).toBeLessThanOrEqual(ns(first!.decision_ts) / 1e9);

  // Assumptions.
  await page.getByRole('button', { name: /^Assumptions & limits/ }).click();
  await expect(page).toHaveURL(/[?&]step=4(&|$)/);
  await expect(page.getByRole('heading', { name: 'What the model assumes (identical in both)' })).toBeVisible();
  await expect(page.getByText('Technical details and identifiers')).toBeVisible();

  // Forensics and back: the view and the scroll position come back.
  const link = page.getByRole('link', { name: `Full forensics for this order (${s.fast.name}) →` });
  await link.scrollIntoViewIfNeeded();
  const top = await scrollerTop(page);
  const viewUrl = page.url();
  await link.click();
  await expect(page).toHaveURL(new RegExp(`/forensics/${s.fast.runId}/orders/${first!.order_id}$`));
  await page.goBack();
  await expect(page).toHaveURL(viewUrl);
  await expect(heading).toBeVisible();
  await expect(page.locator('.ex-life-table')).toBeVisible();
  await expect.poll(() => scrollerTop(page)).toBe(top);
});

for (const [width, height] of [
  [768, 1024],
  [390, 844],
] as const) {
  test(`no horizontal overflow and no blank band at ${width}x${height}`, async ({ page, request }) => {
    const s = await experiment(request);
    await page.setViewportSize({ width, height });
    await openExplore(page);
    await noHorizontalOverflow(page);
    // The chart starts within the first screen.
    const chartTop = await page.locator('.ex-chart-wrap').evaluate((el) => el.getBoundingClientRect().top);
    expect(chartTop).toBeLessThan(height);

    await page.getByRole('button', { name: /^First divergence/ }).click();
    const first = s.comparison.alignment!.first_divergence!;
    await expect(page.getByRole('heading', { name: `Order ${first.order_id} in both scenarios` })).toBeFocused();
    await expect(page.locator('.ex-life-table')).toBeVisible();
    await noHorizontalOverflow(page);
    // The console never scrolls as a document; nothing below it shows through.
    expect(await page.evaluate(() => window.scrollY)).toBe(0);
    const blank = await page.evaluate(
      ([w, h]) => [0.6, 0.75, 0.9].map((f) => document.elementFromPoint(w / 2, Math.round(h * f))?.tagName ?? null),
      [width, height] as const,
    );
    expect(blank).not.toContain('HTML');
    await expect(page.getByRole('heading', { name: `Order ${first.order_id} in both scenarios` })).toBeInViewport();
  });
}

test('keyboard only: reach the first divergence, read its lifecycle, change step', async ({ page, request }) => {
  const s = await experiment(request);
  const first = s.comparison.alignment!.first_divergence!;
  await openExplore(page);
  await page.locator('body').focus();

  let reached = false;
  for (let i = 0; i < 60 && !reached; i += 1) {
    await page.keyboard.press('Tab');
    reached = await page.evaluate(() => (document.activeElement?.textContent ?? '').trim().startsWith('First divergence'));
  }
  expect(reached, 'Tab reaches "First divergence"').toBe(true);
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { name: `Order ${first.order_id} in both scenarios` })).toBeFocused();
  await expect(page).toHaveURL(/[?&]step=3(&|$)/);

  // The next Tab stop is inside the lifecycle: its first time moves the replay.
  await page.keyboard.press('Tab');
  await expect(page.locator('.ex-life-sec button:focus')).toHaveCount(1);
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(new RegExp(`[?&]t=${ns(first.decision_ts)}(&|$)`));

  // `]` goes to the next guided step from anywhere outside a text field.
  await page.keyboard.press(']');
  await expect(page).toHaveURL(/[?&]step=4(&|$)/);
  await expect(page.getByRole('heading', { name: 'What the model assumes (identical in both)' })).toBeVisible();
});
