// Research user: enter a key, submit from the Strategy Lab, follow the job through the queue to
// completion, open the run, cancel a second (queued) job, and reload with the key kept in this
// tab's sessionStorage only.

import type { Page } from '@playwright/test';

import { expect, researchKey, shot, signIn, test } from './fixtures';

/** Sets the seed (a new experiment identity) and submits; returns the job id from the URL. */
async function submitWithSeed(page: Page, seed: number): Promise<string> {
  await page.goto('/strategy');
  await page.getByRole('tab', { name: /Seed/ }).click();
  const field = page.getByLabel('Seed', { exact: true }).and(page.locator('input'));
  await field.fill(String(seed));
  await field.blur();
  await page.getByTestId('lab-submit').click();
  await page.waitForURL(/\/sim\/jobs\//);
  return page.url().split('/sim/jobs/')[1].split(/[?#]/)[0];
}

test('authenticated research story: submit, queue, complete, forensics, cancel, reload', async ({ page }) => {
  const base = 1_000_000 + Math.floor(Math.random() * 1_000_000_000);

  await page.goto('/');
  await signIn(page);
  await expect(page.getByTestId('sb-mode')).toContainText('RESEARCH');
  expect(await page.evaluate(() => sessionStorage.getItem('bb.apiKey'))).toBe(researchKey());
  expect(await page.evaluate(() => Object.values({ ...localStorage }).some((v) => v.includes('bbk_')))).toBe(false);

  // Submit -> created/queued/running -> completed.
  const jobA = await submitWithSeed(page, base);
  const detail = page.getByTestId('job-detail');
  await expect(detail).toHaveAttribute('data-job-state', /created|queued|running|completed/);
  await shot(page, '09-workbench-job-submitted');

  // A second job queues behind the first on the single worker slot; cancel it.
  const jobB = await submitWithSeed(page, base + 1);
  expect(jobB).not.toBe(jobA);
  const detailB = page.getByTestId('job-detail');
  const stateB = await detailB.getAttribute('data-job-state');
  if (stateB !== 'completed') {
    await page.getByTestId('cancel-job').click();
    await expect(detailB).toHaveAttribute('data-job-state', /cancelling|cancelled/);
    await expect(detailB).toHaveAttribute('data-job-state', 'cancelled', { timeout: 120_000 });
    await shot(page, '10-workbench-job-cancelled');
  }

  // Back to the first job: it completes and links to its run.
  await page.goto(`/sim/jobs/${jobA}`);
  await expect(page.getByTestId('job-detail')).toHaveAttribute('data-job-state', 'completed', { timeout: 150_000 });
  await shot(page, '11-workbench-job-completed');
  const runLink = page.getByTestId('job-detail').locator('.job-run-links a').first();
  const runId = (await runLink.textContent())?.trim() ?? '';
  expect(runId).toMatch(/^[0-9a-f]{16}$/);

  // Forensics of the new run.
  await runLink.click();
  await expect(page).toHaveURL(new RegExp(`/forensics/${runId}`));
  await expect(page.getByText(runId.slice(0, 12)).first()).toBeVisible();

  // Idempotent resubmission returns the stored run instead of a new job.
  await page.goto('/strategy');
  await page.getByRole('tab', { name: /Seed/ }).click();
  await page.getByLabel('Seed', { exact: true }).and(page.locator('input')).fill(String(base));
  await page.getByTestId('lab-submit').click();
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
  await shot(page, '12-saved-report');

  // Sign out clears the key.
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByTestId('access-badge')).toContainText('DEMO');
  expect(await page.evaluate(() => sessionStorage.getItem('bb.apiKey'))).toBeNull();
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
