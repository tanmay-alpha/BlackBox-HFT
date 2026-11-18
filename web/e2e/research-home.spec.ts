// Research home (`/research`, docs/engineering/RESEARCH_IDENTITY.md): five investigations whose
// numbers equal the sources they are read from (the published study artefact, the workbench event
// detail API), whose evidence links land on the exact state they name with Back/Forward
// restoring it, and whose failure states are visible: a tampered study is refused in every row
// that reads it, a busy workbench API is shown as such, and no number is shown from either.

import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { Page } from '@playwright/test';

import { expect, shot, test } from './fixtures';

const webDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const STUDY = '**/study-results/exec-validation/study-result.json';

interface Metric {
  kind: string;
  value?: number;
  bootstrap?: { mean: number } | null;
}
interface Aggregate {
  regime: string;
  queue_model: string;
  latency: string;
  feed_offset_ns: number;
  design: string;
  metrics: Record<string, Metric>;
}
interface Cell {
  run_id: string;
  regime: string;
  design: string;
  measures: Record<string, number>;
  accounting: { gross_pnl: number; fees: number; net_pnl: number };
}
interface Artefact {
  baseline: { run_id: string; regime: string; queue_model: string; latency: string; feed_offset_ns: number };
  cells: Cell[];
  aggregates: Aggregate[];
}

/** The published artefact, read from disk: the independent oracle for every study number. */
const artefact: Artefact = JSON.parse(readFileSync(join(webDir, 'public', 'study-results', 'exec-validation', 'study-result.json'), 'utf8'));
const value = (m: Metric) => (m.kind === 'deterministic_single' ? (m.value as number) : (m.bootstrap as { mean: number }).mean);
const pct = (x: number) => `${(x * 100).toFixed(2)}%`;
const signed4 = (x: number) => (x < 0 ? `\u2212${Math.abs(x).toFixed(4)}` : `+${x.toFixed(4)}`);
const int = (x: number) => x.toLocaleString('en-US');

function row(page: Page, id: string) {
  return page.getByTestId(`rh-${id}`);
}

async function ready(page: Page): Promise<void> {
  for (const id of ['l2-queue', 'fees', 'latency', 'regime', 'ranking']) await expect(row(page, id)).toHaveAttribute('data-state', 'ready');
}

