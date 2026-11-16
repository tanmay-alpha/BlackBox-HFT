// In-page layout measurements for the responsive and touch-target stories (ported from
// `data/p31/measure.js`).
//
// Hit regions are measured, not read from the box: from the centre of each visible interactive
// control, `document.elementFromPoint` finds how far left, right, up and down the hit still lands
// on the control (or a label bound to it). This counts the coarse-pointer hit frames (`::after`
// frames and padding in `styles/base.css`) and subtracts anything stacked over the control, which is
// what a finger actually gets. Each edge is found by binary search within `minSide` px of the
// centre, so the region is assumed contiguous along the centre lines; a linear 1 px scan costs
// about 1 ms per hit test on the ~10k-node pair comparison (1,000+ links), which overran the
// test timeout.
//
// Tolerance: samples are taken at integer CSS px. A frame whose edges fall on half pixels (observed
// at 390×844, DPR 2: a 44 px `::after` centred on an 18 px table link at y = 710.5 spans
// 697.5–741.5) measures 43: Chromium attributes the sample on the far half-pixel edge to the
// element underneath. That 1 px is measurement, not layout. `MIN_HIT` is the Phase 3.1 acceptance
// bar of 42 px; note it also admits frames that really are 42 px, e.g. the topbar DEMO badge (its
// `inset` frame starts at the padding box, so the 1 px border on each side is lost).
//
// Inline links inside running prose (WCAG 2.5.8's inline exception) are reported but marked
// `inline`, and the specs exempt them.

import { expect, type APIRequestContext, type Page } from '@playwright/test';

/** WCAG 2.5.5 / the `--h-touch` token. */
export const TOUCH_TARGET = 44;
/** The Phase 3.1 acceptance bar: `TOUCH_TARGET` less the tolerance explained above. */
export const MIN_HIT = TOUCH_TARGET - 2;

export interface SmallTarget {
  tag: string;
  name: string;
  cls: string;
  /** Layout box, CSS px. */
  box: [number, number];
  /** Measured hit region, CSS px. */
  hit: [number, number];
  /** An inline link inside running prose (exempt). */
  inline: boolean;
  /** Something else owns the control's centre point; `hit` is the box. */
  covered: boolean;
}

export interface Measurement {
  vw: number;
  vh: number;
  /** Document width beyond the viewport, CSS px (0 = no horizontal overflow). */
  overflowX: number;
  /** Visible interactive controls measured. */
  controls: number;
  /** Controls whose hit region is below `min` on either axis. */
  small: SmallTarget[];
}

/** Measures horizontal overflow and the hit region of every visible interactive control. */
export async function measureTargets(page: Page, min = MIN_HIT): Promise<Measurement> {
  return page.evaluate((minSide: number) => {
    const SEL =
      'a[href], button, [role="button"], [role="tab"], [role="link"], [role="menuitem"], [role="option"], [role="switch"], [role="checkbox"], [role="radio"], [role="slider"], input:not([type="hidden"]), select, textarea, summary, [tabindex]:not([tabindex="-1"])';
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const visible = (el: Element) => {
      const r = el.getBoundingClientRect();
      const cs = getComputedStyle(el);
      return (
        r.width > 0 &&
        r.height > 0 &&
        el.checkVisibility({ contentVisibilityAuto: true, visibilityProperty: true, opacityProperty: false }) &&
        cs.pointerEvents !== 'none' &&
        !el.closest('[aria-hidden="true"]') &&
        !el.closest('[inert]') &&
        !(el as HTMLButtonElement).disabled &&
        !el.closest('.visually-hidden')
      );
    };
    const els = Array.from(document.querySelectorAll<HTMLElement>(SEL))
      .filter(visible)
      .filter((el) => !el.classList.contains('skip-link'));
    const owns = (el: HTMLElement, x: number, y: number) => {
      if (x < 0 || y < 0 || x >= vw || y >= vh) return false;
      const hit = document.elementFromPoint(x, y);
      if (!hit) return false;
      if (hit === el || el.contains(hit)) return true;
      const lab = hit.closest('label');
      return lab !== null && (lab.control === el || lab.contains(el));
    };
    // WCAG 2.5.8 inline exception: an inline link inside a run of text longer than the link.
    const isInline = (el: HTMLElement) => {
      const cs = getComputedStyle(el);
      if (!cs.display.startsWith('inline') || cs.display.includes('flex') || cs.display.includes('block')) return false;
      const par = el.closest<HTMLElement>('p, li, dd, figcaption, footer, .note, .prose, .field-help');
      if (!par) return false;
      return (par.innerText || '').trim().length > (el.innerText || '').trim().length + 12;
    };
    const small: SmallTarget[] = [];
    for (const el of els) {
      el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
      const r = el.getBoundingClientRect();
      const cx = Math.round(r.left + r.width / 2);
      const cy = Math.round(r.top + r.height / 2);
      let hw: number;
      let hh: number;
      let covered = false;
      if (owns(el, cx, cy)) {
        // The farthest owned sample within `minSide` px of the centre in direction (dx, dy).
        const reach = (dx: number, dy: number) => {
          let lo = 0;
          let hi = minSide;
          while (lo < hi) {
            const mid = (lo + hi + 1) >> 1;
            if (owns(el, cx + dx * mid, cy + dy * mid)) lo = mid;
            else hi = mid - 1;
          }
          return lo;
        };
        hw = reach(-1, 0) + reach(1, 0) + 1;
        hh = reach(0, -1) + reach(0, 1) + 1;
      } else {
        covered = true;
        hw = Math.round(r.width);
        hh = Math.round(r.height);
      }
      if (Math.min(hw, hh) < minSide) {
        small.push({
          tag: el.tagName.toLowerCase(),
          name: (el.getAttribute('aria-label') || el.innerText || el.getAttribute('title') || el.getAttribute('placeholder') || el.id || '')
            .replace(/\s+/g, ' ')
            .trim()
            .slice(0, 48),
          cls: (typeof el.className === 'string' ? el.className : '').slice(0, 50),
          box: [Math.round(r.width), Math.round(r.height)],
          hit: [hw, hh],
          inline: isInline(el),
          covered,
        });
      }
    }
    window.scrollTo(0, 0);
    const de = document.documentElement;
    return { vw, vh, overflowX: Math.max(de.scrollWidth, document.body.scrollWidth) - vw, controls: els.length, small };
  }, min);
}

