// Anonymous visitor: the public demo workspace, read-only. Every stored result can be opened;
// every mutating control is visible, locked, and opens the key dialog instead of acting.

import { expect, shot, test } from './fixtures';

interface RunItem {
  run_id: string;
}

test('anonymous read-only story: overview, forensics, risk, compare, report, system', async ({ page, request }) => {
  // A fresh database: `demo seed` queued the demo experiments; the in-process worker computes them.
  let items: RunItem[] = [];
  await expect
    .poll(
      async () => {
        const res = await request.get('/api/v1/runs?workspace=demo&limit=10');
        items = res.ok() ? ((await res.json()) as { items: RunItem[] }).items : [];
        return items.length;
      },
      { timeout: 150_000, intervals: [2_000] },
    )
    .toBeGreaterThanOrEqual(2);
  const [a, b] = items;

  // `/` is the latency experiment (explore.spec.ts); the experiments catalogue is `/experiments`.
  await page.goto('/');
  await expect(page.getByRole('heading', { level: 1, name: /What changes when a market-making strategy/ })).toBeVisible();
  await page.goto('/experiments');
  await expect(page.getByTestId('access-badge')).toContainText('DEMO');
  await expect(page.getByTestId('sb-mode')).toContainText('DEMO read-only');
  await expect(page.locator('.sb-api')).toContainText('API ok');
  await expect(page.getByTestId('sb-worker')).toContainText('worker');
  await expect(page.getByRole('grid', { name: 'Recent runs' }).or(page.getByRole('table', { name: 'Recent runs' }))).toBeVisible();
  await shot(page, '01-overview-demo');

  // Strategy Lab: the submit button is locked and opens the key dialog.
  await page.goto('/strategy');
  const submit = page.getByTestId('lab-submit');
  await expect(submit).toHaveAttribute('aria-disabled', 'true');
  await expect(submit).toHaveAttribute('data-locked', 'true');
  await shot(page, '02-strategy-lab-demo');
  // aria-disabled (not `disabled`) keeps it focusable; Playwright treats it as disabled, so force.
  await submit.click({ force: true });
  await expect(page.getByRole('dialog', { name: 'Research access' })).toBeVisible();
  await page.keyboard.press('Escape');

  // Execution Forensics of a demo run.
  await page.goto(`/forensics/${a.run_id}`);
  await expect(page.getByText(a.run_id.slice(0, 12)).first()).toBeVisible();
  await shot(page, '03-forensics-run');

  // Risk & Performance.
  await page.goto(`/risk/${a.run_id}`);
  await expect(page.getByRole('heading', { name: 'Run metrics' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Risk analytics' })).toBeVisible();
  await shot(page, '04-risk-performance');

  // Experiment Comparison: either a stored comparison or a locked "Create comparison".
  await page.goto(`/compare/pair?a=${a.run_id}&b=${b.run_id}`);
  await expect(page).toHaveURL(new RegExp(`a=${a.run_id}`));
  await expect(page.getByText(a.run_id.slice(0, 12)).first()).toBeVisible();
  await shot(page, '05-compare-pair');

  // Research report with its manifest.
  await page.goto(`/reports/${a.run_id}`);
  await expect(page.getByRole('article')).toContainText('BlackBox research report');
  await expect(page.getByTestId('report-manifest')).toContainText(a.run_id);
  await expect(page.getByTestId('save-report')).toHaveAttribute('aria-disabled', 'true');
  await shot(page, '06-research-report');

  // System health from /status, readiness on demand.
  await page.goto('/system');
  await expect(page.getByTestId('system-build')).toBeVisible();
  await page.getByTestId('check-ready').click();
  await expect(page.getByRole('row', { name: /Database/ })).toBeVisible();
  await shot(page, '07-system');

  // Simulation Workbench is readable; nothing can be cancelled without a key.
  await page.goto('/sim?state=all');
  await expect(page.getByRole('heading', { name: 'Job queue' })).toBeVisible();
  await shot(page, '08-workbench-demo');
});
