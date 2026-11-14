import type { Page } from '@playwright/test';
import type { Preset, RunSummary } from '../src/api/types';
import { test, expect } from './fixtures';

const BANNER = 'Simulated on historical replay \u2014 research only. Simulated orders do not affect historical prices.';

async function open(page: Page, path: string) {
  await page.goto(path);
  await page.waitForLoadState('networkidle');
}

test('legacy URLs redirect to their workspaces and keep the query', async ({ page }) => {
  const cases: [string, RegExp][] = [
    ['/datasets', /\/data(\/synthetic-demo)?$/],
    ['/replay/synthetic-demo?levels=10', /\/book\/data\/synthetic-demo\?levels=10$/],
    ['/lab?dataset=synthetic-demo', /\/strategy\?dataset=synthetic-demo$/],
    ['/runs', /\/forensics$/],
    ['/latency', /\/compare\/set$/],
    ['/grid', /\/compare\/sweep$/],
    ['/compare', /\/compare\/pair$/],
  ];
  for (const [from, to] of cases) {
    await open(page, from);
    await expect(page, from).toHaveURL(to);
  }
});

test('shell: landmarks, status bar, research banner, keyboard model, palette', async ({ page, request }) => {
  const presets = (await (await request.get('/api/presets')).json()) as Preset[];
  const created = await request.post('/api/runs', { data: { config: { ...presets[0].config, dataset_id: 'synthetic-demo' } } });
  expect(created.ok()).toBeTruthy();
  const run = (await created.json()) as RunSummary;

  await open(page, '/');
  await expect(page).toHaveTitle(/^Overview \u2014 BlackBox HFT$/);
  await expect(page.getByRole('banner')).toContainText('BlackBox HFT');
  const rail = page.getByRole('navigation', { name: 'Workspaces' });
  for (const name of ['Overview', 'Market Data Explorer', 'Order Book Lab', 'Strategy Lab', 'Execution Forensics', 'Experiment Comparison']) {
    await expect(rail.getByRole('link', { name, exact: true })).toBeVisible();
  }
  // Workspaces without an API behind them are not offered.
  for (const name of ['Simulation Workbench', 'Risk & Performance', 'Research Reports', 'System']) {
    await expect(rail.getByRole('link', { name })).toHaveCount(0);
  }
  await expect(rail.getByRole('link', { name: 'Overview', exact: true })).toHaveAttribute('aria-current', 'page');
  await expect(page.getByRole('main')).toBeVisible();
  await expect(page.getByRole('complementary', { name: 'Inspector' })).toBeVisible();

  const status = page.getByRole('contentinfo', { name: 'Status' });
  await expect(status).toContainText(BANNER);
  await expect(status.getByRole('button', { name: /^API ok \d+ ms$/ })).toBeVisible();
  await expect(status).toContainText(/\d{2}:\d{2}:\d{2} UTC/);
  await expect(status).toContainText('LOCAL');
  await expect(status).toContainText(/v\d+\.\d+\.\d+ \u00b7 contract v1 \u00b7 hftbacktest/);

  // g-chords navigate; F6 moves between regions.
  await page.locator('body').press('g');
  await page.locator('body').press('f');
  await expect(page).toHaveURL(/\/forensics$/);
  await expect(page).toHaveTitle(/^Execution Forensics \u2014 BlackBox HFT$/);
  const region = () => page.evaluate(() => document.activeElement?.closest('header, nav, [role="toolbar"], main, aside, footer')?.tagName ?? null);
  await page.keyboard.press('F6');
  expect(await region()).toBe('HEADER');
  await page.keyboard.press('F6');
  expect(await region()).toBe('NAV');
  await page.keyboard.press('Shift+F6');
  expect(await region()).toBe('HEADER');

  // Inspector toggles with i; single keys can be turned off and then do nothing.
  await page.locator('body').press('i');
  await expect(page.getByRole('complementary', { name: 'Inspector' })).toHaveCount(0);
  await page.locator('body').press('i');
  await expect(page.getByRole('complementary', { name: 'Inspector' })).toBeVisible();
  await page.locator('body').press('?');
  const help = page.getByRole('dialog', { name: 'Help, shortcuts and settings' });
  await expect(help).toBeVisible();
  await help.getByRole('checkbox', { name: /Single-key shortcuts/ }).uncheck();
  await page.keyboard.press('Escape');
  await expect(help).toHaveCount(0);
  await page.locator('body').press('i');
  await expect(page.getByRole('complementary', { name: 'Inspector' })).toBeVisible();

  // Colour-vision-deficiency palette is a setting and survives a reload.
  await page.keyboard.press('Control+k');
  const palette = page.getByRole('dialog', { name: 'Command palette' });
  await expect(palette).toBeVisible();
  await palette.getByRole('combobox').fill('blue orange');
  await page.keyboard.press('Enter');
  await expect(page.locator('html')).toHaveAttribute('data-side-palette', 'cvd');
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-side-palette', 'cvd');

  // The palette finds runs from the API and opens them.
  await page.keyboard.press('Control+k');
  await palette.getByRole('combobox').fill(run.manifest.run_id.slice(0, 10));
  await expect(palette.getByRole('option').first()).toContainText(run.manifest.run_id.slice(0, 12));
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(new RegExp(`/forensics/${run.manifest.run_id}$`));
});

test('data table: grid semantics, sorting and keyboard row navigation', async ({ page }) => {
  await open(page, '/forensics');
  const grid = page.getByRole('grid').first();
  await expect(grid).toBeVisible();
  const total = Number(await grid.getAttribute('aria-rowcount'));
  expect(total).toBeGreaterThan(1);
  const sortable = grid.locator('th[aria-sort]').first();
  await expect(sortable).toHaveAttribute('aria-sort', 'none');
  await sortable.getByRole('button').click();
  await expect(sortable).toHaveAttribute('aria-sort', 'ascending');
  const first = grid.locator('tbody tr[aria-rowindex="2"]');
  await first.focus();
  await page.keyboard.press('End');
  await expect(grid.locator(`tbody tr[aria-rowindex="${total}"]`)).toBeFocused();
  await page.keyboard.press('Home');
  await expect(first).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/\/forensics\/[0-9a-f]{16}$/);
});

test('split panes resize from the keyboard and persist per workspace', async ({ page }) => {
  await open(page, '/data/synthetic-demo');
  const separator = page.getByRole('separator', { name: /^Resize / }).first();
  await expect(separator).toBeVisible();
  const before = Number(await separator.getAttribute('aria-valuenow'));
  await separator.focus();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowRight');
  await expect.poll(async () => Number(await separator.getAttribute('aria-valuenow'))).toBeGreaterThan(before);
  const resized = Number(await separator.getAttribute('aria-valuenow'));
  await page.reload();
  await page.waitForLoadState('networkidle');
  await expect.poll(async () => Number(await page.getByRole('separator', { name: /^Resize / }).first().getAttribute('aria-valuenow'))).toBeCloseTo(resized, 0);
  await page.getByRole('button', { name: 'Reset layout' }).click();
  await expect.poll(async () => Number(await page.getByRole('separator', { name: /^Resize / }).first().getAttribute('aria-valuenow'))).toBeCloseTo(before, 0);
});
