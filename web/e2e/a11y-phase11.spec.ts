// Phase 11 accessibility audit of the Phase 6–7 pages (research home, studies and the cell
// explorer, queue lab, Market Data Workbench, compare set), keyboard and screen reader only, no
// axe: every page has one h1, a main landmark, named controls and headed tables in its
// accessibility tree; closing the cell detail (Close or Escape) returns focus to the control that
// opened it, a phone moves focus to the detail it opens; the workbench's Previous/Next keep focus
// while they search; the ladder's L2 cells say what their two figures are; the loading line does
// not announce its seconds counter; badges reach 4.5:1 against their surface; and on a touch
// screen every chart row, seed value and stacked link is a 44 px target.

import type { Locator, Page } from '@playwright/test';

import { expect, test } from './fixtures';

const STUDY = '/studies/exec-validation';
const CELL = 'group.normal_liquidity.risk_adverse.heavy_tail.0';

/** Waits until `locator` holds the keyboard focus; the message names what holds it instead. */
async function expectFocused(locator: Locator, what: string): Promise<void> {
  const holder = () =>
    locator.page().evaluate(() => {
      const e = document.activeElement;
      return e === null || e === document.body ? 'body' : `${e.tagName.toLowerCase()}.${e.className} ${e.getAttribute('data-cell') ?? ''}`;
    });
  await expect.poll(() => locator.evaluate((el) => el === document.activeElement), { message: `${what}: focus is on ${await holder()}` }).toBe(true);
}

test.describe('accessibility tree of the phase 6–7 pages', () => {
  for (const path of ['/research', '/studies', STUDY, `${STUDY}?cell=${CELL}`, '/queue-lab', '/market/l3-demo', '/market/l3-demo?tab=book', '/compare/set?demo=execution_validation']) {
    test(`${path}: one h1, a main landmark, named controls, headed tables`, async ({ page }) => {
      await page.goto(path);
      await expect(page.getByRole('heading', { level: 1 })).toHaveCount(1);
      await expect(page.locator('.spinner')).toHaveCount(0);
      const tree = await page.locator('body').ariaSnapshot();
      expect(tree.match(/^\s*- main\b/m), 'one main landmark').not.toBeNull();
      // A control without an accessible name shows in the snapshot as a bare role line.
      const nameless = tree.split('\n').filter((l) => /^\s*- (button|link|combobox|tab|checkbox|textbox)\s*(\[[^\]]*\])?:?$/.test(l));
      expect(nameless, 'controls without an accessible name').toEqual([]);
      const unheaded = await page.locator('main table').evaluateAll((ts) => ts.filter((t) => t.querySelector('th') === null).map((t) => t.className));
      expect(unheaded, 'tables without header cells').toEqual([]);
      const skips = await page.evaluate(() => {
        const levels = Array.from(document.querySelectorAll('main :is(h1, h2, h3, h4, h5, h6)')).map((h) => Number(h.tagName[1]));
        return levels.flatMap((l, i) => (i > 0 && l > levels[i - 1] + 1 ? [`h${levels[i - 1]} -> h${l}`] : []));
      });
      expect(skips, 'skipped heading levels').toEqual([]);
    });
  }
});

