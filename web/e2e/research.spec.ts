// Research user: enter a key, set up a run in the Strategy Lab's Guided mode (same configuration
// as Advanced), follow the job through the queue to completion, open the run and one order,
// compare it with a public demo run, cancel a second (queued) job, reload with the key kept in
// this tab's sessionStorage only, and sign out: the private run is then not visible.

import type { Page } from '@playwright/test';

import { expect, researchKey, shot, signIn, test } from './fixtures';

/** A public demo run seeded by `blackbox demo seed` (content-addressed, so stable). */
const DEMO_RUN = '253bbc23b342f373';

/** Switches the Strategy Lab form to Advanced (all sections as tabs). */
async function advanced(page: Page): Promise<void> {
  await page.getByRole('group', { name: 'Form mode' }).getByRole('button', { name: 'Advanced' }).click();
}

/** Sets the seed (a new experiment identity), submits and opens the job; returns its id from the URL. */
async function submitWithSeed(page: Page, seed: number): Promise<string> {
  await page.goto('/strategy');
  await advanced(page);
  await page.getByRole('tab', { name: /Seed/ }).click();
  const field = page.getByLabel('Seed', { exact: true }).and(page.locator('input'));
  await field.fill(String(seed));
  await field.blur();
  await page.getByTestId('lab-submit').click();
  await page.getByTestId('lab-next-job').click();
  await page.waitForURL(/\/sim\/jobs\//);
  return page.url().split('/sim/jobs/')[1].split(/[?#]/)[0];
}

/** Waits for the job on screen to complete and returns its run id. */
async function completedRun(page: Page): Promise<string> {
  await expect(page.getByTestId('job-detail')).toHaveAttribute('data-job-state', 'completed', { timeout: 150_000 });
  const runId = (await page.getByTestId('job-detail').locator('.job-run-links a').first().textContent())?.trim() ?? '';
  expect(runId).toMatch(/^[0-9a-f]{16}$/);
  return runId;
}

test.describe('keyed research journey', () => {
  // Signing out leaves the page on private resources, which the API then answers with 404.
  // 404: private resources after sign-out. 409: a cancel that lost the race to job completion.
  test.use({ allowStatus: [404, 409] });

  test('guided setup, queue, forensics, comparison, cancel, reload, sign out', async ({ page }) => {
    const base = 1_000_000 + Math.floor(Math.random() * 1_000_000_000);

    await page.goto('/');
    await signIn(page);
    await expect(page.getByTestId('sb-mode')).toContainText('RESEARCH');
    expect(await page.evaluate(() => sessionStorage.getItem('bb.apiKey'))).toBe(researchKey());
    expect(await page.evaluate(() => Object.values({ ...localStorage }).some((v) => v.includes('bbk_')))).toBe(false);

    // Guided mode (the default): a preset plus the essentials, edited in plain units.
    await page.goto('/strategy?dataset=synthetic-demo');
    // A preset card's radio is named by its short title and the strategy tag.
    await page.getByRole('radio', { name: 'Baseline Touch maker Passive maker', exact: true }).check();
    const modes = page.getByRole('group', { name: 'Form mode' });
    await expect(modes.getByRole('button', { name: 'Guided' })).toHaveAttribute('aria-pressed', 'true');
    await expect(page.getByRole('tab', { name: /Seed/ })).toHaveCount(0);
    for (const [label, value] of [
      ['Entry latency', '5'],
      ['Maker fee', '-0.5'],
      ['Taker fee', '3.5'],
    ] as const) {
      const input = page.getByLabel(label, { exact: true });
      await input.fill(value);
      await input.blur();
    }
    const changed = page.getByRole('region', { name: 'Changed vs preset' });
    await expect(changed).toContainText('Maker fee');
    await expect(changed).toContainText('-0.5 bp');
    await expect(changed).toContainText('5 ms');
    await expect(page.getByRole('region', { name: 'Validation' })).toContainText('valid');
    await shot(page, '08-lab-guided');

    // Advanced shows the same configuration object, not a copy.
    await advanced(page);
    await page.getByRole('tab', { name: /Fees/ }).click();
    await expect(page.getByLabel('Maker fee', { exact: true })).toHaveValue('-0.5');
    await page.getByRole('tab', { name: /Seed/ }).click();
    await page.getByLabel('Seed', { exact: true }).and(page.locator('input')).fill(String(base));
    await page.getByLabel('Run label').fill(`e2e guided ${base}`);
    await page.getByTestId('lab-submit').click();
    // A new configuration is a new job, never reported as already simulated (PHASE3_REVIEW D1).
    const jobStatus = page.getByTestId('lab-job-status');
    await expect(jobStatus).toHaveText(/^(Queued|Running|Completed)\./);
    await expect(jobStatus).not.toContainText('Already simulated');
    // The Lab stays on the page and links the next steps; the first is the job.
    await page.getByTestId('lab-next-job').click();
    await page.waitForURL(/\/sim\/jobs\//);
    const jobA = page.url().split('/sim/jobs/')[1].split(/[?#]/)[0];
    await expect(page.getByTestId('job-detail')).toHaveAttribute('data-job-state', /created|queued|running|completed/);
    await shot(page, '09-workbench-job-submitted');

    // A second job: cancel it. A queued job is cancelled at once; a running one stops at its next
    // checkpoint (API_CONTRACT §8.5), so a small job may finish first. Both are valid outcomes.
    const jobB = await submitWithSeed(page, base + 1);
    expect(jobB).not.toBe(jobA);
    const detailB = page.getByTestId('job-detail');
    const stateB = await detailB.getAttribute('data-job-state');
    if (stateB !== 'completed') {
      await page.getByTestId('cancel-job').click();
      const final = stateB === 'queued' || stateB === 'created' ? /^cancelled$/ : /^(cancelled|completed)$/;
      await expect(detailB).toHaveAttribute('data-job-state', final, { timeout: 120_000 });
      await shot(page, '10-workbench-job-cancelled');
    }

    // Back to the first job: it completes and links to its run.
    await page.goto(`/sim/jobs/${jobA}`);
    const runId = await completedRun(page);
    await shot(page, '11-workbench-job-completed');

    // The run, then one order's forensics.
    await page.getByTestId('job-detail').locator('.job-run-links a').first().click();
    await expect(page).toHaveURL(new RegExp(`/forensics/${runId}`));
    await expect(page.getByText(runId.slice(0, 12)).first()).toBeVisible();
    await page.locator(`a[href^="/forensics/${runId}/orders/"]`).first().click();
    await expect(page).toHaveURL(new RegExp(`/forensics/${runId}/orders/\\d+`));
    await expect(page.getByText('simulated outcome').first()).toBeVisible();

    // Compare with a public demo run, typed into the picker (it is not in this workspace's list).
    await page.goto('/compare/pair');
    await page.locator('#cmp-pick-a').fill(runId);
    await page.locator('#cmp-pick-b').fill(DEMO_RUN);
    await page.getByRole('button', { name: 'Compare', exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`/compare/pair\\?a=${runId}&b=${DEMO_RUN}`));
    await page.getByRole('button', { name: 'Create comparison' }).click();
    await expect(page.getByText('Measured outcomes', { exact: false }).first()).toBeVisible({ timeout: 30_000 });
    // Differing settings show their plain label, with the raw configuration path as the title.
    const makerFee = page.locator('th[title="execution.fees.maker"]').first();
    await expect(makerFee).toBeVisible();
    await expect(makerFee).toHaveText('Maker fee');
    await shot(page, '12-comparison-created');

    // Idempotent resubmission returns the stored run instead of a new job.
    await page.goto('/strategy');
    await advanced(page);
    await page.getByRole('radio', { name: 'Baseline Touch maker Passive maker', exact: true }).check();
    await page.getByRole('tab', { name: /Order latency/ }).click();
    await page.getByLabel('Entry latency', { exact: true }).fill('5');
    await page.getByRole('tab', { name: /Fees/ }).click();
    await page.getByLabel('Maker fee', { exact: true }).fill('-0.5');
    await page.getByLabel('Taker fee', { exact: true }).fill('3.5');
    await page.getByRole('tab', { name: /Seed/ }).click();
    await page.getByLabel('Seed', { exact: true }).and(page.locator('input')).fill(String(base));
    await page.getByTestId('lab-submit').click();
    const results = page.getByRole('region', { name: 'Job and results' });
    await expect(results.getByTestId('lab-job-status')).toContainText(`Already simulated. This exact configuration was simulated before: stored run ${runId}`);
    await page.getByTestId('lab-next-forensics').click();
    await expect(page).toHaveURL(new RegExp(`/forensics/${runId}`));

    // Reload: the key survives in this tab (sessionStorage) and research data stays readable.
    await page.goto(`/forensics/${runId}`);
    await page.reload();
    await expect(page.getByTestId('access-badge')).toContainText('RESEARCH');
    await expect(page.getByText(runId.slice(0, 12)).first()).toBeVisible();

    // Report snapshot of the new run.
    await page.goto(`/reports/${runId}`);
    await expect(page.getByRole('article')).toContainText('BlackBox research report');
    await page.getByTestId('save-report').click();
    await expect(page).toHaveURL(/\/reports\/saved\//);
    await expect(page.getByText('matches')).toBeVisible();
    await shot(page, '13-saved-report');

    // Sign out on the private run: the key is cleared and the run is no longer shown.
    await page.goto(`/forensics/${runId}`);
    await expect(page.getByRole('heading', { name: /Order blotter/i })).toBeVisible();
    await page.getByRole('button', { name: 'Sign out' }).click();
    await expect(page.getByTestId('access-badge')).toContainText('DEMO');
    expect(await page.evaluate(() => sessionStorage.getItem('bb.apiKey'))).toBeNull();
    await expect(page.getByText(`Run ${runId} was not found`)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Enter API key' })).toBeVisible();
    await expect(page.getByRole('heading', { name: /Order blotter/i })).toHaveCount(0);
  });
});

test.describe('refused key', () => {
  test.use({ allowStatus: [401] });
  test('a refused key is cleared and reported verbatim', async ({ page }) => {
    await page.goto('/');
    await page.getByTestId('access-badge').click();
    const dialog = page.getByRole('dialog', { name: 'Research access' });
    await dialog.getByLabel('API key').fill(`bbk_${'x'.repeat(43)}`);
    await dialog.getByRole('button', { name: 'Use key' }).click();
    await expect(dialog.getByRole('alert')).toContainText('ERROR 401 unauthorized');
    expect(await page.evaluate(() => sessionStorage.getItem('bb.apiKey'))).toBeNull();
  });
});
