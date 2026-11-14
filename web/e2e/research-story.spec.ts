import { readFile } from 'node:fs/promises';
import type { Download, Page, Response } from '@playwright/test';
import type { Comparison, Demo, OrderDetail, RunConfig, RunResult, RunSummary } from '../src/api/types';
import { test, expect } from './fixtures';

function panel(page: Page, title: string) {
  return page.locator('section').filter({ has: page.getByRole('heading', { name: title, exact: true }) });
}

async function open(page: Page, path: string) {
  await page.waitForLoadState('networkidle');
  await page.goto(path);
  await page.waitForLoadState('networkidle');
}

async function downloadedText(download: Download, extension: string) {
  expect(download.suggestedFilename()).toMatch(new RegExp(`\\.${extension}$`));
  expect(await download.failure()).toBeNull();
  const path = await download.path();
  expect(path).not.toBeNull();
  return readFile(path!, 'utf8');
}

test('synthetic research: demo, replay, forensics, comparison, exports, reproducibility, validation', async ({ page, request }) => {
  let demo: Demo;
  let runs: RunSummary[];
  let first: RunSummary;
  let firstConfig: RunConfig;
  let ids: string[];

  await test.step('Overview runs the cost_of_latency pair; Experiment Comparison explains its controls and first divergence', async () => {
    await open(page, '/');
    await expect(page).toHaveTitle(/^Overview/);
    const datasets = await request.get('/api/datasets');
    expect(datasets.ok()).toBeTruthy();
    expect((await datasets.json()).map((d: { id: string }) => d.id)).toEqual(['synthetic-demo']);
    const demosResponse = await request.get('/api/demos?dataset_id=synthetic-demo');
    expect(demosResponse.ok()).toBeTruthy();
    demo = ((await demosResponse.json()) as Demo[]).find((d) => d.id === 'cost_of_latency')!;
    expect(demo).toBeDefined();
    expect(demo.runs).toHaveLength(2);
    const posts: Promise<RunSummary>[] = [];
    const configs: RunConfig[] = [];
    const capture = (response: Response) => {
      if (response.request().method() === 'POST' && new URL(response.url()).pathname === '/api/runs') {
        expect(response.ok()).toBeTruthy();
        configs.push(response.request().postDataJSON().config);
        posts.push(response.json());
      }
    };
    page.on('response', capture);
    const row = page.getByRole('row').filter({ has: page.getByRole('rowheader', { name: demo.title, exact: true }) });
    await row.getByRole('button', { name: 'Run demo', exact: true }).click();
    await expect(page).toHaveURL(/\/compare\/set\?.*demo=cost_of_latency/);
    page.off('response', capture);
    await expect(page).toHaveTitle(/^Experiment Comparison/);
    runs = await Promise.all(posts);
    expect(runs).toHaveLength(2);
    expect(configs).toEqual(demo.runs.map((run) => run.config));
    first = runs[0];
    firstConfig = configs[0];
    ids = runs.map((run) => run.manifest.run_id);
    expect(new URL(page.url()).searchParams.get('runs')).toBe(ids.join(','));
    const runSet = page.getByRole('grid', { name: 'Run set' });
    for (const run of runs) {
      expect(run.manifest.dataset.id).toBe('synthetic-demo');
      await expect(runSet.getByRole('row').filter({ hasText: run.manifest.run_id.slice(0, 12) })).toContainText(run.label);
    }
    const experiment = panel(page, `Experiment ${demo.title}`);
    await expect(experiment.getByText('Varied', { exact: true })).toBeVisible();
    await expect(experiment.getByText('Controlled', { exact: true })).toBeVisible();
    for (const varied of demo.varied) await expect(experiment).toContainText(varied);
    for (const controlled of demo.controlled) await expect(experiment).toContainText(controlled);
    const variables = panel(page, 'Varied versus controlled');
    await expect(variables.getByRole('table').first()).toBeVisible();
    await expect(variables).toContainText('aligned by decision');
    await expect(variables).not.toContainText('not like-for-like');
    const divergence = panel(page, 'Decision alignment and first divergence');
    await expect(divergence).toContainText('First divergence: order');
    await expect(divergence).toContainText('Every pair before it is identical, timing included.');
    await expect(divergence.getByRole('link', { name: 'Baseline forensics', exact: true })).toBeVisible();
  });

  await test.step('Order Book Lab plays, pauses, advances one event and selects an order from the event log', async () => {
    await page.getByRole('link', { name: 'Replay Terminal', exact: true }).first().click();
    await expect(page).toHaveURL(new RegExp(`/book/runs/${ids[0]}`));
    await expect(page).toHaveTitle(/^Order Book Lab \u00b7 /);
    const play = page.getByRole('button', { name: 'Play', exact: true });
    await expect(play).toBeEnabled();
    const cursor = page.getByRole('slider', { name: 'Cursor position in the dataset' });
    const before = Number(await cursor.inputValue());
    await play.click();
    await expect.poll(async () => Number(await cursor.inputValue())).toBeGreaterThan(before);
    await page.getByRole('button', { name: 'Pause', exact: true }).click();
    await expect(play).toHaveAttribute('aria-pressed', 'false');
    const paused = Number(await cursor.inputValue());
    await page.getByRole('button', { name: /Next event/ }).click();
    await expect.poll(async () => Number(await cursor.inputValue())).toBeGreaterThan(paused);
    const eventLog = page.getByRole('grid', { name: 'Event log' });
    const orderButton = eventLog.getByRole('button', { name: /^\d+$/ }).first();
    await expect(orderButton).toBeVisible();
    const orderId = await orderButton.innerText();
    await orderButton.click();
    await expect(page.getByRole('heading', { name: new RegExp(`Selected order #${orderId}`) })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Full forensics', exact: true }).first()).toHaveAttribute('href', `/forensics/${ids[0]}/orders/${orderId}`);
  });

  await test.step('Order forensics shows evidence bases, caveats, a rendered two-clock timeline and queue chart', async () => {
    const detailResponse = page.waitForResponse((response) => /\/api\/runs\/[^/]+\/orders\/\d+$/.test(response.url()));
    await page.getByRole('link', { name: 'Full forensics', exact: true }).click();
    const detail = (await (await detailResponse).json()) as OrderDetail;
    const explanation = panel(page, 'Explanation');
    await expect(explanation).toBeVisible();
    expect(detail.explanation.steps.length).toBeGreaterThan(0);
    for (const basis of new Set(detail.explanation.steps.map((step) => step.basis))) {
      await expect(explanation.getByText(basis, { exact: true }).first()).toBeVisible();
    }
    const caveats = explanation.getByRole('region', { name: 'Caveats' });
    expect(detail.explanation.caveats.length).toBeGreaterThan(0);
    for (const caveat of detail.explanation.caveats) await expect(caveats).toContainText(caveat);
    const timeline = page.getByRole('group', { name: /^Two-clock timeline\./ });
    await expect(timeline).toBeVisible();
    // The timeline is canvas-rendered; assert pixels were actually painted, not just its wrapper.
    await expect.poll(() => timeline.locator('canvas').evaluate((canvas: HTMLCanvasElement) => {
      if (canvas.width === 0 || canvas.height === 0) return false;
      const pixels = canvas.getContext('2d')!.getImageData(0, 0, canvas.width, canvas.height).data;
      return pixels.some((value, index) => index % 4 === 3 && value > 0);
    })).toBeTruthy();
    const queue = page.getByRole('img', { name: /^Modeled quantity ahead of the order/ });
    await expect(queue).toBeVisible();
    expect(await queue.locator('path').first().getAttribute('d')).toMatch(/[MLH]/);
    await expect(panel(page, 'Modeled queue position')).toContainText('estimate and is never observed');
  });

  await test.step('Full comparison displays populated metric delta columns for the demo pair', async () => {
    await open(page, `/compare?a=${ids[0]}&b=${ids[1]}`);
    const metrics = panel(page, 'Measured outcomes').getByRole('table');
    await expect(metrics.getByRole('columnheader', { name: 'Delta (B - A)', exact: true })).toBeVisible();
    const rows = metrics.getByRole('row');
    await expect.poll(() => rows.count()).toBeGreaterThan(2);
    const cells = rows.nth(1).getByRole('cell');
    await expect(cells.nth(2)).toHaveText(/[+\-\d]/);
    await expect(page.getByRole('region', { name: 'Scenarios' })).toContainText(ids[0]);
    // 1 ms versus 50 ms each way on the same grid: aligned, like-for-like, and every pair that
    // reached the exchange in both runs arrived exactly 49 ms later, so none of them is identical.
    const compared = await request.post('/api/compare', { data: { a: ids[0], b: ids[1] } });
    expect(compared.ok()).toBeTruthy();
    const comparison = (await compared.json()) as Comparison;
    expect(comparison.eligibility).toEqual({ aligned: true, like_for_like: true, reasons: [] });
    const arrival = comparison.alignment!.timing.exch_arrival;
    expect(arrival.count).toBeGreaterThan(0);
    expect([arrival.nonzero, arrival.min, arrival.max]).toEqual([arrival.count, 49_000_000, 49_000_000]);
    const alignment = panel(page, 'Order alignment');
    await expect(alignment.getByRole('row', { name: /^Exchange arrival/ })).toContainText(String(arrival.count));
    await expect(alignment).toContainText('Same outcome, different timing');
    await expect(page.getByRole('note', { name: 'Comparison eligibility' })).toHaveCount(0);
    await expect(page.getByRole('region', { name: 'Scenarios' })).toContainText(ids[1]);
  });

  await test.step('Report and JSON downloads preserve run identity and configuration/result hashes', async () => {
    // The legacy run URL redirects into Execution Forensics.
    await open(page, `/runs/${ids[0]}`);
    await expect(page).toHaveURL(new RegExp(`/forensics/${ids[0]}$`));
    const reportDownload = page.waitForEvent('download');
    await page.getByRole('link', { name: 'Download report (.md)', exact: true }).click();
    const report = await downloadedText(await reportDownload, 'md');
    for (const value of [ids[0], first.manifest.config_sha256, first.manifest.result_sha256]) expect(report).toContain(value);
    const jsonDownload = page.waitForEvent('download');
    await page.getByRole('link', { name: 'Download run (.json)', exact: true }).click();
    const exported = JSON.parse(await downloadedText(await jsonDownload, 'json')) as RunResult;
    expect(exported.manifest.run_id).toBe(ids[0]);
    expect(exported.manifest.config_sha256).toBe(first.manifest.config_sha256);
    expect(exported.manifest.result_sha256).toBe(first.manifest.result_sha256);
    expect(exported.manifest.config).toEqual(firstConfig);
  });

  await test.step('Reload and re-POST of the exact configuration returns identical content-addressed results', async () => {
    await page.waitForLoadState('networkidle');
    await page.reload();
    await page.waitForLoadState('networkidle');
    const stored = await request.get(`/api/runs/${ids[0]}`);
    expect(stored.ok()).toBeTruthy();
    expect(((await stored.json()) as RunSummary).manifest.result_sha256).toBe(first.manifest.result_sha256);
    const repeated = await request.post('/api/runs', { data: { config: firstConfig } });
    expect(repeated.ok()).toBeTruthy();
    const reproduced = (await repeated.json()) as RunSummary;
    expect(reproduced.manifest.run_id).toBe(ids[0]);
    expect(reproduced.manifest.config_sha256).toBe(first.manifest.config_sha256);
    expect(reproduced.manifest.result_sha256).toBe(first.manifest.result_sha256);
  });

  await test.step('Strategy Lab refuses a sub-millisecond interval without submitting a run', async () => {
    await open(page, '/lab');
    const submitted: string[] = [];
    page.on('request', (request) => {
      if (request.method() === 'POST' && new URL(request.url()).pathname === '/api/runs') submitted.push(request.url());
    });
    await page.getByRole('textbox', { name: /Evaluation interval/ }).fill('0.5');
    await expect(page.getByText('invalid configuration', { exact: true })).toBeVisible();
    await expect(page.getByText('strategy.interval_ns must be between 1000000 (1 ms) and 3600000000000 (1 h)', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Run simulation', exact: true })).toBeDisabled();
    await page.getByRole('textbox', { name: /Evaluation interval/ }).press('Enter');
    await page.waitForLoadState('networkidle');
    expect(submitted).toEqual([]);
    await expect(page).toHaveURL(/\/strategy$/);
  });
});

test('foreign-origin run creation is refused before simulation', async ({ request }) => {
  const response = await request.post('/api/runs', {
    headers: { Origin: 'https://foreign.invalid' },
    data: {},
  });
  expect(response.status()).toBe(403);
  expect((await response.json()).error.code).toBe('forbidden');
  const runs = await request.get('/api/runs');
  expect(runs.ok()).toBeTruthy();
  expect(((await runs.json()) as RunSummary[]).every((run) => run.manifest.dataset.id === 'synthetic-demo')).toBeTruthy();
});
