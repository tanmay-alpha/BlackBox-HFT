// Production error paths (Phase 7 H3) that are safe to provoke against the deployed product:
// anonymous, read-only, three page loads. The failures are an unknown run id (a real 404), and,
// faked in this browser only by route interception, a study artefact with a newer contract
// version and an unreachable API. Each must show a visible, specific error; the API one offers
// Retry, and Retry recovers once the API is reachable again.

import { createHash } from 'node:crypto';

import { API_URL, WEB_URL } from './env';
import { api, expect, load, test } from './prod';

const UNKNOWN_RUN = 'ffffffffffffffff';
const STUDY = '**/study-results/exec-validation/study-result.json';

test.describe('unknown run', () => {
  test.use({ allowStatus: [404] });

  test('an unknown run id shows the not-found state', async ({ page, request }) => {
    const res = await request.get(`${API_URL}/api/v1/runs/${UNKNOWN_RUN}`, { headers: { Origin: WEB_URL }, timeout: 120_000 });
    expect(res.status()).toBe(404);
    await load(page, `/forensics/${UNKNOWN_RUN}`);
    await expect(page.getByText(`Run ${UNKNOWN_RUN} was not found.`)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Enter API key' })).toBeVisible();
  });
});

test('a study artefact with a newer contract version is refused visibly', async ({ page }) => {
  const rewritten = Promise.withResolvers<string>();
  await page.route(STUDY, async (route) => {
    const response = await route.fetch();
    const body = (await response.text()).replace('"schema_version":1', '"schema_version":2');
    expect(body, 'the artefact declares schema_version 1').toContain('"schema_version":2');
    rewritten.resolve(body);
    await route.fulfill({ response, body });
  });
  // The hash file is served for the rewritten body, so only the contract version is wrong.
  await page.route(`${STUDY}.sha256`, async (route) => {
    const digest = createHash('sha256').update(await rewritten.promise, 'utf8').digest('hex');
    await route.fulfill({ status: 200, contentType: 'text/plain', body: `${digest}  study-result.json\n` });
  });
  await load(page, '/studies/exec-validation');
  const error = page.getByTestId('study-error');
  await expect(error).toHaveAttribute('data-kind', 'unsupported_contract');
  await expect(error).toContainText('Unsupported study contract version');
  await expect(error).toContainText('version 2');
  await expect(error.getByRole('button', { name: 'Retry' })).toBeVisible();
  await expect(page.getByTestId('study-view')).toHaveCount(0);
});

test.describe('unreachable API', () => {
  test.use({ allowFailed: true });

  test('the workbench shows the network error with Retry, and Retry recovers', async ({ page, request }) => {
    const summary = await api<{ events: number }>(request, '/market/datasets/l3-demo');
    const dataset = '**/api/v1/market/datasets/l3-demo';
    await page.route(dataset, (route) => route.abort());
    await load(page, '/market/l3-demo');
    const error = page.locator('.error-box');
    await expect(error).toContainText('Could not reach the BlackBox API');
    await expect(error.getByRole('button', { name: 'Retry' })).toBeVisible();

    await page.unroute(dataset);
    await error.getByRole('button', { name: 'Retry' }).click();
    await expect(page.locator('.mk-meta')).toContainText(`${summary.events.toLocaleString('en-US')} events`);
    await expect(page.locator('.error-box')).toHaveCount(0);
  });
});
