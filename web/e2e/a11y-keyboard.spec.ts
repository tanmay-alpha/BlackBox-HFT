// Phase 3.1 keyboard stories at 1440×900, keyboard only: the workspace toolbar behaves as an APG
// toolbar (one tab stop, arrow keys, Home/End, roving tabindex, the "View" disclosure closes on
// Escape back to its button, the arrow-key hint shows while a roving member has keyboard focus);
// the replay's Space and arrow shortcuts act only when focus is not on a control; the skip link
// reaches the workspace and every workspace has exactly one h1. The `browserHealth` fixture fails
// any story with a console error.

import type { Locator, Page } from '@playwright/test';

import { expect, test } from './fixtures';
import { ROUTES, demoIds, openSettled } from './measure';

interface Roving {
  /** Accessible-ish names of the roving members, in order. */
  names: string[];
  /** Members with tabindex 0. */
  zero: number;
  /** Index of the focused member, -1 when focus is elsewhere. */
  active: number;
  /** Focus is inside the toolbar (member or not). */
  inside: boolean;
}

/** The roving members of the workspace toolbar as the APG pattern defines them: buttons, links,
 *  checkboxes and disclosure buttons (summaries) of this toolbar, not those in a closed disclosure. */
function roving(toolbar: Locator): Promise<Roving> {
  return toolbar.evaluate((tb) => {
    const members = Array.from(
      tb.querySelectorAll<HTMLElement>(
        'button:not([disabled]), a[href], summary, input[type="checkbox"]:not([disabled]), input[type="radio"]:not([disabled])',
      ),
    ).filter((el) => {
      if (el.closest('[role="toolbar"]') !== tb) return false;
      const closed = el.closest('details:not([open])');
      return closed === null || (el.tagName === 'SUMMARY' && el.parentElement === closed);
    });
    const focused = document.activeElement;
    return {
      names: members.map((el) => (el.getAttribute('aria-label') || el.innerText || '').replace(/\s+/g, ' ').trim()),
      zero: members.filter((el) => el.tabIndex === 0).length,
      active: focused instanceof HTMLElement ? members.indexOf(focused) : -1,
      inside: focused !== null && tb.contains(focused),
    };
  });
}

/** Presses `key` and checks focus landed on member `index` and it alone carries tabindex 0. */
async function pressTo(page: Page, toolbar: Locator, key: string, index: number): Promise<void> {
  await page.keyboard.press(key);
  const r = await roving(toolbar);
  expect(r.active, `${key} focuses member ${index} of ${JSON.stringify(r.names)}`).toBe(index);
  expect(r.zero, `${key}: exactly one member has tabindex 0`).toBe(1);
}

async function openReplay(page: Page, run: string): Promise<Locator> {
  await openSettled(page, `/book/runs/${run}`);
  const toolbar = page.locator('.ws-toolbar');
  await expect(toolbar.getByRole('button', { name: 'Play' })).toBeEnabled();
  return toolbar;
}

test('workspace toolbar: one tab stop, arrows, Home/End, roving tabindex, View disclosure, hint', async ({ page, request }) => {
  const ids = await demoIds(request);
  const toolbar = await openReplay(page, ids.run);
  const hint = toolbar.locator(':scope > .toolbar-hint');
  const cursor = page.locator('.book-cursor');
  const cursorBefore = await cursor.innerText();

  const start = await roving(toolbar);
  expect(start.names.length, `roving members: ${JSON.stringify(start.names)}`).toBeGreaterThanOrEqual(4);
  expect(start.zero, 'exactly one member has tabindex 0').toBe(1);
  await expect(hint).toBeHidden();

  // Tab enters the toolbar once: on its tabindex-0 member, then only on fields that keep their own
  // tab stop (selects, the scrubber), then out.
  let entered = false;
  for (let i = 0; i < 120 && !entered; i += 1) {
    await page.keyboard.press('Tab');
    entered = (await roving(toolbar)).inside;
  }
  expect(entered, 'Tab reaches the toolbar').toBe(true);
  const entry = await roving(toolbar);
  expect(entry.active, `Tab enters on a roving member, not ${await page.evaluate(() => document.activeElement?.outerHTML.slice(0, 80))}`).toBeGreaterThanOrEqual(0);
  await expect(page.locator(':focus')).toHaveAttribute('tabindex', '0');
  await expect(hint).toBeVisible();

  let memberStops = 1;
  let left = false;
  for (let i = 0; i < 10 && !left; i += 1) {
    await page.keyboard.press('Tab');
    const r = await roving(toolbar);
    if (r.active >= 0) memberStops += 1;
    left = !r.inside;
  }
  expect(left, 'Tab leaves the toolbar').toBe(true);
  expect(memberStops, 'Tab stops on exactly one roving member').toBe(1);
  await expect(hint).toBeHidden();

  // Shift+Tab comes back to the same single member.
  let back = -1;
  for (let i = 0; i < 10 && back < 0; i += 1) {
    await page.keyboard.press('Shift+Tab');
    back = (await roving(toolbar)).active;
  }
  expect(back).toBe(entry.active);

  // Arrows, Home and End move among the members (wrapping), with a roving tabindex.
  const last = start.names.length - 1;
  await pressTo(page, toolbar, 'Home', 0);
  await pressTo(page, toolbar, 'ArrowRight', 1);
  await expect(hint).toBeVisible();
  await pressTo(page, toolbar, 'ArrowLeft', 0);
  await pressTo(page, toolbar, 'ArrowLeft', last);
  await pressTo(page, toolbar, 'ArrowRight', 0);
  await pressTo(page, toolbar, 'End', last);
  // Arrow keys inside the toolbar move focus only; they never step the replay.
  await expect(cursor).toHaveText(cursorBefore);

  // The last member is the "View" disclosure: Enter opens it, Escape closes it and returns focus.
  const view = toolbar.locator('details.ws-view');
  const viewSummary = view.locator(':scope > summary');
  await expect(viewSummary).toBeFocused();
  await expect(viewSummary).toHaveText('View');
  await page.keyboard.press('Enter');
  await expect(view).toHaveAttribute('open', '');
  await page.keyboard.press('Tab');
  await expect(view.getByLabel('Row density')).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(view).not.toHaveAttribute('open');
  await expect(viewSummary).toBeFocused();
  // Escape on the open disclosure's own button closes it too.
  await page.keyboard.press('Enter');
  await expect(view).toHaveAttribute('open', '');
  await page.keyboard.press('Escape');
  await expect(view).not.toHaveAttribute('open');
  await expect(viewSummary).toBeFocused();
  await expect(hint).toBeVisible();
  expect((await roving(toolbar)).zero).toBe(1);
});

