// Queue Model & Fill Uncertainty Lab (`/queue-lab`, Phase 7 D4): the published synthetic
// `blackbox.queue-shadow` artefact renders only after its SHA-256 and contract are verified, every
// displayed interval is the artefact's own value, the view (model, L, tau, checkpoint) lives in the
// URL and Back restores it, a tampered or newer-contract file is refused visibly, and a 390 px phone
// gets staged tabs with nothing wider than the screen.

import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { Page } from '@playwright/test';

import { expect, shot, test } from './fixtures';

const ARTEFACT = '**/study-results/queue-shadow/study-result.json';
const artefact = JSON.parse(
  readFileSync(join(dirname(fileURLToPath(import.meta.url)), '..', 'public', 'study-results', 'queue-shadow', 'study-result.json'), 'utf8'),
);

interface JsonValue {
  status: string;
  value?: number;
  reason?: string;
}
interface JsonCell {
  latency_ns: number;
  horizon_ns: number;
  model: string;
  fill_fraction: JsonValue;
  fill_fraction_95: { lower: JsonValue; upper: JsonValue };
  vs_l3: { difference: JsonValue; difference_95: { lower: JsonValue; upper: JsonValue } } | null;
}
interface JsonHypothesis {
  h_a_supported: boolean | null;
  h0_not_rejected: string[];
}

function jsonCell(latency: number, tau: number, model: string): JsonCell {
  const c = (artefact.results.cells as JsonCell[]).find((x) => x.latency_ns === latency && x.horizon_ns === tau && x.model === model);
  if (!c) throw new Error(`no cell ${latency}/${tau}/${model}`);
  return c;
}

const MINUS = '\u2212';
const signed = (x: number) => {
  const rounded = Number(x.toFixed(3));
  return `${rounded === 0 ? '' : x < 0 ? MINUS : '+'}${Math.abs(x).toFixed(3)}`;
};

/** Elements of the lab whose box reaches past the viewport's right edge (cut off or scrolled away). */
async function overflowing(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const vw = document.documentElement.clientWidth;
    const out: string[] = [];
    const docWidth = Math.max(document.documentElement.scrollWidth, document.body.scrollWidth);
    if (docWidth > window.innerWidth) out.push(`document ${docWidth} px wide`);
    for (const el of Array.from(document.querySelectorAll<HTMLElement>('[data-testid="ql-view"] *'))) {
      // Screen-reader-only text (captions, the stacked tables' column headers) is clipped on purpose.
      if (el.closest('.visually-hidden, .stc-stack thead') || !el.checkVisibility()) continue;
      const r = el.getBoundingClientRect();
      if (r.width > 0 && (r.right > vw + 0.5 || r.left < -0.5)) out.push(`<${el.tagName.toLowerCase()} class="${el.className}"> ${(el.textContent ?? '').trim().slice(0, 40)} [${Math.round(r.left)}, ${Math.round(r.right)}]`);
    }
    return out.slice(0, 20);
  });
}

