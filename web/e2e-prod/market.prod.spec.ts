// Production check of the Market Data Workbench on the synthetic `l3-demo` dataset (Phase 6 G1,
// Phase 7 H1), anonymous and read-only, three page loads. Every number asserted on screen is
// compared with the API response this test fetched itself:
//
// * dataset summary (`/market/datasets/l3-demo`): the synthetic provenance badge, the event count
//   and every validation figure (overall outcome, quarantined messages, each check's count);
// * curated examples (`…/examples`): the example opened from the "Curated examples" navigation;
// * event detail (`…/events/{i}?levels=5&orders_per_level=50`, the book step's own query): the
//   event line, every listed L2 level total and order count before and after the event, the L2
//   statement and the L3 queue of each changed level;
// * events (`…/events?from_event_index=i+1&limit=1`): where J (next) lands;
// * metrics (`…/metrics?max_points=300`) and comparison (`…/comparison`): the counts table, the
//   queue position at execution and the L3 -> L2 loss counts.
//
// Also: K (previous) returns, Back restores the book event after the metrics step, an event index
// beyond the dataset shows the bounded "Out of range" state, and 390x844 has no horizontal overflow.

import { api, expect, load, test } from './prod';

const DATASET = 'l3-demo';
const ROOT = `/market/${DATASET}`;
const BADGE = 'synthetic \u2014 generated, not market data';
/** The book step's event-detail query (`EventCompare`: 5 levels per side, 50 orders per level). */
const DETAIL_QUERY = 'levels=5&orders_per_level=50';

interface Check {
  id: string;
  count: number;
}
interface Summary {
  data_class: string;
  events: number;
  quality: { passed: boolean; quarantined_total: number; checks: Check[] };
}
interface Example {
  id: string;
  title: string;
  event_index: number;
}
interface Order {
  order_ref: string;
  shares: number;
  queue_position: number;
}
interface Level {
  price4: number;
  shares: number;
  order_count: number;
  orders: Order[];
}
interface Detail {
  event: { event_index: number; message_type: string; order_ref: string | null; shares: number | null; price4: number | null };
  before: { bids: Level[]; asks: Level[] };
  after: { bids: Level[]; asks: Level[] };
  affected: { side: 'buy' | 'sell'; price4: number; role: string; before: Level; after: Level }[];
}
interface Distribution {
  n: number;
  min: number | null;
  p50: number | null;
  p90: number | null;
  max: number | null;
}
interface Metrics {
  counts: Record<string, number>;
  queue_position_at_execution: Distribution;
}
interface Comparison {
  execution_messages: number;
  cancel_messages: number;
}

// The workbench's number formats: thousands commas, U+2212 minus, price4 with four decimals.
const group = (digits: string) => digits.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
const int = (n: number) => `${n < 0 ? '\u2212' : ''}${group(String(Math.abs(Math.trunc(n))))}`;
const price = (p: number) =>
  `${p < 0 ? '\u2212' : ''}${group(String(Math.floor(Math.abs(p) / 10_000)))}.${String(Math.abs(p) % 10_000).padStart(4, '0')}`;
const exact = (text: string) => new RegExp('^' + text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '$');

/** `before → after` as the queue table prints a pair. */
function pair(b: number | undefined, a: number | undefined): string {
  if (b === undefined && a === undefined) return '\u2014';
  if (b === undefined) return `\u2014 \u2192 ${int(a!)}`;
  if (a === undefined) return `${int(b)} \u2192 \u2014`;
  return b === a ? int(b) : `${int(b)} \u2192 ${int(a)}`;
}

const ROLE: Record<string, string> = {
  order: 'changed level',
  replaced: 'replaced in place',
  replaced_from: 'replaced from',
  replaced_to: 'replaced to',
};