test.describe('study cell detail focus, 1440×900', () => {
  test('Close and Escape return focus to the matrix cell that opened the detail', async ({ page }) => {
    await page.goto(STUDY);
    const opener = page.getByTestId('study-matrix-normal_liquidity').locator(`button[data-cell="${CELL}"]`);
    const panel = page.getByTestId('cell-panel');

    await opener.focus();
    await page.keyboard.press('Enter');
    await expect(panel).toHaveAttribute('data-cell', CELL);
    await expectFocused(opener, 'selection keeps focus on the cell');
    await panel.getByRole('button', { name: 'Close cell detail' }).focus();
    await page.keyboard.press('Enter');
    await expect(panel).toHaveCount(0);
    await expectFocused(opener, 'Close returns focus');

    await page.keyboard.press('Enter');
    await expect(panel).toHaveAttribute('data-cell', CELL);
    await panel.getByRole('button', { name: /seed 7/ }).focus();
    await page.keyboard.press('Escape');
    await expect(panel).toHaveCount(0);
    await expectFocused(opener, 'Escape returns focus');
  });

  test('a selection made inside the detail moves focus to the new detail, not the page', async ({ page }) => {
    await page.goto(`${STUDY}?cell=${CELL}`);
    const panel = page.getByTestId('cell-panel');
    await panel.getByRole('button', { name: /seed 7/ }).focus();
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(/cell=run\.[0-9a-f]{16}(&|$)/);
    await expectFocused(panel.getByRole('heading', { level: 2 }).first(), 'focus on the new detail title');
  });

  test('closing a detail opened from the URL leaves focus in the workspace, not on the page body', async ({ page }) => {
    await page.goto(`${STUDY}?cell=${CELL}`);
    await page.getByTestId('cell-panel').getByRole('button', { name: 'Close cell detail' }).click();
    await expect(page.getByTestId('cell-panel')).toHaveCount(0);
    await expectFocused(page.getByTestId('study-matrix-normal_liquidity').locator(`button[data-cell="${CELL}"]`), 'the cell of the closed detail');
  });
});

test.describe('study cell detail focus, phone', () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

  test('opening a cell focuses its detail; Close returns to the cell on the Matrix tab', async ({ page }) => {
    await page.goto(`${STUDY}?tab=matrix`);
    const cell = page.getByTestId('study-matrix-normal_liquidity').locator(`button[data-cell="${CELL}"]`);
    await cell.focus();
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(/tab=cell/);
    const panel = page.getByTestId('cell-panel');
    await expectFocused(panel.getByRole('heading', { level: 2 }).first(), 'focus on the detail title');
    await panel.getByRole('button', { name: 'Close cell detail' }).focus();
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(/tab=matrix/);
    await expectFocused(cell, 'Close returns focus to the cell');
  });

  test('queue lab stage tabs follow the arrow keys', async ({ page }) => {
    await page.goto('/queue-lab');
    const tabs = page.getByRole('tablist', { name: 'Lab view' });
    await tabs.getByRole('tab', { selected: true }).focus();
    await page.keyboard.press('ArrowRight');
    await expect(tabs.getByRole('tab', { name: 'Fills' })).toHaveAttribute('aria-selected', 'true');
    await expectFocused(tabs.getByRole('tab', { name: 'Fills' }), 'arrow moves focus with the selection');
    await page.keyboard.press('End');
    await expectFocused(tabs.getByRole('tab', { name: 'Hypotheses' }), 'End');
  });

  /** Height of the region that hit-tests as `el` through its centre column. */
  async function hitHeight(el: Locator): Promise<number> {
    await el.scrollIntoViewIfNeeded();
    return el.evaluate((e) => {
      const r = e.getBoundingClientRect();
      const x = r.left + Math.min(r.width / 2, 20);
      const y = r.top + r.height / 2;
      const hit = (dy: number) => {
        const t = document.elementFromPoint(x, y + dy);
        return t !== null && (t === e || e.contains(t));
      };
      let up = 0;
      let down = 0;
      while (up < 30 && hit(-(up + 1))) up += 1;
      while (down < 30 && hit(down + 1)) down += 1;
      return up + down + 1;
    });
  }

  for (const [path, selector] of [
    [`${STUDY}?tab=matrix`, '[data-testid="study-queue-chart"] .stc-pick'],
    [`${STUDY}?tab=uncertainty`, '[data-testid="study-seed-strip"] .stc-seedvals .stc-pick'],
    ['/studies', '.stc-index-link'],
    ['/research', '.rh-evidence > a'],
  ] as const) {
    test(`touch targets: ${selector} on ${path} are at least 44 px tall`, async ({ page }) => {
      await page.goto(path);
      const targets = page.locator(selector);
      await expect(targets.first()).toBeVisible();
      const n = Math.min(await targets.count(), 6);
      for (let i = 0; i < n; i += 1) expect(await hitHeight(targets.nth(i)), `${selector} #${i}`).toBeGreaterThanOrEqual(44);
    });
  }
});

