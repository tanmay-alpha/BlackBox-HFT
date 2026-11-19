// Phone layout stability: the research home and the Market Data Workbench keep their
// cumulative layout shift under 0.05 at 390x844 while their data loads, under the perf harness's
// mobile profile (`web/perf/profiles.mjs`: DPR 3, touch, 4x CPU, Slow 4G), where the shifts showed.
import { expect, test, type Page } from '@playwright/test';

test.use({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true });

async function throttle(page: Page): Promise<void> {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Network.enable');
  await cdp.send('Network.emulateNetworkConditions', {
    offline: false,
    latency: 150,
    downloadThroughput: (1.6 * 1e6) / 8,
    uploadThroughput: (750 * 1e3) / 8,
  });
  await cdp.send('Emulation.setCPUThrottlingRate', { rate: 4 });
}

/** Every layout shift (without recent input) from page start, with the shifted nodes. */
async function recordShifts(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const shifts: { value: number; sources: string[] }[] = [];
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) {
        if (!('hadRecentInput' in e) || e.hadRecentInput) continue;
        const value = 'value' in e && typeof e.value === 'number' ? e.value : 0;
        const sources =
          'sources' in e && Array.isArray(e.sources)
            ? e.sources.map((s: { node?: Node | null; previousRect: DOMRectReadOnly; currentRect: DOMRectReadOnly }) => {
                const n = s.node;
                const name = n instanceof Element ? `${n.tagName.toLowerCase()}.${n.className}` : String(n?.nodeName);
                return `${name} y ${Math.round(s.previousRect.y)}->${Math.round(s.currentRect.y)} h ${Math.round(s.previousRect.height)}->${Math.round(s.currentRect.height)}`;
              })
            : [];
        shifts.push({ value, sources });
      }
      document.documentElement.dataset.cls = JSON.stringify(shifts);
    }).observe({ type: 'layout-shift', buffered: true });
  });
}

async function cls(page: Page): Promise<number> {
  // Let late data and fonts settle before reading the total.
  await page.waitForLoadState('networkidle');
  await page.waitForTimeout(1500);
  const raw = (await page.locator('html').getAttribute('data-cls')) ?? '[]';
  const shifts: { value: number; sources: string[] }[] = JSON.parse(raw);
  const total = shifts.reduce((s, x) => s + x.value, 0);
  console.log(`${page.url()} CLS ${total.toFixed(4)} ${JSON.stringify(shifts)}`);
  return total;
}

for (const path of ['/research', '/market', '/market/l3-demo?tab=book']) {
  test(`${path} keeps CLS under 0.05 on a 390x844 phone`, async ({ page }) => {
    test.setTimeout(120_000);
    await recordShifts(page);
    await throttle(page);
    await page.goto(path);
    expect(await cls(page)).toBeLessThan(0.05);
  });
}