test('l3-demo: provenance, validation, a curated example against the event API, J/K, metrics and Back', async ({ page, request }) => {
  const summary = await api<Summary>(request, `/market/datasets/${DATASET}`);
  const { items: examples } = await api<{ items: Example[] }>(request, `/market/datasets/${DATASET}/examples`);
  expect(summary.data_class).toBe('synthetic');
  expect(examples.length, 'curated examples').toBeGreaterThan(1);

  await load(page, ROOT);
  await expect(page.getByText(BADGE).first()).toBeVisible();
  await expect(page.locator('.mk-meta')).toContainText(`${int(summary.events)} events`);

  // Validation: every figure is the summary's.
  await page.getByRole('tab', { name: 'Validation' }).click();
  await expect(page).toHaveURL(/[?&]tab=validation(&|$)/);
  await expect(page.getByLabel('Check outcomes')).toContainText(summary.quality.passed ? /Overall\s*passed/ : /Overall\s*not passed/);
  await expect(page.locator('dt:text-is("Quarantined messages") + dd')).toHaveText(int(summary.quality.quarantined_total));
  await expect(page.getByRole('heading', { name: `Checks (${int(summary.quality.checks.length)})` })).toBeVisible();
  for (const c of summary.quality.checks) {
    const item = page.locator('li.mk-check').filter({ has: page.locator('span.mono', { hasText: exact(c.id) }) });
    await expect(item.locator('[title="Occurrences"]'), c.id).toHaveText(int(c.count));
  }

  // Order book: it opens on the first curated example; open the second from the navigation.
  await page.getByRole('tab', { name: 'Order book' }).click();
  await expect(page).toHaveURL(new RegExp(`[?&]event=${examples[0].event_index}(&|$)`));
  const x = examples[1];
  await page.getByRole('navigation', { name: 'Curated examples' }).getByRole('link', { name: new RegExp(x.title) }).click();
  await expect(page).toHaveURL(new RegExp(`[?&]event=${x.event_index}(&|$)`));
  await expect(page).toHaveURL(new RegExp(`[?&]example=${x.id}(&|$)`));

  const d = await api<Detail>(request, `/market/datasets/${DATASET}/events/${x.event_index}?${DETAIL_QUERY}`);
  const facts = page.locator('.mk-cmp-event .mk-event-facts');
  await expect(facts).toContainText(`#${int(x.event_index)} ${d.event.message_type}`);
  if (d.event.shares !== null) await expect(facts).toContainText(int(d.event.shares));
  if (d.event.price4 !== null) await expect(facts).toContainText(`@ ${price(d.event.price4)}`);
  if (d.event.order_ref !== null) await expect(facts).toContainText(`ref ${d.event.order_ref}`);

  // L2: every listed level's total and order count, before (column 1) and after (column 2).
  const ladder = page.locator('.mk-ladder');
  await expect(ladder).toBeVisible();
  for (const [side, key] of [
    ['bid', 'bids'],
    ['ask', 'asks'],
  ] as const) {
    for (const [when, column] of [
      ['before', 0],
      ['after', 1],
    ] as const) {
      for (const lv of d[when][key]) {
        const row = ladder.locator(`tbody tr.mk-r-${side}`).filter({ has: page.locator('th', { hasText: exact(price(lv.price4)) }) });
        // Visible "shares · count", plus the visually hidden words that make it read as a sentence.
        const text = `${int(lv.shares)} shares in${int(lv.order_count)} ${lv.order_count === 1 ? 'order' : 'orders'}`;
        await expect(row.locator('td').nth(column), `${side} ${price(lv.price4)} ${when}`).toHaveText(text);
      }
    }
  }

  // Each changed level: its L2 statement, its order counts and the event's order in its L3 queue.
  expect(d.affected.length, 'the example changes at least one level').toBeGreaterThan(0);
  const statements = page.locator('.mk-obs-list');
  for (const lv of d.affected) {
    const label = `${lv.side === 'buy' ? 'bid' : 'ask'} ${price(lv.price4)}`;
    await expect(statements).toContainText(`${label}: ${int(lv.before.shares)} \u2192 ${int(lv.after.shares)} shares`);
    const block = page.getByRole('region', { name: `${label} queue` }).filter({ hasText: ROLE[lv.role] ?? lv.role });
    await expect(block.locator('h3')).toContainText(`${int(lv.before.order_count)} \u2192 ${int(lv.after.order_count)} orders`);
    const ref = d.event.order_ref;
    const b = lv.before.orders.find((o) => o.order_ref === ref);
    const a = lv.after.orders.find((o) => o.order_ref === ref);
    if (ref === null || (b === undefined && a === undefined)) continue;
    const row = block.locator('tbody tr').filter({ has: page.locator('th', { hasText: exact(ref) }) });
    await expect(row.locator('td').nth(0), `queue # of ${ref}`).toHaveText(pair(b?.queue_position, a?.queue_position));
    await expect(row.locator('td').nth(2), `shares of ${ref}`).toHaveText(pair(b?.shares, a?.shares));
  }

  // J lands on the next event the events API lists; K and the buttons follow it.
  const next = (await api<{ items: { event_index: number }[] }>(request, `/market/datasets/${DATASET}/events?from_event_index=${x.event_index + 1}&limit=1`)).items[0]?.event_index;
  expect(next, 'a later event exists').toBeDefined();
  const at = async (index: number) => {
    await expect(page).toHaveURL(new RegExp(`[?&]event=${index}(&|$)`));
    await expect(facts).toContainText(`#${int(index)} `);
  };
  await ladder.click();
  await page.keyboard.press('j');
  await at(next!);
  await page.keyboard.press('k');
  await at(x.event_index);
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  await at(next!);
  await page.getByRole('button', { name: 'Previous', exact: true }).click();
  await at(x.event_index);

  // Research metrics: the counts, queue position and loss figures are the server's.
  const [metrics, loss] = await Promise.all([
    api<Metrics>(request, `/market/datasets/${DATASET}/metrics?max_points=300`),
    api<Comparison>(request, `/market/datasets/${DATASET}/comparison`),
  ]);
  await page.getByRole('tab', { name: 'Research metrics' }).click();
  await expect(page).toHaveURL(/[?&]tab=metrics(&|$)/);
  const counts = page.getByRole('grid', { name: 'Message counts and rates' });
  for (const [key, count] of Object.entries(metrics.counts)) {
    const row = counts.getByRole('row').filter({ has: page.getByRole('rowheader', { name: key.replaceAll('_', ' '), exact: true }) });
    await expect(row.locator('td').first(), key).toHaveText(int(count));
  }
  const q = metrics.queue_position_at_execution;
  await expect(page.locator('dt:text-is("Queue position at execution") + dd')).toHaveText(
    q.n > 0 ? `n ${int(q.n)} \u00b7 min ${int(q.min!)} \u00b7 p50 ${int(q.p50!)} \u00b7 p90 ${int(q.p90!)} \u00b7 max ${int(q.max!)}` : 'no execution',
  );
  const lossGrid = page.getByRole('grid', { name: 'L3 to L2 information loss' });
  for (const [measure, value] of [
    ['Execution messages', loss.execution_messages],
    ['Cancellations (X and D)', loss.cancel_messages],
  ] as const) {
    const row = lossGrid.getByRole('row').filter({ has: page.getByRole('rowheader', { name: measure, exact: true }) });
    await expect(row.locator('td').first(), measure).toHaveText(int(value));
  }

  // Back restores the book at the selected event.
  await page.goBack();
  await expect(page).toHaveURL(/[?&]tab=book(&|$)/);
  await at(x.event_index);
  await expect(ladder).toBeVisible();
});

