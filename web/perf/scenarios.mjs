// Scenarios of the browser performance harness. Each one is its own navigation in its own page.
//
// `ready(P)` and an interaction's `baseline(P)` / `result(P, base)` run inside the page once per
// animation frame (see probe.mjs) and are serialised with `toString`, so they use only `P`
// helpers (`P.vis`, `P.count`, `P.text`) and the DOM. Selectors come from `src/features/*`.
// `act` runs in Node and performs real input: `press` is a mouse click on the desktop profile and
// a touchscreen tap on the mobile profile.

export const RUN_1MS = '253bbc23b342f373';
/** Run compared with RUN_1MS by the stored demo comparison (`blackbox demo seed`). */
export const RUN_PAIR_B = 'd6a57aed972e18d0';

/** Explore: the headline comparison row holds numbers (`Results.tsx`, table.ex-compare). */
const exploreNumbers = (P) => {
  const cell = document.querySelector('table.ex-compare tbody td.ex-num');
  return P.vis(cell) && /\d/.test(cell.textContent || '');
};

/**
 * Explore: the price chart canvas has a size and the replay window is loaded
 * (`ReplayPanel.tsx`: `.ex-chart-wrap` carries `is-stale` until the window's data is current).
 */
const exploreChart = (P) => {
  const wrap = document.querySelector('.ex-chart-wrap');
  if (!wrap || wrap.classList.contains('is-stale')) return false;
  const canvas = wrap.querySelector('.ex-chart-host canvas');
  return P.vis(canvas);
};

/** Replay terminal: heatmap canvas sized, ladder rows present, cursor clock shown (`ReplayTerminalPage.tsx`). */
const replayReady = (P) =>
  P.vis(document.querySelector('.heatmap-wrap canvas')) && P.count('table.ladder tbody tr') > 0 && /\d/.test(P.text('.book-cursor') || '');

/** Run overview: the order blotter grid has data rows (`RunOverviewPage.tsx`, DataTable label "Orders"). */
const blotterReady = (P) => P.count('table[aria-label="Orders"] tbody tr:not(.dt-pad)') > 1 && /showing/.test(P.text('.pager span') || '');

/** Strategy Lab: preset cards from the API are rendered (`LabPage.tsx`; one card exists without presets). */
const labReady = (P) => P.count('.lab-cards label.lab-card') >= 2 && P.count('#lab-dataset option') > 0;

/** Risk: the run metrics grid has rows and a chart canvas has a size (`RiskPage.tsx`). */
const riskReady = (P) =>
  P.count('table[aria-label="Run metrics"] tbody tr:not(.dt-pad)') > 0 && P.vis(document.querySelector('.tv-lightweight-charts canvas'));

/** Pair comparison: the measured outcomes table has rows and the equity overlay SVG is drawn (`ComparePage.tsx`, `EquityOverlay.tsx`). */
const compareReady = (P) => P.count('table.cmp-metrics tbody tr') > 0 && P.vis(document.querySelector('svg.cmp-equity-svg'));

/** Report: the rendered Markdown report has headings and the manifest is present (`ReportsPage.tsx`). */
const reportReady = (P) => P.count('.report-heading') > 0 && P.count('[data-testid="report-manifest"]') > 0;

const cursorText = (P) => P.text('.book-cursor');

