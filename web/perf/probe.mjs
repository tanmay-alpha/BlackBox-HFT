// In-page probe, installed with `context.addInitScript` before any page script runs. It is
// serialised with `Function.prototype.toString`, so it must not reference anything outside itself.
// `readyFn` is the scenario's ready predicate (also serialised); both run in the top frame only.

export function installProbe(readyFn) {
  if (window.top !== window || window.__bbperf) return;
  const P = {
    lcp: [],
    shifts: [],
    paints: {},
    longtasks: [],
    events: [],
    inputs: [],
    unsupported: [],
    ready: null,
    // What the page shows, per animation frame, recorded at each change (see `phase` below).
    phases: [],
    watch: null,
  };
  window.__bbperf = P;

  const observe = (type, onEntry, extra) => {
    try {
      new PerformanceObserver((list) => list.getEntries().forEach(onEntry)).observe({ type, buffered: true, ...extra });
    } catch {
      P.unsupported.push(type);
    }
  };
  observe('largest-contentful-paint', (e) => {
    const el = e.element;
    P.lcp.push({
      t: e.startTime,
      size: e.size,
      tag: el ? el.tagName.toLowerCase() : null,
      cls: el && typeof el.className === 'string' ? el.className : null,
      id: el ? el.id || null : null,
      url: e.url || null,
      text: el ? (el.textContent || '').trim().slice(0, 60) : null,
    });
  });
  observe('layout-shift', (e) => P.shifts.push({ t: e.startTime, v: e.value, input: e.hadRecentInput }));
  observe('paint', (e) => {
    P.paints[e.name] = e.startTime;
  });
  observe('longtask', (e) => P.longtasks.push({ t: e.startTime, d: e.duration }));
  const onEvent = (source) => (e) => {
    if (!e.interactionId) return;
    P.events.push({
      source,
      name: e.name,
      t: e.startTime,
      d: e.duration,
      ps: e.processingStart,
      pe: e.processingEnd,
      id: e.interactionId,
    });
  };
  observe('event', onEvent('event'), { durationThreshold: 16 });
  observe('first-input', onEvent('first-input'));

  // Trusted input timestamps (`Event.timeStamp` is on the `performance.now()` timeline), so an
  // interaction's result time is measured from the input itself, not from the automation call.
  for (const type of ['pointerdown', 'keydown', 'touchstart', 'wheel']) {
    window.addEventListener(
      type,
      (e) => {
        if (e.isTrusted) P.inputs.push({ type, t: e.timeStamp });
      },
      { capture: true, passive: true },
    );
  }

  P.vis = (el) => {
    if (!el) return false;
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  };
  P.count = (sel) => document.querySelectorAll(sel).length;
  P.text = (sel) => {
    const el = document.querySelector(sel);
    return el ? (el.textContent || '').trim() : null;
  };

  // Ready marker: the predicate is evaluated once per animation frame from document start.
  const tick = () => {
    let ok = false;
    try {
      ok = Boolean(readyFn(P));
    } catch {
      ok = false;
    }
    if (ok) P.ready = performance.now();
    else requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);

  // Visible-content phases from the first frame until the ready marker: `blank` (no text in the
  // app root), `boot` (static loading markup, no shell yet), `shell:<first 40 characters of the
  // workspace area>` (`shell:` alone = the workspace area is empty while the shell shows). Checks
  // that the workspace area is never empty while a chunk or the data loads. `textContent`, not
  // `innerText`, so sampling forces no layout.
  let lastPhase = null;
  const phase = () => {
    const root = document.getElementById('root');
    const main = document.getElementById('main');
    let label;
    if (main) label = `shell:${(main.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 40)}`;
    else label = root && (root.textContent || '').trim() ? 'boot' : 'blank';
    if (label !== lastPhase) {
      lastPhase = label;
      P.phases.push({ t: Math.round(performance.now()), label, busy: Boolean(main && main.querySelector('[aria-busy="true"]')) });
    }
    if (P.ready === null && P.phases.length < 60) requestAnimationFrame(phase);
  };
  requestAnimationFrame(phase);

  // Interaction watcher: per animation frame, records when `resultFn(P, baseline)` first holds and
  // the largest gap between frames until `stopWatch` is called.
  P.arm = (resultFn, baselineFn) => {
    const w = { armed: performance.now(), done: null, stop: false, frames: 0, maxGap: 0, base: baselineFn ? baselineFn(P) : null };
    P.watch = w;
    let last = w.armed;
    const frame = () => {
      const now = performance.now();
      w.frames += 1;
      if (now - last > w.maxGap) w.maxGap = now - last;
      last = now;
      if (w.done === null) {
        let ok = false;
        try {
          ok = Boolean(resultFn(P, w.base));
        } catch {
          ok = false;
        }
        if (ok) w.done = now;
      }
      if (!w.stop) requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
    return w.armed;
  };
  P.stopWatch = () => {
    const w = P.watch;
    if (!w) return null;
    w.stop = true;
    return { armed: w.armed, done: w.done, frames: w.frames, maxGap: w.maxGap, stopped: performance.now() };
  };

  P.snapshot = () => {
    const nav = performance.getEntriesByType('navigation')[0];
    return {
      now: performance.now(),
      phases: P.phases,
      ready: P.ready,
      lcp: P.lcp,
      shifts: P.shifts,
      paints: P.paints,
      longtasks: P.longtasks,
      events: P.events,
      inputs: P.inputs,
      unsupported: P.unsupported,
      nav: nav
        ? {
            responseStart: nav.responseStart,
            domContentLoadedEventEnd: nav.domContentLoadedEventEnd,
            loadEventEnd: nav.loadEventEnd,
            transferSize: nav.transferSize,
          }
        : null,
      url: location.pathname + location.search,
    };
  };
}