test.describe('queue model lab', () => {
  test('verified artefact, values equal the JSON, evidence labels and the explanation link', async ({ page }) => {
    await page.goto('/studies');
    // The study index lists the queue study; its `/studies/<id>` route redirects to the lab.
    await page.getByTestId('study-index').getByRole('link', { name: 'Queue model and fill uncertainty lab' }).click();
    await page.waitForURL(/\/queue-lab$/);
    await expect(page.getByRole('link', { name: 'Queue model lab', exact: true })).toHaveAttribute('aria-current', 'page');
    await expect(page.getByRole('link', { name: 'Execution studies', exact: true })).not.toHaveAttribute('aria-current', 'page');
    await expect(page.getByRole('heading', { level: 1, name: 'Queue model and fill uncertainty lab' })).toBeVisible();
    await expect(page.getByTestId('ql-sha256')).toContainText('verified in this browser');
    await expect(page.getByTestId('ql-sha256')).toContainText('blackbox.queue-shadow v1');
    await expect(page).toHaveTitle(/^Queue model lab \u00b7 /);
    await expect(page.getByTestId('ql-question')).toContainText('not ground truth for a hypothetical order');
    await expect(page.getByTestId('ql-question')).toContainText('did not react');

    // The L3 reference at L 0, tau 1 s: Wilson interval exactly as in the artefact.
    const l3 = jsonCell(0, 1e9, 'l3_shadow');
    const l3Est = page.getByTestId('ql-fill-chart').locator('tr[data-model="l3_shadow"] [data-testid="ql-est"]');
    await expect(l3Est).toHaveAttribute('data-value', String(l3.fill_fraction.value));
    await expect(l3Est).toHaveAttribute('data-lower', String(l3.fill_fraction_95.lower.value));
    await expect(l3Est).toHaveAttribute('data-upper', String(l3.fill_fraction_95.upper.value));
    await expect(l3Est).toContainText(`${l3.fill_fraction.value!.toFixed(3)} [${l3.fill_fraction_95.lower.value!.toFixed(3)}, ${l3.fill_fraction_95.upper.value!.toFixed(3)}]`);
    await expect(page.getByTestId('ql-fill-chart')).toContainText('nominal 95% Wilson');

    // RiskAdverse minus L3: the Newcombe interval as published.
    const ra = jsonCell(0, 1e9, 'risk_adverse').vs_l3!;
    const raEst = page.getByTestId('ql-diff-chart').locator('tr[data-model="risk_adverse"] [data-testid="ql-est"]');
    await expect(raEst).toHaveAttribute('data-lower', String(ra.difference_95.lower.value));
    await expect(raEst).toHaveAttribute('data-upper', String(ra.difference_95.upper.value));
    await expect(raEst).toContainText(`[${signed(ra.difference_95.lower.value!)}, ${signed(ra.difference_95.upper.value!)}]`);
    await expect(page.getByTestId('ql-diff-chart')).toContainText('nominal 95% Newcombe');

    // tau = 30 s has no eligible shadow in the synthetic session: shown as unavailable with the reason.
    const unavailable = jsonCell(0, 30e9, 'l3_shadow');
    expect(unavailable.fill_fraction.status).toBe('unavailable');
    await expect(page.getByTestId('ql-grid').locator(`tr[data-latency="0"][data-tau="30000000000"]`)).toContainText(`unavailable: ${unavailable.fill_fraction.reason}`);

    for (const label of ['Counterfactual shadow fill', 'Reconstructed state', 'Estimated queue position', 'Derived metric']) {
      await expect(page.getByTestId('ql-view')).toContainText(label);
    }
    await expect(page.getByTestId('ql-hyp-table').locator('tr[data-tau="30000000000"]').first()).toHaveAttribute('data-ha', 'not_testable');
    const tested = (artefact.hypotheses.cells as JsonHypothesis[]).filter((c) => c.h_a_supported !== null);
    const supported = tested.filter((c) => c.h_a_supported).length;
    const modelCount = artefact.models.length - 1;
    const rejected = tested.reduce((n, c) => n + modelCount - c.h0_not_rejected.length, 0);
    await expect(page.getByTestId('ql-outcome')).toContainText(`H-A is nominally supported in ${supported} of ${tested.length}`);
    await expect(page.getByTestId('ql-outcome')).toContainText(`H0 is nominally rejected in ${rejected} of ${tested.length * modelCount}`);
    await expect(page.getByTestId('ql-outcome')).toContainText('exploratory; unadjusted for multiplicity');
    await expect(page.getByTestId('ql-hyp-table')).toContainText('Nominal/exploratory decisions');
    await expect(page.getByTestId('ql-real-data')).toContainText('Quoted text, not data');
    await expect(page.getByTestId('ql-real-data').getByRole('link')).toHaveAttribute('href', /QUEUE_SHADOW_EXPERIMENT\.md#7-hypothesis-outcomes$/);
    await expect(page.getByTestId('ql-real-data')).toContainText('Three clean out-of-sample acquisitions');
    await expect(page.getByTestId('ql-real-data')).toContainText('2019-01-30 and 2020-01-30');
    await expect(page.getByTestId('ql-real-data')).toContainText('9 / 36 / 3 of 243');
    await expect(page.getByTestId('ql-real-data')).toContainText('0 / 11 / 3 of 27');
    await expect(page.getByTestId('ql-real-data')).toContainText('0.0167 / 0.0243 / 0.0258');
    await expect(page.getByTestId('ql-real-data')).toContainText('534.128393 s and 271.554873308 s');
    await expect(page.getByTestId('ql-real-data')).toContainText('do not replicate uniformly');
    await expect(page.getByTestId('ql-real-data')).not.toContainText('Only 2019-10-30');
    await expect(page.getByTestId('ql-real-data')).toContainText('December 30 QQQ/AAPL had already been inspected');
    await expect(page.getByTestId('ql-real-data')).toContainText('AMZN is a held-out symbol on that previously decoded day');
    await expect(page.getByTestId('ql-real-data')).toContainText('3 contrasts repeated across L, not 9 independent discoveries');
    await expect(page.getByTestId('ql-real-data')).toContainText('corrected engine predates the January acquisitions');
    await expect(page.getByTestId('ql-real-data')).not.toContainText('about what multiple comparisons');
    const limits = page.getByTestId('ql-limitations');
    await expect(limits.locator('[data-limit="dependence"]')).toContainText('too narrow');
    await expect(limits.locator('[data-limit="dependence"]')).toContainText('nominal coverage is not calibrated');
    await expect(limits.locator('[data-limit="dependence"]')).toContainText('unadjusted');
    expect(artefact.interval_note).toEqual(expect.any(String));
    await expect(page.getByTestId('ql-interval-note')).toHaveText(artefact.interval_note);
    const caveats = page.getByTestId('ql-artifact-caveats');
    await expect(caveats.locator('li')).toHaveText(artefact.caveats);
    await expect(caveats).toContainText(/overlap|serial dependen/i);
    await expect(caveats).toContainText(/multiplicity|multiple compar/i);
    expect(artefact.engine.revision).toBe('inclusive-deadlines-v2');
    for (const value of Object.values(artefact.engine) as string[]) {
      await expect(page.getByTestId('ql-engine')).toContainText(value);
    }
    await expect(limits.locator('[data-limit="latency"]')).toContainText('No latency calibration');
    await expect(limits.locator('[data-limit="counterfactual"]')).toContainText('never a recorded execution');
    await shot(page, 'queue-lab');

    // No per-shadow trace in contract v1: the workbench illustration is labelled as such.
    const explain = page.getByTestId('ql-explain');
    await expect(explain).toContainText('illustration on the same synthetic generator, not the same shadow order');
    await explain.getByTestId('ql-workbench-link').click();
    await page.waitForURL(/\/market\/l3-demo\?tab=book&example=cancel-ahead/);
  });

  test('model, L, tau and checkpoint live in the URL and Back restores them', async ({ page }) => {
    await page.goto('/queue-lab');
    await expect(page.getByTestId('ql-sha256')).toContainText('verified');
    const fills = page.getByTestId('ql-fill-chart');

    await page.getByLabel('L2 model in focus').selectOption('log_prob2');
    await expect(page).toHaveURL(/[?&]model=log_prob2(&|$)/);
    await expect(fills.locator('tr[data-model="log_prob2"]')).toHaveClass(/stc-selected/);

    await page.getByLabel('Horizon \u03c4').selectOption('5000000000');
    await expect(page).toHaveURL(/[?&]tau=5000000000(&|$)/);
    const l3At5 = jsonCell(0, 5e9, 'l3_shadow');
    await expect(fills.locator('tr[data-model="l3_shadow"] [data-testid="ql-est"]')).toHaveAttribute('data-value', String(l3At5.fill_fraction.value));

    // A selectable row is another way in: the checkpoint table sets the checkpoint.
    await page.getByTestId('ql-checkpoints').getByRole('button', { name: '1 s', exact: true }).click();
    await expect(page).toHaveURL(/[?&]checkpoint=1000000000(&|$)/);

    await page.goBack();
    await expect(page).not.toHaveURL(/checkpoint=1000000000/);
    await expect(page.getByLabel('Horizon \u03c4')).toHaveValue('5000000000');
    await page.goBack();
    await expect(page.getByLabel('Horizon \u03c4')).toHaveValue('1000000000');
    await expect(page.getByLabel('L2 model in focus')).toHaveValue('log_prob2');
    await page.goBack();
    await expect(page.getByLabel('L2 model in focus')).toHaveValue('risk_adverse');
    await page.goForward();
    await expect(page.getByLabel('L2 model in focus')).toHaveValue('log_prob2');

    // A shared link (also through the study route) opens the same view; a per-side request is answered, not faked.
    await page.goto('/studies/queue-shadow?model=power_prob2_n2&latency=50000&tau=5000000000&checkpoint=100000000&side=buy');
    await page.waitForURL(/\/queue-lab\?model=power_prob2_n2&/);
    await expect(page.getByLabel('Latency L (assumed)')).toHaveValue('50000');
    await expect(page.getByLabel('L2 model in focus')).toHaveValue('power_prob2_n2');
    await expect(page.getByTestId('ql-grid').locator('tr[data-latency="50000"][data-tau="5000000000"]')).toHaveClass(/selected/);
    await expect(page.getByTestId('ql-ignored')).toContainText('side=buy: contract v1 pools buy and sell shadows');
    const pp = jsonCell(50_000, 5e9, 'power_prob2_n2');
    await expect(page.getByTestId('ql-fill-chart').locator('tr[data-model="power_prob2_n2"] [data-testid="ql-est"]')).toHaveAttribute(
      'data-value',
      String(pp.fill_fraction.value),
    );
  });

  test('a tampered artefact is refused with a visible hash mismatch', async ({ page }) => {
    await page.route(ARTEFACT, async (route) => {
      const response = await route.fetch();
      const body = `${await response.text()}\n`;
      await route.fulfill({ response, body });
    });
    await page.goto('/queue-lab');
    await expect(page.getByTestId('ql-error')).toHaveAttribute('data-kind', 'hash_mismatch');
    await expect(page.getByTestId('ql-error')).toContainText('Content hash mismatch');
    await expect(page.getByTestId('ql-view')).toHaveCount(0);
  });

  test('a newer contract version is refused even with a matching hash', async ({ page }) => {
    const rewritten = Promise.withResolvers<string>();
    await page.route(ARTEFACT, async (route) => {
      const response = await route.fetch();
      const body = (await response.text()).replace('"schema_version":1', '"schema_version":2');
      rewritten.resolve(body);
      await route.fulfill({ response, body });
    });
    await page.route(`${ARTEFACT}.sha256`, async (route) => {
      const digest = createHash('sha256').update(await rewritten.promise, 'utf8').digest('hex');
      await route.fulfill({ status: 200, contentType: 'text/plain', body: `${digest}  study-result.json\n` });
    });
    await page.goto('/queue-lab');
    await expect(page.getByTestId('ql-error')).toHaveAttribute('data-kind', 'unsupported_contract');
    await expect(page.getByTestId('ql-error')).toContainText('version 2');
    await expect(page.getByTestId('ql-view')).toHaveCount(0);
  });

  test.describe('phone 390x844', () => {
    test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true, deviceScaleFactor: 2 });

    test('staged tabs, nothing wider than the screen, the tab in the URL', async ({ page }) => {
      await page.goto('/queue-lab');
      await expect(page.getByTestId('ql-sha256')).toContainText('verified');
      const view = page.getByTestId('ql-view');
      await expect(view).toHaveAttribute('data-stage', 'overview');
      await expect(view.getByRole('tab', { name: 'Fills' })).toBeVisible();
      for (const [tab, testId] of [
        ['Overview', 'ql-identity'],
        ['Fills', 'ql-fill-chart'],
        ['Queue', 'ql-checkpoint-chart'],
        ['Hypotheses', 'ql-hyp-table'],
      ] as const) {
        await view.getByRole('tab', { name: tab }).click();
        await expect(view.getByRole('tab', { name: tab })).toHaveAttribute('aria-selected', 'true');
        await expect(page.getByTestId(testId)).toBeVisible();
        if (tab === 'Overview') {
          await expect(page.getByTestId('ql-interval-note')).toHaveText(artefact.interval_note);
          await expect(page.getByTestId('ql-artifact-caveats').locator('li')).toHaveText(artefact.caveats);
        }
        if (tab === 'Hypotheses') await expect(page.getByTestId('ql-hypotheses')).toContainText('nominal and exploratory');
        expect(await overflowing(page), `${tab}: elements past the 390 px viewport`).toEqual([]);
      }
      await expect(page).toHaveURL(/[?&]tab=hypotheses(&|$)/);
      await view.getByRole('tab', { name: 'Fills' }).click();
      await page.getByTestId('ql-paired').locator('xpath=ancestor::details').locator('summary').click();
      expect(await overflowing(page), 'Fills with the paired table open').toEqual([]);
      await shot(page, 'queue-lab-phone');
    });
  });
});