export const SCENARIOS = [
  {
    id: 'explore-load',
    title: 'Explore `/`: result numbers visible',
    path: '/',
    load: true,
    readyDesc: 'first numeric cell of table.ex-compare is visible',
    ready: exploreNumbers,
  },
  {
    id: 'explore-first-chart',
    title: 'Explore `/`: price chart canvas sized and replay window loaded',
    path: '/',
    load: true,
    readyDesc: '.ex-chart-wrap is not .is-stale and its .ex-chart-host canvas has non-zero size',
    ready: exploreChart,
  },
  {
    id: 'explore-scenario-switch',
    title: 'Explore `/`: switch the scenario filter to the slow scenario',
    path: '/',
    load: false,
    readyDesc: 'price chart ready (as explore-first-chart)',
    ready: exploreChart,
    interactions: [
      {
        id: 'switch-scenario',
        input: 'click / tap the third `.ex-seg` button (slow scenario)',
        resultDesc: 'URL has scenario=slow and the button is aria-pressed',
        result: (P) => {
          const b = document.querySelectorAll('button.ex-seg')[2];
          return new URLSearchParams(location.search).get('scenario') === 'slow' && b && b.getAttribute('aria-pressed') === 'true';
        },
        act: ({ page, press }) => press(page.locator('button.ex-seg').nth(2)),
      },
    ],
  },
  {
    id: 'replay-load',
    title: `Order book replay \`/book/runs/${RUN_1MS}\``,
    path: `/book/runs/${RUN_1MS}`,
    load: true,
    readyDesc: '.heatmap-wrap canvas has non-zero size, table.ladder has rows, .book-cursor shows a time',
    ready: replayReady,
  },
  {
    id: 'replay-seek',
    title: `Order book replay \`/book/runs/${RUN_1MS}\`: step one event, then scrub`,
    path: `/book/runs/${RUN_1MS}`,
    load: false,
    readyDesc: 'as replay-load',
    ready: replayReady,
    interactions: [
      {
        id: 'seek-step',
        input: 'desktop: ArrowRight key; mobile: tap "Next event"',
        resultDesc: '.book-cursor text changed',
        baseline: cursorText,
        result: (P, base) => {
          const t = P.text('.book-cursor');
          return t !== null && t !== base;
        },
        act: ({ page, profile, press }) =>
          profile.hasTouch ? press(page.locator('button[aria-label="Next event"]')) : page.keyboard.press('ArrowRight'),
      },
      {
        id: 'seek-scrub',
        input: 'click / tap the time slider #term-scrub at 60 % of its width',
        resultDesc: '.book-cursor text changed, ladder has rows and no .spinner is shown in the workspace',
        baseline: cursorText,
        result: (P, base) => {
          const t = P.text('.book-cursor');
          return t !== null && t !== base && P.count('table.ladder tbody tr') > 0 && P.count('main .spinner') === 0;
        },
        act: async ({ page, press }) => {
          const slider = page.locator('#term-scrub');
          const box = await slider.boundingBox();
          await press(slider, { position: { x: box.width * 0.6, y: box.height / 2 } });
        },
      },
    ],
  },
  {
    id: 'order-select',
    title: 'Explore `/`: select an order row',
    path: '/',
    load: false,
    readyDesc: 'price chart ready and the window order table has rows',
    ready: (P) => {
      const wrap = document.querySelector('.ex-chart-wrap');
      return Boolean(wrap) && !wrap.classList.contains('is-stale') && P.vis(wrap.querySelector('canvas')) && P.count('.ex-orders button.ex-linkbtn') > 0;
    },
    interactions: [
      {
        id: 'select-order',
        input: 'click / tap the second "Inspect order" button of the window order table',
        resultDesc: 'URL has order=…, the lifecycle heading names the order and the lifecycle panel (.ex-life) is rendered',
        result: (P) =>
          new URLSearchParams(location.search).has('order') && /^Order \d+ in both/.test(P.text('#ex-life-title') || '') && P.count('.ex-life') > 0,
        act: ({ page, press }) => press(page.locator('.ex-orders button.ex-linkbtn').nth(1)),
      },
    ],
  },
  {
    id: 'runs-table',
    title: `Run overview \`/forensics/${RUN_1MS}\`: order blotter, scroll, next page`,
    path: `/forensics/${RUN_1MS}`,
    load: true,
    readyDesc: 'table[aria-label="Orders"] has data rows and its pager text is shown',
    ready: blotterReady,
    interactions: [
      {
        id: 'scroll-table',
        input: 'desktop: mouse-wheel scroll gesture of 1500 px over the blotter (CDP Input.synthesizeScrollGesture); mobile: repeated touch drags (CDP Input.dispatchTouchEvent) across the visible part of the blotter totalling at least 1500 px',
        resultDesc: 'the blotter scroller (or an ancestor) moved by at least 200 px',
        baseline: () => {
          let el = document.querySelector('table[aria-label="Orders"]');
          let sum = 0;
          for (; el; el = el.parentElement) sum += el.scrollTop;
          return sum;
        },
        result: (P, base) => {
          let el = document.querySelector('table[aria-label="Orders"]');
          let sum = 0;
          for (; el; el = el.parentElement) sum += el.scrollTop;
          return sum - base >= 200;
        },
        // Centre the blotter in the viewport before arming, so that move is not counted.
        prepare: ({ page }) =>
          page.locator('.dt-scroll', { has: page.locator('table[aria-label="Orders"]') }).evaluate((el) => el.scrollIntoView({ block: 'center', behavior: 'instant' })),
        act: async ({ page, cdp, profile }) => {
          const box = await page.locator('.dt-scroll', { has: page.locator('table[aria-label="Orders"]') }).boundingBox();
          const vp = profile.viewport;
          // Visible part of the scroller, clear of the sticky app bar and the bottom status bar.
          const top = Math.max(box.y, 70);
          const bottom = Math.min(box.y + box.height, vp.height - 70);
          const x = Math.round(Math.min(vp.width - 10, box.x + box.width / 2));
          if (!profile.hasTouch) {
            await cdp.send('Input.synthesizeScrollGesture', { x, y: Math.round((top + bottom) / 2), yDistance: -1500, speed: 1500, gestureSourceType: 'mouse' });
            return;
          }
          // Input.synthesizeScrollGesture does not scroll under phone emulation (no movement observed),
          // so the touch profile drags with raw touch events: bottom to top of the visible part,
          // 8 moves 16 ms apart, then the finger rests 150 ms before lifting (no fling, so the
          // next tap is a tap and not a fling stop); repeated until 1500 px of finger travel.
          const y0 = bottom - 10;
          const y1 = top + 10;
          const steps = 8;
          if (y0 - y1 < 40) throw new Error(`blotter visible height ${Math.round(bottom - top)} px is too small to drag`);
          for (let travel = 0; travel < 1500; travel += y0 - y1) {
            await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x, y: y0 }] });
            for (let i = 1; i <= steps; i += 1) {
              await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ x, y: y0 + ((y1 - y0) * i) / steps }] });
              await page.waitForTimeout(16);
            }
            await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ x, y: y1 }] });
            await page.waitForTimeout(150);
            await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
          }
        },
      },
      {
        id: 'next-page',
        input: 'click / tap the pager\'s "Next" button',
        resultDesc: 'pager text changed (the next 100 orders are shown)',
        // Wait until no scroller around the blotter has moved for 500 ms (the previous scroll ended).
        prepare: ({ page }) =>
          page.waitForFunction(
            () => {
              let sum = 0;
              for (let el = document.querySelector('table[aria-label="Orders"]'); el; el = el.parentElement) sum += el.scrollTop;
              const now = performance.now();
              const s = (window.__bbperfScroll ??= { sum, since: now });
              if (s.sum !== sum) {
                s.sum = sum;
                s.since = now;
              }
              return now - s.since >= 500;
            },
            null,
            { timeout: 15_000, polling: 50 },
          ),
        baseline: (P) => P.text('.pager span'),
        result: (P, base) => {
          const t = P.text('.pager span');
          return t !== null && t !== base && P.count('table[aria-label="Orders"] tbody tr:not(.dt-pad)') > 1;
        },
        act: ({ page, press }) => press(page.locator('.pager').first().getByRole('button', { name: 'Next', exact: true })),
      },
    ],
  },
  {
    id: 'lab-load',
    title: 'Strategy Lab `/strategy`',
    path: '/strategy',
    load: true,
    readyDesc: 'at least two .lab-card preset cards and the dataset select has options',
    ready: labReady,
  },
  {
    id: 'lab-preset-switch',
    title: 'Strategy Lab `/strategy`: choose another preset card',
    path: '/strategy',
    load: false,
    readyDesc: 'as lab-load',
    ready: labReady,
    interactions: [
      {
        id: 'choose-preset',
        input: 'click / tap the first preset card that is not loaded',
        // 7c49dae marks the loaded card `label.lab-card-on`; Phase 3.1 marks its wrapper `.lab-preset-on`.
        resultDesc: 'the title of the loaded card (.lab-card-on / .lab-preset-on .lab-card-title) changed',
        baseline: (P) => P.text('.lab-card-on .lab-card-title, .lab-preset-on .lab-card-title'),
        result: (P, base) => {
          const t = P.text('.lab-card-on .lab-card-title, .lab-preset-on .lab-card-title');
          return t !== null && t !== base;
        },
        act: ({ page, press }) =>
          press(
            page
              .locator('.lab-cards label.lab-card:not(.lab-card-on) .lab-card-title, .lab-cards .lab-preset:not(.lab-preset-on) label.lab-card .lab-card-title')
              .first(),
          ),
      },
    ],
  },
  {
    id: 'risk-load',
    title: `Risk & P&L \`/risk/${RUN_1MS}\``,
    path: `/risk/${RUN_1MS}`,
    load: true,
    readyDesc: 'table[aria-label="Run metrics"] has rows and a chart canvas has non-zero size',
    ready: riskReady,
  },
  {
    id: 'compare-load',
    title: `Pair comparison \`/compare/pair?a=${RUN_1MS}&b=${RUN_PAIR_B}\``,
    path: `/compare/pair?a=${RUN_1MS}&b=${RUN_PAIR_B}`,
    load: true,
    readyDesc: 'table.cmp-metrics has rows and svg.cmp-equity-svg has non-zero size',
    ready: compareReady,
  },
  {
    id: 'report-load',
    title: `Report \`/reports/${RUN_1MS}\``,
    path: `/reports/${RUN_1MS}`,
    load: true,
    readyDesc: 'rendered report headings (.report-heading) and the manifest list are present',
    ready: reportReady,
  },
  {
    id: 'mobile-nav',
    title: 'Navigation from `/` to Strategy Lab (mobile: menu sheet; desktop: rail link)',
    path: '/',
    load: false,
    readyDesc: 'Explore result numbers visible (as explore-load)',
    ready: exploreNumbers,
    interactions: [
      {
        id: 'open-menu',
        only: 'mobile',
        skipReason: 'the desktop layout has a permanent navigation rail and no menu to open',
        input: 'tap .nav-menu-button',
        resultDesc: 'the workspace sheet (.nav-sheet) and its links are visible',
        result: (P) => P.vis(document.querySelector('.nav-sheet a.nav-sheet-item')),
        act: ({ page, press }) => press(page.locator('button.nav-menu-button')),
      },
      {
        id: 'go-workspace',
        input: 'mobile: tap the "Strategy Lab" sheet link; desktop: click the "Strategy Lab" rail link',
        resultDesc: 'pathname is /strategy and the Strategy Lab preset cards are rendered (includes its chunk and API reads)',
        result: (P) => location.pathname === '/strategy' && P.count('.lab-cards label.lab-card') >= 2,
        act: ({ page, profile, press }) =>
          press(
            profile.hasTouch
              ? page.locator('a.nav-sheet-item', { hasText: 'Strategy Lab' })
              : page.locator('a.rail-item[aria-label="Strategy Lab"]'),
          ),
      },
    ],
  },
];