test.describe('research home', () => {
  test('five investigations show their sources\u2019 numbers; evidence links keep context through Back and Forward', async ({ page, request }) => {
    // Workbench oracle: the curated example and its event detail, straight from the API.
    const examples = (await (await request.get('/api/v1/market/datasets/l3-demo/examples')).json()) as { items: { id: string; event_index: number }[] };
    const example = examples.items.find((x) => x.id === 'cancel-ahead')!;
    const detail = await (await request.get(`/api/v1/market/datasets/l3-demo/events/${example.event_index}?orders_per_level=100`)).json();
    const ob = detail.order_before;
    const level = detail.affected.find((a: { side: string; price4: number }) => a.side === ob.side && a.price4 === ob.price4);
    const l2Delta = level.after.shares - level.before.shares;
    const next = level.before.orders.find((o: { queue_position: number }) => o.queue_position === ob.queue_position + 1);
    const nextAfter = level.after.orders.find((o: { order_ref: string }) => o.order_ref === next.order_ref);

    // Study oracle.
    const base = artefact.cells.find((c) => c.run_id === artefact.baseline.run_id)!;
    const onBase = (a: Aggregate) => a.regime === artefact.baseline.regime && a.queue_model === artefact.baseline.queue_model;
    const slow = artefact.aggregates.find((a) => onBase(a) && a.latency === 'entry_slow')!;
    const depletion = artefact.cells.find((c) => c.design === 'regime_baseline' && c.regime === 'liquidity_depletion')!;
    const baseNet = value(artefact.aggregates.find((a) => a.design === 'regime_baseline' && a.regime === artefact.baseline.regime)!.metrics.net_pnl);
    const top = artefact.aggregates
      .filter((a) => a.design === 'one_factor' && a.regime === artefact.baseline.regime)
      .map((a) => ({ a, d: value(a.metrics.net_pnl) - baseNet }))
      .sort((x, y) => Math.abs(y.d) - Math.abs(x.d))[0];
    const topCell = `group.${top.a.regime}.${top.a.queue_model}.${top.a.latency}.${top.a.feed_offset_ns}`;

    await page.goto('/research');
    await expect(page.getByRole('heading', { level: 1, name: 'Research home' })).toBeVisible();
    await expect(page).toHaveTitle('Research home \u2014 BlackBox HFT');
    await ready(page);
    await shot(page, 'research-home');

    // (a) L2 vs L3: the API's numbers, raw and as printed.
    const a = row(page, 'l2-queue');
    await expect(a).toHaveAttribute('data-event-index', String(example.event_index));
    await expect(a).toHaveAttribute('data-l2-delta', String(l2Delta));
    await expect(a).toHaveAttribute('data-position', String(ob.queue_position));
    await expect(a).toHaveAttribute('data-next-ahead-before', String(next.shares_ahead));
    await expect(a).toHaveAttribute('data-next-ahead-after', String(nextAfter.shares_ahead));
    await expect(a.getByTestId('rh-l2-delta')).toHaveText(l2Delta < 0 ? `\u2212${int(-l2Delta)}` : `+${int(l2Delta)}`);
    await expect(a.getByTestId('rh-ahead-change')).toHaveText(`${int(next.shares_ahead)} \u2192 ${int(nextAfter.shares_ahead)}`);
    await expect(a).toContainText('Synthetic generated events');
    await expect(a.getByTestId('rh-reproduce')).toContainText('blackbox market l3-synth --seed 7 --events 30000 --symbol SYNTH');

    // (b) fees: the baseline run's accounting.
    const b = row(page, 'fees');
    await expect(b).toHaveAttribute('data-run-id', base.run_id);
    expect(Number(await b.getAttribute('data-net'))).toBe(base.accounting.net_pnl);
    expect(Number(await b.getAttribute('data-fees'))).toBe(base.accounting.fees);
    await expect(b.getByTestId('rh-net')).toHaveText(signed4(base.accounting.net_pnl));

    // (c) latency: the entry-10 ms group's fill rate.
    const c = row(page, 'latency');
    expect(Number(await c.getAttribute('data-slow-fill-rate'))).toBe(value(slow.metrics.fill_rate));
    await expect(c.getByTestId('rh-slow-fill')).toHaveText(pct(value(slow.metrics.fill_rate)));

    // (d) regimes: the depletion baseline run's fill rate.
    const d = row(page, 'regime');
    expect(Number(await d.getAttribute('data-depletion-fill-rate'))).toBe(depletion.measures.fill_rate);
    await expect(d.getByTestId('rh-regime-liquidity_depletion')).toContainText(pct(depletion.measures.fill_rate));

    // (e) ranking: recomputed here from the artefact.
    const e = row(page, 'ranking');
    await expect(e).toHaveAttribute('data-top-cell', topCell);
    expect(Number(await e.getAttribute('data-top-delta'))).toBeCloseTo(top.d, 12);
    await expect(e.getByTestId('rh-top-delta')).toHaveText(signed4(top.d));
    await expect(e.getByTestId('rh-rank-row')).toHaveCount(5);

    // (a) evidence: the workbench opens on that event and example; Back returns.
    await a.getByTestId('rh-link-primary').click();
    await page.waitForURL(new RegExp(`/market/l3-demo\\?tab=book&event=${example.event_index}&example=cancel-ahead$`));
    await expect(page.locator('.mk-cmp-event .mk-event-facts')).toContainText(`#${int(example.event_index)}`);
    await expect(page.getByRole('navigation', { name: 'Curated examples' }).getByRole('link', { name: /Cancel ahead/ })).toHaveAttribute('aria-current', 'true');
    await expect(page.getByRole('navigation', { name: 'Context' })).toContainText(`event #${example.event_index}`);
    await page.goBack();
    await page.waitForURL(/\/research$/);
    await ready(page);

    // (b) evidence: the study with the baseline run as the selected cell; Back, then Forward.
    await b.getByTestId('rh-link-primary').click();
    const studyUrl = `/studies/exec-validation?metric=fees&cell=run.${base.run_id}`;
    await page.waitForURL((u) => `${u.pathname}${u.search}` === studyUrl);
    await expect(page.getByTestId('study-view')).toHaveAttribute('data-study-id', 'exec-validation');
    await expect(page).toHaveTitle(/^Execution studies \u00b7 /);
    await expect(page.getByRole('navigation', { name: 'Context' })).toContainText(`cell run.${base.run_id}`);
    await page.goBack();
    await page.waitForURL(/\/research$/);
    await ready(page);
    await page.goForward();
    await page.waitForURL((u) => `${u.pathname}${u.search}` === studyUrl);
    await expect(page.getByTestId('study-view')).toBeVisible();
    await page.goBack();
    await ready(page);

    // (e) evidence: the top-ranked group.
    await e.getByTestId('rh-link-primary').click();
    await page.waitForURL((u) => u.pathname === '/studies/exec-validation' && u.searchParams.get('cell') === topCell && u.searchParams.get('metric') === 'net_pnl');
    await page.goBack();
    await ready(page);

    // (d) the stored demo pair (both anchors matched by config and dataset hash) opens the comparison.
    await d.getByRole('link', { name: /Liquidity depletion vs normal, comparison/ }).click();
    await page.waitForURL(/\/compare\/pair\?a=253bbc23b342f373&b=88e0971baa6491ab$/);
    await page.goBack();
    await ready(page);
  });

  test('reachable from Explore, the rail and the command palette; studies have their own title', async ({ page }) => {
    await page.goto('/');
    await page.getByText('Research views', { exact: true }).click();
    await page.getByRole('link', { name: 'Research home: five investigations' }).click();
    await page.waitForURL(/\/research$/);
    await page.goBack();
    await page.waitForURL((u) => u.pathname === '/');

    await page.keyboard.press('Control+k');
    const palette = page.getByRole('combobox', { name: 'Search or run a command' });
    await palette.fill('Execution studies');
    await palette.press('Enter');
    await page.waitForURL(/\/studies$/);
    await expect(page).toHaveTitle(/^Execution studies \u00b7 Studies \u2014 BlackBox HFT$/);

    await page.getByRole('link', { name: 'Research home' }).first().click();
    await page.waitForURL(/\/research$/);
    await ready(page);
  });

  test('a tampered study is refused in every row that reads it; the workbench row still answers', async ({ page }) => {
    await page.route(STUDY, async (route) => {
      const response = await route.fetch();
      const body = Buffer.from(await response.body());
      body[body.length - 2] = body[body.length - 2] === 0x30 ? 0x31 : 0x30;
      await route.fulfill({ response, body });
    });
    await page.goto('/research');
    await expect(row(page, 'l2-queue')).toHaveAttribute('data-state', 'ready');
    for (const id of ['fees', 'latency', 'regime', 'ranking']) {
      const r = row(page, id);
      await expect(r).toHaveAttribute('data-state', 'failed');
      await expect(r.getByTestId('rh-source-error')).toHaveAttribute('data-kind', 'hash_mismatch');
      await expect(r.getByTestId('rh-source-error')).toContainText('Content hash mismatch');
      await expect(r.getByTestId('rh-result')).toHaveCount(0);
      await expect(r.getByTestId('rh-link-primary')).toHaveCount(0);
    }
    await shot(page, 'research-home-hash-mismatch');
  });

  test.describe('workbench unavailable', () => {
    test.use({ allowStatus: [503] });

    test('a busy workbench API is shown as such, with no number from it', async ({ page }) => {
      await page.route('**/api/v1/market/datasets/l3-demo/examples', (route) =>
        route.fulfill({
          status: 503,
          contentType: 'application/json',
          body: JSON.stringify({ error: { code: 'unavailable', message: 'The heavy-read capacity is busy; retry shortly.' } }),
        }),
      );
      await page.goto('/research');
      const a = row(page, 'l2-queue');
      await expect(a).toHaveAttribute('data-state', 'failed');
      await expect(a.getByTestId('rh-source-error')).toContainText('The heavy-read capacity is busy; retry shortly.');
      await expect(a.getByTestId('rh-result')).toHaveCount(0);
      await expect(a).not.toHaveAttribute('data-l2-delta');
      await expect(row(page, 'fees')).toHaveAttribute('data-state', 'ready');
    });
  });
});