test('an event index beyond the dataset shows the bounded out-of-range state', async ({ page, request }) => {
  const summary = await api<Summary>(request, `/market/datasets/${DATASET}`);
  await load(page, `${ROOT}?tab=book&event=999999999`);
  await expect(page.getByRole('note').filter({ hasText: 'Out of range' })).toContainText(
    `Event #999,999,999 is beyond this dataset, whose event indices run from 0 to ${int(summary.events - 1)}.`,
  );
  await expect(page.getByRole('button', { name: 'Book after event #0' })).toBeVisible();
  await expect(page.getByRole('button', { name: `Book after the last event (#${int(summary.events - 1)})` })).toBeVisible();
  await expect(page.locator('.mk-ladder')).toHaveCount(0);
  await expect(page.locator('.error-box')).toHaveCount(0);
});

test.describe('390x844', () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test('a curated example fits without horizontal overflow and keeps the provenance visible', async ({ page, request }) => {
    const { items: examples } = await api<{ items: Example[] }>(request, `/market/datasets/${DATASET}/examples`);
    const x = examples[1];
    await load(page, `${ROOT}?example=${x.id}`);
    await expect(page).toHaveURL(new RegExp(`[?&]event=${x.event_index}(&|$)`));
    await expect(page.locator('.mk-cmp-event .mk-event-facts')).toContainText(`#${int(x.event_index)} `);
    await expect(page.locator('.mk-examples select')).toHaveValue(x.id);
    await expect(page.locator('.mk-ladder')).toBeVisible();
    await expect(page.getByText(BADGE).first()).toBeVisible();
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow).toBeLessThanOrEqual(0);
  });
});
