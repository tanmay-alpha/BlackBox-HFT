// Production check of the curated execution_validation set on Experiment Comparison -> Set
// (Phase 6 A1/A2), anonymous and read-only, one page load. The test classifies every run against
// the baseline from the API itself (`/demos`, the stored comparison list) and requires the page to
// show the same state per run: three aligned, two metrics-only, one compared elsewhere, none
// missing. The page must read no run by id (`GET /api/v1/runs/{id}` verifies a whole bundle on
// the server). A busy server (503) may leave a pair "unavailable: server busy"; that is accepted
// only as the explicit busy state with Retry, in place of a stored comparison, never as missing.

import { api, expect, load, test } from './prod';

const DEMO = 'execution_validation';
const DETAIL_RUN = /\/api\/v1\/runs\/[0-9a-f]{16}(?:\/|$)/;

interface Demo {
  id: string;
  runs: { run_id: string | null }[];
}
interface ComparisonSummary {
  run_a: string;
  run_b: string;
  status: string;
}
interface CursorPage<T> {
  items: T[];
  next_cursor: string | null;
}

/** The page's state for `other` against `base`, from the stored comparisons (`pairStatus`). */
function classify(list: ComparisonSummary[], base: string, other: string): string {
  const direct = list.find((c) => c.run_a === base && c.run_b === other);
  if (direct) return direct.status;
  if (list.some((c) => c.run_a === other && c.run_b === base)) return 'reversed';
  if (list.some((c) => (c.run_a === other || c.run_b === other) && c.run_a !== base && c.run_b !== base)) return 'elsewhere';
  return 'missing';
}

test.use({ allowStatus: [503] });

test('execution_validation: each pair state matches the API classification, no run read by id', async ({ page, request }) => {
  const demos = await api<Demo[]>(request, '/demos');
  const ids = (demos.find((d) => d.id === DEMO)?.runs ?? []).flatMap((r) => (r.run_id ? [r.run_id] : []));
  expect(ids.length, 'the experiment has a baseline and six runs stored').toBe(7);
  const list: ComparisonSummary[] = [];
  let cursor: string | null = null;
  // The page reads at most 1,000 comparisons (20 pages of 50); so does this lookup.
  for (let pages = 0; pages < 20; pages += 1) {
    const res: CursorPage<ComparisonSummary> = await api(request, `/comparisons?limit=50${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ''}`);
    list.push(...res.items);
    cursor = res.next_cursor;
    if (cursor === null) break;
  }
  expect(cursor, 'the comparison list was read to its end').toBeNull();
  const [base, ...others] = ids;
  const expected = new Map(others.map((id) => [id, classify(list, base, id)]));
  const tally = (state: string) => [...expected.values()].filter((s) => s === state).length;
  expect([tally('aligned'), tally('metrics_only'), tally('elsewhere')], 'API: aligned, metrics only, elsewhere').toEqual([3, 2, 1]);

  const byId: string[] = [];
  page.on('request', (r) => {
    const path = new URL(r.url()).pathname;
    if (r.method() === 'GET' && DETAIL_RUN.test(path)) byId.push(path);
  });
  await load(page, `/compare/set?demo=${DEMO}`);
  const grid = page.getByRole('grid', { name: 'Run set' });
  await expect(grid).toBeVisible();
  await expect(page.locator('[data-pair-state]')).toHaveCount(6);
  await expect(page.locator('[data-pair-state="loading"]')).toHaveCount(0);

  let busy = 0;
  for (const [id, want] of expected) {
    const state = grid.getByRole('row').filter({ has: page.locator(`a[href="/forensics/${id}"]`) }).locator('[data-pair-state]');
    const got = await state.getAttribute('data-pair-state');
    if (got === 'busy' && (want === 'aligned' || want === 'metrics_only')) busy += 1;
    else expect(got, `run ${id}`).toBe(want);
  }
  await expect(page.locator('[data-pair-state="missing"], [data-pair-state="unlisted"]')).toHaveCount(0);
  await expect(page.getByRole('button', { name: /missing comparison/ })).toHaveCount(0);
  await expect(page.getByText(/no stored comparison with the baseline \(A =/)).toHaveCount(0);
  await expect(page.getByText('Compared elsewhere', { exact: true })).toBeVisible();
  if (busy > 0) {
    test.info().annotations.push({ type: 'busy', description: `${busy} pair(s) unavailable: server busy` });
    await expect(page.locator('[data-pair-state="busy"]').first()).toContainText('unavailable: server busy');
    await expect(page.getByRole('button', { name: 'Retry' }).first()).toBeVisible();
  }

  expect(byId, 'GET /api/v1/runs/{id} requests made by the page').toEqual([]);
});