/** One line per offender, for failure messages. */
export function describeTargets(targets: SmallTarget[]): string {
  return targets
    .map((t) => `  <${t.tag}${t.cls ? `.${t.cls.trim().split(/\s+/).join('.')}` : ''}> "${t.name}" box ${t.box.join('x')} hit ${t.hit.join('x')}${t.covered ? ' (covered)' : ''}`)
    .join('\n');
}

export interface DemoIds {
  /** The demo run labelled "cost_of_latency: 1 ms entry and response latency". */
  run: string;
  /** Another public demo run, for the pair comparison. */
  other: string;
  /** The first order of `run`. */
  order: number;
}

let cachedIds: DemoIds | null = null;

/** Run and order ids from the API (`serve.mjs` stored the demo runs before `serve` started). */
export async function demoIds(request: APIRequestContext): Promise<DemoIds> {
  if (cachedIds) return cachedIds;
  type RunsPage = { items: { run_id: string; label: string | null }[] };
  let items: RunsPage['items'] = [];
  await expect
    .poll(
      async () => {
        const res = await request.get('/api/v1/runs?workspace=demo&limit=20');
        // The v1 wire shape (`GET /api/v1/runs`), read as the other stories do.
        const body: RunsPage = res.ok() ? await res.json() : { items: [] };
        items = body.items;
        return items.length;
      },
      { timeout: 150_000, intervals: [2_000] },
    )
    .toBeGreaterThanOrEqual(2);
  const run = items.find((r) => r.label === 'cost_of_latency: 1 ms entry and response latency')?.run_id;
  expect(run, 'the 1 ms latency demo run is stored').toBeTruthy();
  const other = items.find((r) => r.run_id !== run)!.run_id;
  // The v1 wire shape of `GET /api/v1/runs/{id}/orders` (offset page).
  const orders: { items: { order_id: number }[] } = await (await request.get(`/api/v1/runs/${run}/orders?limit=1`)).json();
  expect(orders.items.length, 'the demo run has orders').toBeGreaterThan(0);
  cachedIds = { run: run!, other, order: orders.items[0].order_id };
  return cachedIds;
}

/** The workspaces every responsive and keyboard story covers. */
export const ROUTES: { name: string; path: (ids: DemoIds) => string }[] = [
  { name: 'latency experiment', path: () => '/' },
  { name: 'experiments', path: () => '/experiments' },
  { name: 'strategy lab', path: () => '/strategy' },
  { name: 'jobs', path: () => '/sim' },
  { name: 'replay', path: (i) => `/book/runs/${i.run}` },
  { name: 'runs', path: () => '/forensics' },
  { name: 'run forensics', path: (i) => `/forensics/${i.run}` },
  { name: 'order forensics', path: (i) => `/forensics/${i.run}/orders/${i.order}` },
  { name: 'pair comparison', path: (i) => `/compare/pair?a=${i.run}&b=${i.other}` },
  { name: 'risk', path: (i) => `/risk/${i.run}` },
  { name: 'report', path: (i) => `/reports/${i.run}` },
  { name: 'dataset', path: () => '/data/synthetic-demo' },
  { name: 'system', path: () => '/system' },
];

/** Opens `path` and waits until no loading indicator is left on screen and fonts are laid out.
 *  (Panels below the fold may load lazily once scrolled to, so their spinners do not count.) */
export async function openSettled(page: Page, path: string): Promise<void> {
  await page.goto(path);
  await expect(page.locator('main#main')).toBeVisible();
  await expect
    .poll(
      () =>
        page.evaluate(() =>
          Array.from(document.querySelectorAll('.spinner')).filter((s) => {
            const r = s.getBoundingClientRect();
            return r.width > 0 && r.bottom > 0 && r.top < window.innerHeight && r.right > 0 && r.left < window.innerWidth;
          }).length,
        ),
      { message: `${path}: loading indicators on screen`, timeout: 60_000 },
    )
    .toBe(0);
  await page.evaluate(() => document.fonts.ready.then(() => undefined));
}
