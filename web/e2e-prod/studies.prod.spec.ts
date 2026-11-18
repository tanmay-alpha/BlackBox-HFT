// Production check of the execution study viewer (Phase 6 G2, Phase 7 H2), anonymous and
// read-only, one page load. The artefact and its `.sha256` file are fetched by the test itself
// from the UI origin and compared with what the page shows.
//
// Relies on (all present on p6/int, chosen to survive the cell explorer): routes `/studies`,
// `/studies/exec-validation`, `/forensics/{run}`; test ids `study-index`, `study-view`
// (`data-study-id`), `study-sha256`, `study-matrix-{regime}`, `study-cells`; the `data-link`
// attribute (`stored` | `not_stored`) on every run row; a stored run's link to `/forensics/{run}`;
// the "Show every run with its hashes and reproduction command" and "Reproduce locally"
// disclosures. Matrix cells are found by position (queue model row, latency column, in the
// artefact's row order) and matched with `toContainText`, so added cell controls do not matter.

import { createHash } from 'node:crypto';

import { WEB_URL } from './env';
import { api, expect, load, test } from './prod';

const ARTEFACT = `${WEB_URL}/study-results/exec-validation/study-result.json`;
const RUN_HREF = /^\/forensics\/([0-9a-f]{16})$/;

type Metric =
  | { kind: 'deterministic_single'; value: number }
  | { kind: 'stochastic_seed_mean'; bootstrap: { mean: number; lower_95: number; upper_95: number } | null };
interface Study {
  study_id: string;
  schema: string;
  schema_version: number;
  sensitivity_surfaces: { stratum: { regime: string }; rows: { queue_model: string; latency: string; metrics: Record<string, Metric> }[] }[];
}

/** The viewer's print of a net P&L point: 4 decimals, the 95% interval for seed means. */
function point(m: Metric | undefined): string {
  const f = (v: number) => v.toFixed(4);
  if (m?.kind === 'deterministic_single') return f(m.value);
  if (m?.bootstrap) return `${f(m.bootstrap.mean)} [${f(m.bootstrap.lower_95)}, ${f(m.bootstrap.upper_95)}]`;
  return 'not in design';
}

test('exec-validation: verified hash, matrix values from the artefact, a stored run opens, unstored runs reproduce', async ({ page, request }) => {
  const [jsonRes, shaRes] = await Promise.all([request.get(ARTEFACT), request.get(`${ARTEFACT}.sha256`)]);
  expect(jsonRes.status()).toBe(200);
  expect(shaRes.status()).toBe(200);
  const bytes = await jsonRes.body();
  const sha256 = createHash('sha256').update(bytes).digest('hex');
  expect((await shaRes.text()).trim().split(/\s+/)[0], 'the published .sha256 matches the artefact').toBe(sha256);
  const study = JSON.parse(bytes.toString('utf8')) as Study;

  await load(page, '/studies');
  await expect(page.getByRole('heading', { level: 1, name: 'Execution studies' })).toBeVisible();
  await page.getByTestId('study-index').getByRole('link', { name: 'Execution model validation' }).click();
  await expect(page).toHaveURL(/\/studies\/exec-validation$/);

  const view = page.getByTestId('study-view');
  await expect(view).toHaveAttribute('data-study-id', study.study_id);
  const badge = page.getByTestId('study-sha256');
  await expect(badge).toContainText('verified in this browser');
  await expect(badge.locator('[title]').first()).toHaveAttribute('title', sha256);
  await expect(page.getByTestId('study-identity')).toContainText(`${study.schema} v${study.schema_version}`);

  // Every cell of the first sensitivity matrix (net P&L, the default measure) is the artefact's.
  const surface = study.sensitivity_surfaces[0];
  const queues = [...new Set(surface.rows.map((r) => r.queue_model))];
  const latencies = [...new Set(surface.rows.map((r) => r.latency))];
  const matrix = page.getByTestId(`study-matrix-${surface.stratum.regime}`);
  await expect(matrix).toBeVisible();
  for (const [qi, q] of queues.entries()) {
    for (const [li, l] of latencies.entries()) {
      const expected = point(surface.rows.find((r) => r.queue_model === q && r.latency === l)?.metrics.net_pnl);
      await expect(matrix.locator('tbody tr').nth(qi).locator('td').nth(li), `${q} x ${l}`).toContainText(expected);
    }
  }

  // Run links are settled once the stored public runs were checked; a stored run links to its
  // forensics route.
  const runLink = 'a[href^="/forensics/"]:not([href*="/orders/"])';
  const linked = page.locator('[data-link="stored"]').filter({ has: page.locator(runLink) }).first();
  await expect(linked).toBeVisible();
  await expect(page.locator('[data-link="not_stored"]', { hasText: 'Stored public runs not checked' })).toHaveCount(0);

  // Unstored runs offer the reproduction command and no link.
  await page.getByText('Show every run with its hashes and reproduction command').click();
  const cell = page.getByTestId('study-cells').locator('tr[data-link="not_stored"]').first();
  await expect(cell).toBeVisible();
  await cell.getByText('Reproduce locally').click();
  await expect(cell.locator('pre')).toContainText(`blackbox study ${study.study_id}`);
  const unstored = await page.locator('[data-link="not_stored"]').evaluateAll((rows) =>
    rows.map((r) => ({ links: r.querySelectorAll('a').length, reproduce: (r.textContent ?? '').includes('Reproduce locally') })),
  );
  expect(unstored.length, 'unstored runs').toBeGreaterThan(0);
  expect(unstored.filter((r) => r.links > 0 || !r.reproduce), 'unstored rows with a link or without the reproduce text').toEqual([]);

  // The stored run opens on its forensics route, and the API has that run.
  const link = linked.locator(runLink).first();
  const href = (await link.getAttribute('href')) ?? '';
  const runId = RUN_HREF.exec(href)?.[1];
  expect(runId, `run link ${href}`).toBeDefined();
  const run = await api<{ run_id: string; label: string | null }>(request, `/runs/${runId}`);
  expect(run.run_id).toBe(runId);
  await link.click();
  await expect(page).toHaveURL(new RegExp(`/forensics/${runId}$`));
  // The heading names the run by its stored label, without the experiment id prefix.
  if (run.label) await expect(page.getByRole('heading', { level: 1 })).toContainText(run.label.replace(/^[a-z_]+: /, ''));
  await expect(page.locator(`a[href^="/forensics/${runId}/orders/"]`).first()).toBeVisible();
  await expect(page.locator('.error-box')).toHaveCount(0);
  await page.goBack();
  await expect(view).toBeVisible();
});