test.describe('market data workbench', () => {
  test('Previous and Next keep keyboard focus while they search; the ladder names its figures', async ({ page }) => {
    await page.goto('/market/l3-demo?tab=book');
    await expect(page.locator('.mk-ladder')).toBeVisible();
    const next = page.getByRole('button', { name: 'Next', exact: true });
    const event = async () => Number(new URL(page.url()).searchParams.get('event'));
    const start = await event();
    await next.focus();
    await page.keyboard.press('Enter');
    await expect.poll(event).toBe(start + 1);
    await expectFocused(next, 'Next keeps focus');
    await page.keyboard.press('Enter');
    await expect.poll(event).toBe(start + 2);
    const previous = page.getByRole('button', { name: 'Previous', exact: true });
    await previous.focus();
    await page.keyboard.press('Enter');
    await expect.poll(event).toBe(start + 1);
    await expectFocused(previous, 'Previous keeps focus');

    // An L2 cell reads "3,200 shares in 7 orders", not "3,200 7".
    const tree = await page.locator('.mk-ladder').ariaSnapshot();
    expect(tree).toMatch(/cell "[\d,]+ shares in [\d,]+ orders?"/);
    expect(tree).not.toMatch(/cell "[\d,]+ [\d,]+"/);
  });
});

test('the loading line announces its label once, not every second of its counter', async ({ page }) => {
  const { promise: held, resolve: release } = Promise.withResolvers<void>();
  await page.route('**/study-results/exec-validation/study-result.json', async (route) => {
    await held;
    await route.continue();
  });
  await page.goto(STUDY);
  const status = page.locator('.spinner[role="status"]');
  await expect(status).toContainText(/\d+ s/);
  const tree = await status.ariaSnapshot();
  expect(tree, 'the status region text').not.toMatch(/\d+ s/);
  release();
  await expect(page.getByTestId('study-view')).toBeVisible();
});

/** WCAG contrast ratio of every visible badge's text against the surface behind it. */
async function badgeContrast(page: Page): Promise<{ text: string; ratio: number }[]> {
  return page.evaluate(() => {
    const rgb = (c: string) => (c.match(/[\d.]+/g) ?? []).map(Number);
    const lin = (v: number) => {
      const s = v / 255;
      return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
    };
    const lum = ([r, g, b]: number[]) => 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
    const surface = (el: Element | null): number[] => {
      for (let n = el; n !== null; n = n.parentElement) {
        const c = rgb(getComputedStyle(n).backgroundColor);
        if (c.length === 3 || (c.length === 4 && c[3] === 1)) return c.slice(0, 3);
      }
      return [0, 0, 0];
    };
    return Array.from(document.querySelectorAll('main :is(.badge, .stc-class, .pill, [data-pair-state])'))
      .filter((e) => e.getBoundingClientRect().width > 0)
      .map((e) => {
        const fg = lum(rgb(getComputedStyle(e).color));
        const bg = lum(surface(e));
        return { text: (e.textContent ?? '').trim().slice(0, 40), ratio: (Math.max(fg, bg) + 0.05) / (Math.min(fg, bg) + 0.05) };
      });
  });
}

test('badges and pair states reach 4.5:1 against their surface', async ({ page }) => {
  for (const path of ['/research', STUDY, `${STUDY}?cell=${CELL}`, '/queue-lab', '/market/l3-demo?tab=book', '/compare/set?demo=execution_validation']) {
    await page.goto(path);
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
    await expect(page.locator('.spinner')).toHaveCount(0);
    const badges = await badgeContrast(page);
    expect(badges.length, `${path}: badges found`).toBeGreaterThan(0);
    expect(badges.filter((b) => b.ratio < 4.5), `${path}: badges below 4.5:1`).toEqual([]);
  }
});
