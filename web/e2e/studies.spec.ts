// Execution study viewer: the published synthetic study downloads, verifies its SHA-256 and
// contract version, shows its honesty labels and charts, links the `execution_validation` runs
// that `demo seed` stored (matched by config and dataset hash), offers the reproduction command
// for every other run, and refuses a tampered or newer-contract file visibly. Phase 7: any matrix
// cell, chart row or seed opens a detail panel addressed by `?cell=` (Back/Forward restore it),
// the B2 views and the uncertainty breakdown render from validated fields, and a phone gets
// Overview / Matrix / Cell / Uncertainty tabs.

import { createHash } from 'node:crypto';

import { expect, shot, test } from './fixtures';

const STUDY = '**/study-results/exec-validation/study-result.json';

test.describe('execution study viewer', () => {
  test('index, verified study, labels, charts, stored-run links and reproduction', async ({ page }) => {
    await page.goto('/studies');
    await expect(page.getByRole('heading', { level: 1, name: 'Execution studies' })).toBeVisible();
    await page.getByTestId('study-index').getByRole('link', { name: 'Execution model validation' }).click();
    await page.waitForURL(/\/studies\/exec-validation$/);

    const view = page.getByTestId('study-view');
    await expect(view).toHaveAttribute('data-study-id', 'exec-validation');
    await expect(page.getByTestId('study-sha256')).toContainText('verified in this browser');
    await expect(page.getByTestId('study-identity')).toContainText('blackbox.study-result v1');
    await shot(page, 'studies-exec-validation');

    // E4: data classes and the calibration disclaimer; nothing annualised.
    const classes = page.getByTestId('study-data-classes');
    for (const label of ['Deterministic outcome', 'Model sensitivity', 'Seed variation', 'Not calibrated to real markets']) {
      await expect(classes).toContainText(label);
    }
    await expect(view).not.toContainText(/annuali[sz]ed (return|sharpe|volatility|p&l)|per annum|\bp\.a\./i);

    // E1: design, matrix and charts.
    await expect(page.getByTestId('study-baseline')).toContainText('maker_baseline');
    await expect(page.getByTestId('study-held-constant')).toContainText('Decision interval');
    await expect(page.getByTestId('study-matrix-normal_liquidity')).toBeVisible();
    await expect(page.getByTestId('study-queue-chart')).toContainText('Risk-adverse');
    await expect(page.getByTestId('study-latency-chart')).toContainText('seeds');
    await expect(page.getByTestId('study-fill-chart')).toBeVisible();
    await expect(page.getByTestId('study-decomposition')).toContainText('Liquidity depletion');
    await expect(page.getByTestId('study-regime-chart')).toContainText('Widening spreads');
    await page.getByLabel('Measure for the comparisons below').selectOption('fill_rate');
    await expect(page).toHaveURL(/metric=fill_rate/);

    // E3: the seeded demo runs link by config + dataset hash; the chain opens the comparison.
    const heavy = page.locator('[data-testid="study-anchor"][data-run-id="08200b05ca35723f"]');
    await expect(heavy).toHaveAttribute('data-link', 'stored');
    await expect(heavy.getByRole('link', { name: /First divergence/ })).toBeVisible();
    await expect(heavy.getByRole('link', { name: 'Order forensics' })).toBeVisible();
    await expect(heavy.getByRole('link', { name: 'Report' })).toBeVisible();
    await heavy.scrollIntoViewIfNeeded();
    await shot(page, 'studies-evidence');
    await heavy.getByRole('link', { name: 'Comparison' }).click();
    await page.waitForURL(/\/compare\/pair\?a=253bbc23b342f373&b=08200b05ca35723f$/);

    // Study cells live on study-only datasets: no stored run, a reproduction command instead.
    await page.goBack();
    await page.getByText('Show every run with its hashes and reproduction command').click();
    const cell = page.getByTestId('study-cells').locator('tr[data-link="not_stored"]').first();
    await expect(cell).toContainText('No stored public run has this config hash');
    await cell.getByText('Reproduce locally').click();
    await expect(cell.locator('pre')).toContainText('blackbox study exec-validation --out data/studies/exec-validation');
  });

  test('a tampered study is refused with a visible hash mismatch', async ({ page }) => {
    await page.route(STUDY, async (route) => {
      const response = await route.fetch();
      const body = (await response.text()).replace('"data_class":"synthetic"', '"data_class":"synthetiC"');
      await route.fulfill({ response, body });
    });
    await page.goto('/studies/exec-validation');
    await expect(page.getByTestId('study-error')).toHaveAttribute('data-kind', 'hash_mismatch');
    await expect(page.getByTestId('study-error')).toContainText('Content hash mismatch');
    await expect(page.getByTestId('study-view')).toHaveCount(0);
  });

  test('a newer contract version is refused visibly even with a matching hash', async ({ page }) => {
    const rewritten = Promise.withResolvers<string>();
    await page.route(STUDY, async (route) => {
      const response = await route.fetch();
      const body = (await response.text()).replace('"schema_version":1', '"schema_version":2');
      rewritten.resolve(body);
      await route.fulfill({ response, body });
    });
    await page.route(`${STUDY}.sha256`, async (route) => {
      // The JSON request is issued alongside; hash its rewritten body.
      const digest = createHash('sha256').update(await rewritten.promise, 'utf8').digest('hex');
      await route.fulfill({ status: 200, contentType: 'text/plain', body: `${digest}  study-result.json\n` });
    });
    await page.goto('/studies/exec-validation');
    await expect(page.getByTestId('study-error')).toHaveAttribute('data-kind', 'unsupported_contract');
    await expect(page.getByTestId('study-error')).toContainText('version 2');
    await expect(page.getByTestId('study-view')).toHaveCount(0);
  });

  test('cell selection: matrix and chart rows open the detail, the URL keeps it, Back restores it', async ({ page }) => {
    await page.goto('/studies/exec-validation');
    const panel = page.getByTestId('cell-panel');
    await expect(panel).toHaveCount(0);

    // A factorial cell of a stressed regime: paired within its regime, two factors at once.
    const factorial = 'group.widening_spreads.risk_adverse.heavy_tail.0';
    await page.getByTestId('study-matrix-widening_spreads').locator(`button[data-cell="${factorial}"]`).click();
    await expect(page).toHaveURL(new RegExp(`[?&]cell=${factorial.replace(/\./g, '\\.')}(&|$)`));
    await expect(panel).toHaveAttribute('data-cell', factorial);
    await expect(panel.getByRole('heading', { level: 2 })).toHaveText('Widening spreads \u00b7 Risk-adverse \u00b7 Heavy tail (clipped cubic), median 1 ms');
    await expect(panel.getByTestId('comparison-label').first()).toHaveAttribute('data-kinds', 'multi_seed paired not_attributable');
    const diffs = panel.getByTestId('cell-differences');
    await expect(diffs.locator('tr[data-path="dataset_id"]')).toContainText('study-widening_spreads');
    await expect(diffs.locator('th[title="execution.queue_model.type"]')).toHaveText(/Queue-position model/);
    await expect(diffs.locator('tr[data-path="seed"]')).toContainText('5 seeds: 1, 7, 19, 31, 43');
    await expect(panel.getByTestId('cell-baseline')).toContainText('maker_baseline');
    await expect(panel.getByTestId('cell-notes')).toContainText('5 seeds give imprecise intervals');
    await shot(page, 'studies-cell-group');

    // A seed run of that group: study-only dataset, so no stored run and the local command.
    await panel.getByTestId('cell-evidence').getByRole('button', { name: /seed 7/ }).click();
    await expect(page).toHaveURL(/cell=run\.816b974ab18a23d4/);
    const evidence = panel.getByTestId('cell-evidence');
    await expect(evidence).toHaveAttribute('data-link', 'not_stored');
    await expect(evidence).toContainText('Not stored: reproduce locally.');
    await expect(evidence.getByTestId('cell-reproduce-inline')).toContainText('blackbox study exec-validation --out data/studies/exec-validation');
    await expect(evidence.getByTestId('cell-reproduce-inline')).toContainText('cells/816b974ab18a23d4.json');
    await expect(panel.getByTestId('cell-identity')).toContainText('816b974ab18a23d4');

    // A chart row selects its group; keyboard selection works the same way.
    await page.getByTestId('study-queue-chart').getByRole('button', { name: /Risk-adverse/ }).click();
    await expect(page).toHaveURL(/cell=group\.normal_liquidity\.risk_adverse\.constant\.0/);
    await expect(panel.getByTestId('comparison-label').first()).toHaveAttribute('data-kinds', 'paired');

    await page.goBack();
    await expect(panel).toHaveAttribute('data-cell', 'run.816b974ab18a23d4');
    await page.goBack();
    await expect(panel).toHaveAttribute('data-cell', factorial);
    await page.goForward();
    await expect(panel).toHaveAttribute('data-cell', 'run.816b974ab18a23d4');

    const regimeRow = page.getByTestId('study-regime-chart').getByRole('button', { name: /Thin book/ });
    await regimeRow.focus();
    await page.keyboard.press('Enter');
    await expect(panel).toHaveAttribute('data-cell', 'group.thin_book.power_prob3.constant.0');
    await expect(panel.getByTestId('comparison-label').first()).toHaveAttribute('data-kinds', 'cross_dataset metrics_only');
    await panel.getByRole('button', { name: 'Close cell detail' }).click();
    await expect(panel).toHaveCount(0);
    await expect(page).not.toHaveURL(/cell=/);
  });

  test('desktop: from a matrix cell the keyboard reaches the opened detail in two presses', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto('/studies/exec-validation');
    const cell = page.getByTestId('study-matrix-normal_liquidity').locator('button[data-cell]').first();
    await cell.focus();
    await page.keyboard.press('Enter');
    const goto = page.getByRole('button', { name: 'Go to cell detail' });
    await goto.focus();
    await page.keyboard.press('Enter');
    await expect(page.getByTestId('cell-panel').getByRole('heading', { level: 2 })).toBeFocused();
    // The skip control sits before the matrices: Shift+Tab from the selected cell reaches it.
    await cell.focus();
    for (let i = 0; i < 40 && !(await goto.evaluate((el) => el === document.activeElement)); i++) await page.keyboard.press('Shift+Tab');
    await expect(goto).toBeFocused();
  });

  test('a stored demo run in the panel links to its order forensics', async ({ page }) => {
    await page.goto('/studies/exec-validation?cell=run.08200b05ca35723f');
    const evidence = page.getByTestId('cell-panel').getByTestId('cell-evidence');
    await expect(evidence).toHaveAttribute('data-link', 'stored');
    await expect(evidence.getByRole('link', { name: 'First divergence (order 0)' })).toBeVisible();
    await evidence.getByRole('link', { name: 'Order forensics' }).click();
    await page.waitForURL(/\/forensics\/08200b05ca35723f\/orders\/0$/);
  });

  test('B2 views and the uncertainty breakdown render from validated fields', async ({ page }) => {
    await page.goto('/studies/exec-validation');
    await expect(page.getByTestId('study-view')).toBeVisible();
    await expect(page.locator('[data-omitted]')).toHaveCount(0);
    await expect(page.getByTestId('study-inventory-chart')).toContainText('Feed latency offset +20 ms');
    await expect(page.getByTestId('study-cancel-delta')).toContainText('Cancelled orders');
    await expect(page.getByTestId('study-feed-delta')).toContainText('Fill rate');
    const strip = page.getByTestId('study-seed-strip');
    await expect(strip.locator('tbody tr')).toHaveCount(8);
    await strip.locator('tr[data-cell="group.normal_liquidity.power_prob3.jitter.0"]').getByRole('button', { name: /^s19 / }).click();
    await expect(page).toHaveURL(/cell=run\.c72543d81883547d/);
    const unc = page.getByTestId('study-uncertainty');
    for (const text of ['Seed variability', 'Parameter sensitivity', 'Dataset variation', 'Sampling error', 'Model uncertainty', 'Real-world calibration uncertainty']) {
      await expect(unc).toContainText(text);
    }
    await expect(page.getByTestId('uncertainty-sampling')).toContainText('Not estimated here');
    await expect(page.getByTestId('uncertainty-calibration')).toContainText('Not addressed');
    await expect(page.getByTestId('study-view')).not.toContainText(/annuali[sz]ed (return|sharpe|volatility|p&l)|per annum|\bp\.a\.|profitab(le|ility) (strategy|edge)/i);
    await unc.scrollIntoViewIfNeeded();
    await shot(page, 'studies-uncertainty');
  });

  test('a phone gets Overview / Matrix / Cell / Uncertainty tabs with nothing cut off', async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto('/studies/exec-validation');
    const tabs = page.getByRole('tablist', { name: 'Study view' });
    await expect(tabs.getByRole('tab')).toHaveText(['Overview', 'Matrix', 'Cell', 'Uncertainty']);
    await expect(page.getByTestId('study-identity')).toBeVisible();
    await expect(page.getByText('Not calibrated to real markets').first()).toBeVisible();

    await tabs.getByRole('tab', { name: 'Matrix' }).click();
    await expect(page).toHaveURL(/tab=matrix/);
    await page.getByTestId('study-matrix-liquidity_depletion').locator('button[data-cell="group.liquidity_depletion.risk_adverse.constant.0"]').click();
    await expect(page).toHaveURL(/tab=cell/);
    await expect(tabs.getByRole('tab', { name: /Cell/ })).toHaveAttribute('aria-selected', 'true');
    const panel = page.getByTestId('cell-panel');
    await expect(panel).toHaveAttribute('data-cell', 'group.liquidity_depletion.risk_adverse.constant.0');
    // Full hashes wrap instead of being cut; the panel never scrolls sideways.
    const sha = (await panel.getByTestId('cell-identity').locator('.stc-full').first().textContent()) ?? '';
    expect(sha).toMatch(/^[0-9a-f]{64}$/);
    const overflow = await panel.evaluate((el) => el.scrollWidth - el.clientWidth);
    expect(overflow).toBeLessThanOrEqual(1);
    await shot(page, 'studies-phone-cell');

    await page.goBack();
    await expect(tabs.getByRole('tab', { name: 'Matrix' })).toHaveAttribute('aria-selected', 'true');
    await tabs.getByRole('tab', { name: 'Uncertainty' }).click();
    await expect(page.getByTestId('study-uncertainty')).toBeVisible();
    await expect(page.getByTestId('study-seed-strip')).toBeVisible();
  });
});