test('replay: skip link, Space and arrows only off controls, one h1', async ({ page, request }) => {
  const ids = await demoIds(request);
  const toolbar = await openReplay(page, ids.run);
  const play = toolbar.getByRole('button', { name: /^(Play|Pause)$/ });
  const cursor = page.locator('.book-cursor');
  await expect(page.locator('h1')).toHaveCount(1);

  // The skip link is the first tab stop and moves focus to the workspace.
  await page.keyboard.press('Tab');
  const skip = page.getByRole('link', { name: 'Skip to workspace' });
  await expect(skip).toBeFocused();
  await expect(skip).toBeInViewport({ ratio: 1 });
  await page.keyboard.press('Enter');
  await expect(page.locator('main#main')).toBeFocused();

  // Focus on the workspace, not a control: Space toggles playback, arrows step events.
  await expect(play).toHaveAttribute('aria-pressed', 'false');
  await page.keyboard.press('Space');
  await expect(play).toHaveAttribute('aria-pressed', 'true');
  await page.keyboard.press('Space');
  await expect(play).toHaveAttribute('aria-pressed', 'false');
  const t0 = await cursor.innerText();
  await page.keyboard.press('ArrowRight');
  await expect(cursor).not.toHaveText(t0);
  const t1 = await cursor.innerText();
  await page.keyboard.press('ArrowLeft');
  await expect(cursor).not.toHaveText(t1);
  await expect(page.locator('main#main')).toBeFocused();

  // Focus on the Play button: Space activates the button once (the page shortcut stands down).
  await page.keyboard.press('Tab');
  await expect(play).toBeFocused();
  await page.keyboard.press('Space');
  await expect(play).toHaveAttribute('aria-pressed', 'true');
  await page.keyboard.press('Space');
  await expect(play).toHaveAttribute('aria-pressed', 'false');

  // Focus on the scrubber (a field with its own keys): Space does not toggle playback.
  const scrub = toolbar.getByLabel('Cursor position in the dataset');
  for (let i = 0; i < 5; i += 1) {
    if (await scrub.evaluate((el) => el === document.activeElement)) break;
    await page.keyboard.press('Tab');
  }
  await expect(scrub).toBeFocused();
  await page.keyboard.press('Space');
  await expect(play).toHaveAttribute('aria-pressed', 'false');
});

for (const route of ROUTES) {
  test(`${route.name}: skip link reaches the workspace, exactly one h1`, async ({ page, request }) => {
    const path = route.path(await demoIds(request));
    await openSettled(page, path);
    const h1 = await page.locator('h1').allInnerTexts();
    // Soft: the skip-link checks below still run when the heading outline is wrong.
    expect.soft(h1, `${path}: h1 elements`).toHaveLength(1);
    await page.keyboard.press('Tab');
    const skip = page.getByRole('link', { name: 'Skip to workspace' });
    await expect(skip).toBeFocused();
    await expect(skip).toBeInViewport({ ratio: 1 });
    await page.keyboard.press('Enter');
    await expect(page.locator('main#main')).toBeFocused();
  });
}
