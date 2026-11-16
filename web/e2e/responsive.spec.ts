// Phase 3.1 responsive regression stories: on phones and a portrait tablet (touch, DPR 2) no
// workspace scrolls sideways and every visible interactive control that is not an inline prose link
// gets a hit region of at least `MIN_HIT` CSS px on both axes (44 px `--h-touch`, less the
// sampling error documented in `measure.ts`); offenders are listed in the failure message. At
// 1024×768 with a mouse, the run replay keeps its toolbar on one row, playback in reach and a
// readable depth ladder.

import type { Page } from '@playwright/test';

import { expect, test } from './fixtures';
import { MIN_HIT, ROUTES, demoIds, describeTargets, measureTargets, openSettled } from './measure';

const TOUCH = { hasTouch: true, isMobile: true, deviceScaleFactor: 2 };

/** No horizontal overflow and no undersized hit region on the page as it is now. */
async function expectTouchReady(page: Page, path: string): Promise<void> {
  const m = await measureTargets(page);
  expect(m.controls, `${path}: interactive controls found`).toBeGreaterThan(0);
  expect.soft(m.overflowX, `${path} at ${m.vw}x${m.vh}: document wider than the viewport by ${m.overflowX} px`).toBeLessThanOrEqual(0);
  const offenders = m.small.filter((t) => !t.inline);
  expect(offenders.length, `${path} at ${m.vw}x${m.vh}: hit regions below ${MIN_HIT} px:\n${describeTargets(offenders)}`).toBe(0);
}

for (const viewport of [
  { width: 390, height: 844 },
  { width: 360, height: 800 },
]) {
  test.describe(`touch ${viewport.width}x${viewport.height}`, () => {
    test.use({ viewport, ...TOUCH });
    for (const route of ROUTES) {
      test(`${route.name}: no horizontal overflow, touch targets`, async ({ page, request }) => {
        const path = route.path(await demoIds(request));
        await openSettled(page, path);
        await expectTouchReady(page, path);
      });
    }
  });
}

test.describe('touch 768x1024', () => {
  test.use({ viewport: { width: 768, height: 1024 }, ...TOUCH });
  for (const route of ROUTES.filter((r) => ['latency experiment', 'replay', 'risk'].includes(r.name))) {
    test(`${route.name}: no horizontal overflow, touch targets`, async ({ page, request }) => {
      const path = route.path(await demoIds(request));
      await openSettled(page, path);
      await expectTouchReady(page, path);
    });
  }
});

test.describe('replay at 1024x768 with a mouse', () => {
  test.use({ viewport: { width: 1024, height: 768 } });

  test('one toolbar row, playback in reach, eight ladder rows, jump fields in the disclosure', async ({ page, request }) => {
    const ids = await demoIds(request);
    await openSettled(page, `/book/runs/${ids.run}`);
    const toolbar = page.locator('.ws-toolbar');
    await expect(toolbar.getByRole('button', { name: 'Play' })).toBeEnabled();

    // Every visible toolbar control sits on the same row. Rows are compared by vertical centre, not
    // top: the scrubber (a range input) is shorter than the 44 px buttons and centred in the row,
    // so its top is lower by half the difference although nothing wraps.
    const placed = await toolbar.evaluate((tb) =>
      Array.from(tb.querySelectorAll<HTMLElement>('button, a[href], input, select, summary'))
        .filter((el) => el.closest('[role="toolbar"]') === tb && el.checkVisibility() && el.getBoundingClientRect().width > 0)
        .map((el) => {
          const r = el.getBoundingClientRect();
          const name = el.getAttribute('aria-label') || (el instanceof HTMLSelectElement ? el.labels?.[0]?.textContent : el.innerText) || el.id;
          return { name: (name ?? '').trim().slice(0, 32), top: Math.round(r.top), centre: Math.round(r.top + r.height / 2) };
        }),
    );
    expect(placed.length).toBeGreaterThanOrEqual(5);
    const spread = Math.max(...placed.map((t) => t.centre)) - Math.min(...placed.map((t) => t.centre));
    expect(spread, `toolbar controls on more than one row: ${JSON.stringify(placed)}`).toBeLessThanOrEqual(4);

    for (const control of [
      toolbar.getByRole('button', { name: 'Play' }),
      toolbar.getByRole('button', { name: 'Previous event' }),
      toolbar.getByRole('button', { name: 'Next event' }),
      toolbar.getByLabel('Speed'),
      toolbar.getByLabel('Cursor position in the dataset'),
    ]) {
      await expect(control).toBeVisible();
      await expect(control).toBeInViewport({ ratio: 1 });
    }

    // At least eight price rows of the depth ladder are fully visible in its scroller and the viewport.
    const ladder = await page.locator('.term-ladder-scroll').evaluate((sc) => {
      const box = sc.getBoundingClientRect();
      const head = sc.querySelector('thead');
      const top = Math.max(box.top, head ? head.getBoundingClientRect().bottom : box.top) - 1;
      const bottom = Math.min(box.bottom, window.innerHeight) + 1;
      const rows = Array.from(sc.querySelectorAll('tbody tr:not(.ladder-spread)'));
      const visible = rows.filter((tr) => {
        const r = tr.getBoundingClientRect();
        return r.height > 0 && r.top >= top && r.bottom <= bottom;
      });
      return { rows: rows.length, visible: visible.length, box: [Math.round(box.width), Math.round(box.height)] };
    });
    expect(ladder.visible, `fully visible ladder rows: ${JSON.stringify(ladder)}`).toBeGreaterThanOrEqual(8);

    // The jump fields live in the "Jump, view, links" disclosure.
    const more = toolbar.locator('details.term-more');
    const summary = more.locator(':scope > summary');
    await expect(summary).toHaveText('Jump, view, links');
    // By id: each form is also labelled "Jump to …", so a label lookup would match two elements.
    await expect(more.locator('input#term-jump')).toBeHidden();
    await summary.click();
    await expect(more).toHaveAttribute('open', '');
    await expect(more.locator('input#term-jump')).toBeVisible();
    await expect(more.locator('input#term-jump-order')).toBeVisible();
  });
});
