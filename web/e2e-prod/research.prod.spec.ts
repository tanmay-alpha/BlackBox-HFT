// Production research journey (one bounded experiment) against the deployed frontend and API.
// Run only on purpose: `PROD_RESEARCH_KEY=… npx playwright test -c playwright.prod.config.ts
// e2e-prod/research.prod.spec.ts`. The key is read from the environment, typed into the access
// dialog and never logged. Evidence (ids, states, timings) is written to
// `../docs/rebuild/prod-research-evidence.json`.

import { writeFileSync } from 'node:fs';

import { expect, test } from '@playwright/test';

const API = process.env.PROD_API_URL ?? 'https://blackbox-hft-api.onrender.com/api/v1';
const ORIGIN = process.env.PROD_URL ?? 'https://blackbox-hft.vercel.app';
const KEY = process.env.PROD_RESEARCH_KEY ?? '';
const DEMO_RUN = '253bbc23b342f373';

test.skip(!KEY, 'PROD_RESEARCH_KEY is not set');

test('bounded keyed experiment: submit, complete, inspect, compare, report, reload, sign out', async ({ page, request }) => {
  test.setTimeout(600_000);
  const evidence: Record<string, unknown> = { started_at: new Date().toISOString() };
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));

  const signIn = async () => {
    await page.getByTestId('access-badge').click();
    const dialog = page.getByRole('dialog', { name: 'Research access' });
    await dialog.getByLabel('API key').fill(KEY);
    await dialog.getByRole('button', { name: 'Use key' }).click();
    await expect(page.getByTestId('access-badge')).toContainText('RESEARCH');
  };

  const status = await (await request.get(`${API}/status`, { headers: { Origin: ORIGIN } })).json();
  evidence.backend_git_sha = status.git_sha;

  // 1–6: Strategy Lab, key, dataset, preset, a small change, validation.
  await page.goto('/strategy?dataset=synthetic-demo');
  await signIn(page);
  await page.getByRole('radio', { name: 'Touch maker: baseline', exact: true }).check();
  const entry = page.getByLabel('Entry latency', { exact: true });
  await entry.fill('5');
  await entry.blur();
  const changed = page.getByRole('region', { name: 'Changed vs preset' });
  await expect(changed).toContainText('5 ms');
  await expect(page.getByRole('region', { name: 'Validation' })).toContainText('valid');
  const seed = 2_026_111_500 + Math.floor(Math.random() * 1_000_000);
  evidence.seed = seed;
  await page.getByRole('group', { name: 'Form mode' }).getByRole('button', { name: 'Advanced' }).click();
  await page.getByRole('tab', { name: /Seed/ }).click();
  await page.getByLabel('Seed', { exact: true }).and(page.locator('input')).fill(String(seed));
  await page.getByLabel('Run label').fill(`prod acceptance ${seed}`);
  await page.screenshot({ path: '../docs/rebuild/img/prodj-01-lab.png' });

  // 7–10: submit; observe states until completed.
  const t0 = Date.now();
  await page.getByTestId('lab-submit').click();
  await page.getByTestId('lab-next-job').click();
  await page.waitForURL(/\/sim\/jobs\//);
  const jobId = page.url().split('/sim/jobs/')[1].split(/[?#]/)[0];
  evidence.job_id = jobId;
  const detail = page.getByTestId('job-detail');
  const states: { state: string | null; at_ms: number }[] = [];
  for (;;) {
    const s = await detail.getAttribute('data-job-state');
    if (states.at(-1)?.state !== s) states.push({ state: s, at_ms: Date.now() - t0 });
    if (s === 'completed' || s === 'failed' || s === 'cancelled') break;
    if (Date.now() - t0 > 300_000) break;
    await page.waitForTimeout(500);
  }
  evidence.ui_states = states;
  await expect(detail).toHaveAttribute('data-job-state', 'completed');
  const runId = ((await detail.locator('.job-run-links a').first().textContent()) ?? '').trim();
  expect(runId).toMatch(/^[0-9a-f]{16}$/);
  evidence.run_id = runId;
  await page.screenshot({ path: '../docs/rebuild/img/prodj-02-job-completed.png' });

  const auth = { Origin: ORIGIN, Authorization: `Bearer ${KEY}` };
  const job = await (await request.get(`${API}/jobs/${jobId}`, { headers: auth })).json();
  evidence.job = {
    state: job.state, attempt: job.attempt, queued_at_ns: job.queued_at_ns, started_at_ns: job.started_at_ns,
    finished_at_ns: job.finished_at_ns, progress: job.progress, platform: job.platform,
  };
  const runRes = await request.get(`${API}/runs/${runId}`, { headers: auth });
  const run = await runRes.json();
  evidence.run = {
    status: runRes.status(), result_sha256: run.result_sha256, config_sha256: run.config_sha256,
    platform: run.platform, fills: run.metrics?.fill_count, orders: run.metrics?.orders_submitted, net_pnl: run.metrics?.net_pnl,
  };

  // 11–14: run, orders, replay, one order's forensics.
  await page.goto(`/forensics/${runId}`);
  await expect(page.getByRole('heading', { name: /Order blotter/i })).toBeVisible();
  await page.locator(`a[href^="/forensics/${runId}/orders/"]`).first().click();
  await expect(page).toHaveURL(new RegExp(`/forensics/${runId}/orders/\\d+`));
  await page.screenshot({ path: '../docs/rebuild/img/prodj-03-order.png' });
  await page.goto(`/book/runs/${runId}`);
  await expect(page.getByText('Depth at cursor').first()).toBeVisible({ timeout: 60_000 });
  await expect(page.locator('.error-box')).toHaveCount(0);
  await page.screenshot({ path: '../docs/rebuild/img/prodj-04-replay.png' });

  // 15: comparison with the public demo run (aligned; the configs differ in latency and seed).
  const cmpRes = await request.post(`${API}/comparisons`, { headers: auth, data: { run_a: DEMO_RUN, run_b: runId } });
  const cmp = await cmpRes.json();
  evidence.comparison = { status: cmpRes.status(), id: cmp.id, eligibility: cmp.eligibility, diffs: (cmp.config_diffs ?? []).map((d: { path: string }) => d.path) };
  expect([200, 201]).toContain(cmpRes.status());

  // 16: report.
  const rep = await request.get(`${API}/runs/${runId}/report`, { headers: auth });
  const text = await rep.text();
  evidence.report = { status: rep.status(), bytes: text.length, has_title: text.includes('BlackBox research report') };
  expect(rep.status()).toBe(200);

  // 17–18: reload, persistence.
  await page.goto(`/forensics/${runId}`);
  await page.reload();
  await expect(page.getByTestId('access-badge')).toContainText('RESEARCH');
  await expect(page.getByText(runId.slice(0, 12)).first()).toBeVisible();

  // 19–21: sign out; private run hidden in the UI and refused anonymously by the API.
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByTestId('access-badge')).toContainText('DEMO');
  await expect(page.getByText(`Run ${runId} was not found`)).toBeVisible();
  await page.screenshot({ path: '../docs/rebuild/img/prodj-05-signed-out.png' });
  const anon = await request.get(`${API}/runs/${runId}`, { headers: { Origin: ORIGIN } });
  evidence.anonymous_run_status = anon.status();
  expect(anon.status()).toBe(404);

  // 22: sign in again; access restored.
  await signIn(page);
  await expect(page.getByRole('heading', { name: /Order blotter/i })).toBeVisible();
  evidence.reauth_ok = true;

  evidence.page_errors = errors;
  evidence.finished_at = new Date().toISOString();
  writeFileSync('../docs/rebuild/prod-research-evidence.json', JSON.stringify(evidence, null, 2));
  expect(errors).toEqual([]);
});
