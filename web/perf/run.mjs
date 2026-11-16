// Browser performance harness for the web console.
//
//   node perf/run.mjs --base <url> --profile desktop|mobile --reps N --label <name>
//                     [--only <scenario,...>] [--warm] [--pause-ms <ms>] [--api <api base>] [--headed]
//
// Drives the installed Playwright Chromium against `--base`, with CPU and network emulation set
// through CDP, and writes `data/perf/<label>/<profile>.json` (raw values of every repetition)
// and `<profile>.md` (summary, also printed to stdout). Read-only: it only navigates and uses
// view controls; it never submits a job. Progress goes to stderr.
//
// A Vercel Security Checkpoint (bot challenge) aborts the run with `vercel_security_checkpoint`;
// nothing measured after it is kept, and no attempt is made to pass it.

import { chromium } from '@playwright/test';
import { mkdirSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import os from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { NetLog } from './net.mjs';
import { LIMITS, PROFILES } from './profiles.mjs';
import { installProbe } from './probe.mjs';
import { buildSummary, renderMarkdown } from './report.mjs';
import { SCENARIOS } from './scenarios.mjs';
import { cls, cpuUtil, longTasks, summarise } from './stats.mjs';

const VERCEL_UI_HOST = 'blackbox-hft.vercel.app';
const VERCEL_API = 'https://blackbox-hft-api.onrender.com/api/v1';
const CHECKPOINT = 'vercel_security_checkpoint';
const HERE = dirname(fileURLToPath(import.meta.url));
const log = (...a) => process.stderr.write(`${a.join(' ')}\n`);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

class CheckpointError extends Error {
  constructor(detail) {
    super(`${CHECKPOINT}: ${detail}`);
    this.code = CHECKPOINT;
    this.detail = detail;
  }
}

const isLoopback = (base) => ['127.0.0.1', 'localhost', '[::1]'].includes(new URL(base).hostname);

function parseArgs(argv) {
  const out = { base: null, profile: null, reps: 1, label: null, only: null, warm: false, api: null, headed: false, pauseMs: null };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    const value = () => {
      i += 1;
      if (i >= argv.length) throw new Error(`${a} needs a value`);
      return argv[i];
    };
    if (a === '--base') out.base = value().replace(/\/+$/, '');
    else if (a === '--profile') out.profile = value();
    else if (a === '--reps') out.reps = Number(value());
    else if (a === '--label') out.label = value();
    else if (a === '--only') out.only = value().split(',').map((s) => s.trim()).filter(Boolean);
    else if (a === '--api') out.api = value().replace(/\/+$/, '');
    else if (a === '--pause-ms') out.pauseMs = Number(value());
    else if (a === '--warm') out.warm = true;
    else if (a === '--headed') out.headed = true;
    else throw new Error(`unknown argument ${a}`);
  }
  if (!out.base) throw new Error('--base <url> is required');
  new URL(out.base);
  // Remote hosts get a pause between page loads unless one is given; a loopback server needs none.
  out.pauseMs ??= isLoopback(out.base) ? 0 : 4000;
  if (!Number.isFinite(out.pauseMs) || out.pauseMs < 0) throw new Error('--pause-ms must be a non-negative number');
  if (!PROFILES[out.profile]) throw new Error(`--profile must be one of ${Object.keys(PROFILES).join(', ')}`);
  if (!Number.isInteger(out.reps) || out.reps < 1) throw new Error('--reps must be a positive integer');
  if (!out.label || !/^[\w.-]+$/.test(out.label)) throw new Error('--label <name> is required (letters, digits, . _ -)');
  if (out.only) {
    const unknown = out.only.filter((id) => !SCENARIOS.some((s) => s.id === id));
    if (unknown.length) throw new Error(`unknown scenario ${unknown.join(', ')}; known: ${SCENARIOS.map((s) => s.id).join(', ')}`);
  }
  return out;
}

async function timedGet(url, timeoutMs) {
  const t0 = performance.now();
  try {
    const res = await fetch(url, { signal: AbortSignal.timeout(timeoutMs), headers: { accept: 'application/json' } });
    const body = await res.text();
    let json = null;
    try {
      json = JSON.parse(body);
    } catch {
      json = null;
    }
    return { ms: performance.now() - t0, status: res.status, json };
  } catch (e) {
    return { ms: performance.now() - t0, status: null, json: null, error: String(e?.message ?? e) };
  }
}

/** Three sequential `GET <api>/status` from Node (unthrottled); the first may include a cold start. */
async function apiStatus(api) {
  const calls = [];
  for (let i = 0; i < 3; i += 1) calls.push(await timedGet(`${api}/status`, i === 0 ? 180_000 : 60_000));
  const body = calls.find((c) => c.json)?.json ?? null;
  return {
    url: `${api}/status`,
    ms: calls.map((c) => c.ms),
    http_status: calls.map((c) => c.status),
    errors: calls.map((c) => c.error ?? null),
    git_sha: body?.git_sha ?? null,
    blackbox_version: body?.blackbox_version ?? null,
  };
}

const serialise = (fn) => (fn ? fn.toString() : 'null');

/** Waits until neither a long task nor a request happened for `quietMs` after the ready marker. */
async function waitQuiet(page, net) {
  const started = Date.now();
  while (Date.now() - started < LIMITS.quietCapMs) {
    const s = await page.evaluate(() => {
      const P = window.__bbperf;
      return { now: performance.now(), ready: P.ready, lastLong: P.longtasks.reduce((m, x) => Math.max(m, x.t + x.d), 0) };
    });
    if (s.now - Math.max(s.lastLong, s.ready) >= LIMITS.quietMs && net.quietFor() >= LIMITS.quietMs) return true;
    await sleep(250);
  }
  return false;
}

function deriveLoad(snap, net, quietReached, heap) {
  const fcp = snap.paints['first-contentful-paint'] ?? null;
  const last = snap.lcp.length ? snap.lcp[snap.lcp.length - 1] : null;
  const all = longTasks(snap.longtasks, 0, snap.now, LIMITS.longTaskMs);
  return {
    ttfb_ms: snap.nav?.responseStart ?? null,
    fcp_ms: fcp,
    lcp_ms: last ? last.t : null,
    lcp_element: last ? { tag: last.tag, cls: last.cls, id: last.id, size: last.size, url: last.url, text: last.text } : null,
    lcp_candidates: snap.lcp.length,
    cls: cls(snap.shifts, snap.now),
    layout_shift_count: snap.shifts.filter((s) => !s.input).length,
    tbt_ms: fcp === null ? null : longTasks(snap.longtasks, fcp, snap.now, LIMITS.longTaskMs).blocking_ms,
    long_tasks: { count: all.count, max_ms: all.max_ms, total_ms: all.total_ms },
    ready_ms: snap.ready,
    dom_content_loaded_ms: snap.nav?.domContentLoadedEventEnd ?? null,
    load_event_ms: snap.nav?.loadEventEnd ?? null,
    measure_end_ms: snap.now,
    quiet_reached: quietReached,
    js_heap_used_bytes: heap,
    unsupported_entry_types: snap.unsupported,
    net,
  };
}

async function runInteraction(page, cdp, profile, net, def) {
  if (def.only && def.only !== profile.name) return { id: def.id, skipped: def.skipReason };
  const press = (locator, options = {}) => (profile.hasTouch ? locator.tap(options) : locator.click(options));
  // `prepare` (optional) positions the page before the watcher is armed, so it is not measured.
  if (def.prepare) {
    try {
      await def.prepare({ page, profile });
      await page.waitForTimeout(LIMITS.settleMs);
    } catch (e) {
      return { id: def.id, error: `prepare: ${String(e?.message ?? e)}` };
    }
  }
  const mark = net.mark();
  const t0 = await page.evaluate(`window.__bbperf.arm(${serialise(def.result)}, ${serialise(def.baseline)})`);
  let error = null;
  try {
    await def.act({ page, cdp, profile, press });
    await page.waitForFunction(() => window.__bbperf.watch.done !== null, null, { timeout: LIMITS.resultTimeoutMs, polling: 50 });
  } catch (e) {
    error = String(e?.message ?? e);
  }
  await page.waitForTimeout(LIMITS.settleMs);
  const { w, snap } = await page.evaluate(() => ({ w: window.__bbperf.stopWatch(), snap: window.__bbperf.snapshot() }));

  const inputs = snap.inputs.filter((i) => i.t >= t0);
  const inputT = inputs.length ? Math.min(...inputs.map((i) => i.t)) : null;
  // `first-input` repeats the first interaction's entry; keep one entry per event name and time.
  const seen = new Set();
  const entries = snap.events
    .filter((e) => e.t >= t0)
    .filter((e) => {
      const k = `${e.name}@${e.t}`;
      if (seen.has(k)) return false;
      seen.add(k);
      return true;
    });
  const lt = longTasks(snap.longtasks, t0, w.stopped, LIMITS.longTaskMs);
  const n = net.summary(mark);
  const apiDurations = n.api.durations_ms.filter((x) => x !== null);
  return {
    id: def.id,
    ...(error ? { error } : {}),
    interaction_ms: entries.length ? Math.max(...entries.map((e) => e.d)) : null,
    interaction_ids: [...new Set(entries.map((e) => e.id))].length,
    event_entries: entries.map((e) => ({ name: e.name, source: e.source, start_ms: e.t, duration_ms: e.d, input_delay_ms: e.ps - e.t, processing_ms: e.pe - e.ps })),
    input_events: inputs.map((i) => i.type),
    // Without a captured trusted input event the result time is counted from arming the watcher.
    result_from: inputT === null ? 'armed' : 'input-event',
    result_ms: w.done === null ? null : w.done - (inputT ?? t0),
    long_tasks: { count: lt.count, max_ms: lt.max_ms, total_ms: lt.total_ms },
    frames: w.frames,
    max_frame_gap_ms: w.maxGap,
    window_ms: w.stopped - t0,
    net: {
      request_count: n.request_count,
      total_encoded_bytes: n.total_encoded_bytes,
      api_count: n.api.count,
      api_error_count: n.api.error_count,
      api_decoded_bytes: n.api.decoded_bytes,
      api_encoded_bytes: n.api.encoded_bytes,
      api_slowest_ms: apiDurations.length ? Math.max(...apiDurations) : null,
      requests: [...n.api.requests, ...n.static],
    },
  };
}

/** One navigation of one scenario in its own page. `shared` is the warm-mode context. */
async function runScenario({ browser, shared, contextOptions, scenario, profile, args, apiBase, rep, failDir, record = true }) {
  const cpuBefore = os.cpus();
  const started = Date.now();
  const rec = { rep, started_at: new Date(started).toISOString() };
  const context = shared ?? (await browser.newContext(contextOptions));
  let stage = 'open page';
  let page = null;
  let checkpoint = null;
  try {
    page = await context.newPage();
    await page.addInitScript(`(${installProbe.toString()})(${scenario.ready.toString()})`);
    const cdp = await context.newCDPSession(page);
    await cdp.send('Network.enable');
    await cdp.send('Performance.enable');
    await cdp.send('Network.emulateNetworkConditions', {
      offline: false,
      latency: profile.network.latencyMs,
      downloadThroughput: profile.network.downloadBytesPerSec,
      uploadThroughput: profile.network.uploadBytesPerSec,
    });
    await cdp.send('Emulation.setCPUThrottlingRate', { rate: profile.cpuThrottle });
    const net = new NetLog(cdp, apiBase);
    // Any response carrying the challenge token header, or a challenge page body, is a checkpoint.
    let challenged = null;
    page.on('response', (r) => {
      if (challenged === null && r.headers()['x-vercel-challenge-token'] !== undefined) challenged = `HTTP ${r.status()} ${r.url()}`;
    });
    checkpoint = async () => {
      if (challenged !== null) throw new CheckpointError(challenged);
      const text = await page.evaluate(() => `${document.title}\n${document.body ? document.body.innerText.slice(0, 2000) : ''}`).catch(() => '');
      if (/Vercel Security Checkpoint/i.test(text)) throw new CheckpointError(`checkpoint page at ${page.url()}`);
    };

    stage = 'navigate';
    const response = await page.goto(`${args.base}${scenario.path}`, { waitUntil: 'commit', timeout: LIMITS.readyTimeoutMs });
    if (response && response.status() === 403 && response.headers()['x-vercel-challenge-token'] !== undefined) {
      throw new CheckpointError(`HTTP 403 ${response.url()}`);
    }
    stage = 'wait for ready marker';
    try {
      await page.waitForFunction(() => window.__bbperf && window.__bbperf.ready !== null, null, { timeout: LIMITS.readyTimeoutMs, polling: 100 });
    } finally {
      // Whatever was observed is kept even when the ready marker never appears.
      rec.partial = await page
        .evaluate(() => (window.__bbperf ? window.__bbperf.snapshot() : null))
        .then((s) => (s ? { fcp_ms: s.paints['first-contentful-paint'] ?? null, lcp_ms: s.lcp.at(-1)?.t ?? null, url: s.url, net: net.summary().api.requests } : null))
        .catch(() => null);
    }
    delete rec.partial;
    await checkpoint();
    stage = 'wait for quiet window';
    const quiet = await waitQuiet(page, net);
    stage = 'collect load metrics';
    const snap = await page.evaluate(() => window.__bbperf.snapshot());
    const netSummary = net.summary();
    await cdp.send('HeapProfiler.collectGarbage');
    const metrics = await cdp.send('Performance.getMetrics');
    const heap = metrics.metrics.find((m) => m.name === 'JSHeapUsedSize')?.value ?? null;
    rec.load = deriveLoad(snap, netSummary, quiet, heap);
    rec.url_after_load = snap.url;

    if (scenario.interactions && record) {
      rec.interactions = [];
      for (const def of scenario.interactions) {
        stage = `interaction ${def.id}`;
        const r = await runInteraction(page, cdp, profile, net, def);
        rec.interactions.push(r);
        if (r.error) {
          await checkpoint();
          throw new Error(r.error);
        }
      }
    }
  } catch (e) {
    if (e instanceof CheckpointError) throw e;
    if (page && checkpoint) {
      try {
        await checkpoint();
      } catch (c) {
        if (c instanceof CheckpointError) throw c;
      }
    }
    rec.error = { stage, message: String(e?.message ?? e) };
    if (page && record) {
      try {
        mkdirSync(failDir, { recursive: true });
        const file = join(failDir, `${profile.name}-${scenario.id}-rep${rep}.png`);
        await page.screenshot({ path: file, timeout: 15_000 });
        rec.error.screenshot = file;
      } catch {
        // A screenshot is a debugging aid only.
      }
    }
  } finally {
    if (shared) await page?.close().catch(() => {});
    else await context.close().catch(() => {});
  }
  rec.wall_ms = Date.now() - started;
  rec.machine_cpu_util = cpuUtil(cpuBefore, os.cpus());
  return rec;
}

/** `--api`, else the production API for the Vercel UI host, else the same-origin `<base>/api/v1`. */
function resolveApi(args) {
  if (args.api) return { api: args.api, source: '--api' };
  if (new URL(args.base).hostname === VERCEL_UI_HOST) return { api: VERCEL_API, source: `default for ${VERCEL_UI_HOST}` };
  return { api: `${args.base}/api/v1`, source: 'same origin as --base' };
}

function environment(chromiumVersion, headless) {
  const require = createRequire(import.meta.url);
  const cpus = os.cpus();
  return {
    chromium_version: chromiumVersion,
    headless,
    playwright_version: require('@playwright/test/package.json').version,
    node_version: process.version,
    os: `${os.type()} ${os.release()} (${os.version()}), ${os.arch()}`,
    cpu_model: cpus[0]?.model.trim() ?? 'unknown',
    cpu_count: cpus.length,
    memory_gib: Math.round((os.totalmem() / 2 ** 30) * 10) / 10,
  };
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const profile = PROFILES[args.profile];
  const cacheMode = args.warm ? 'warm' : 'cold';
  const outDir = resolve(HERE, '../../data/perf', args.label);
  const stem = profile.name;
  mkdirSync(outDir, { recursive: true });

  const startedAt = new Date();
  const cpuStart = os.cpus();
  const { api: apiBase, source: apiBaseSource } = resolveApi(args);
  log(`API base ${apiBase} (${apiBaseSource}); checking /status`);
  const status = await apiStatus(apiBase);
  log(`API /status ${status.ms.map(Math.round).join(', ')} ms (HTTP ${status.http_status.join(', ')}); git_sha ${status.git_sha}`);

  const browser = await chromium.launch({ headless: !args.headed });
  const chromiumVersion = browser.version();
  const userAgent = profile.userAgent ? profile.userAgent.replace('CHROME_VERSION', `${chromiumVersion.split('.')[0]}.0.0.0`) : null;
  const contextOptions = {
    viewport: profile.viewport,
    deviceScaleFactor: profile.deviceScaleFactor,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    ...(userAgent ? { userAgent } : {}),
  };
  const selected = SCENARIOS.filter((s) => !args.only || args.only.includes(s.id));
  const scenarios = selected.map((s) => ({
    id: s.id,
    title: s.title,
    path: s.path,
    load: s.load,
    ready_desc: s.readyDesc,
    interactions: (s.interactions ?? []).map((i) => ({ id: i.id, input: i.input, result_desc: i.resultDesc })),
    ...(s.only && s.only !== profile.name ? { skipped: s.skipReason } : {}),
    reps: [],
  }));
  const failDir = join(outDir, 'failures');
  let loads = 0;
  // Every page load except the first waits `--pause-ms` first (rate limit towards remote hosts).
  const run = async (opts) => {
    if (loads > 0 && args.pauseMs > 0) await sleep(args.pauseMs);
    loads += 1;
    return runScenario({ browser, contextOptions, profile, args, apiBase, failDir, ...opts });
  };
  let effectiveUa = null;
  let aborted = null;

  try {
    const shared = args.warm ? await browser.newContext(contextOptions) : null;
    {
      const ctx = shared ?? (await browser.newContext(contextOptions));
      const p = await ctx.newPage();
      effectiveUa = await p.evaluate(() => navigator.userAgent);
      await p.close();
      if (!shared) await ctx.close();
    }
    if (shared) {
      for (const [i, s] of selected.entries()) {
        if (scenarios[i].skipped) continue;
        log(`priming ${s.id}`);
        const r = await run({ shared, scenario: s, rep: -1, record: false });
        scenarios[i].priming = r.error ? { error: r.error } : { ready_ms: r.load.ready_ms, total_encoded_bytes: r.load.net.total_encoded_bytes };
      }
    }
    for (let rep = 0; rep < args.reps; rep += 1) {
      for (const [i, s] of selected.entries()) {
        if (scenarios[i].skipped) continue;
        const r = await run({ shared, scenario: s, rep });
        scenarios[i].reps.push(r);
        log(
          `rep ${rep + 1}/${args.reps} ${s.id}: ${
            r.error
              ? `FAILED at ${r.error.stage}: ${r.error.message.split('\n')[0]}`
              : `ready ${Math.round(r.load.ready_ms)} ms, LCP ${r.load.lcp_ms === null ? 'null' : Math.round(r.load.lcp_ms)} ms, TBT ${r.load.tbt_ms === null ? 'null' : Math.round(r.load.tbt_ms)} ms`
          } (${Math.round(r.wall_ms / 1000)} s)`,
        );
      }
    }
    if (shared) await shared.close();
  } catch (e) {
    if (!(e instanceof CheckpointError)) throw e;
    aborted = { error: CHECKPOINT, detail: e.detail, page_loads_started: loads };
  } finally {
    await browser.close();
  }

  const env = environment(chromiumVersion, !args.headed);
  if (aborted) {
    // Nothing from this run is a measurement: only the abort itself is written.
    const file = join(outDir, `${stem}.aborted.json`);
    const record = { schema: 1, label: args.label, started_at: startedAt.toISOString(), aborted_at: new Date().toISOString(), base_url: args.base, profile: profile.name, ...aborted, environment: env };
    writeFileSync(file, `${JSON.stringify(record, null, 1)}\n`);
    log(`ABORTED: ${CHECKPOINT} (${aborted.detail}) after ${loads} page load(s); wrote ${file}, no measurements kept`);
    process.exitCode = 3;
    return;
  }

  const result = {
    schema: 1,
    label: args.label,
    started_at: startedAt.toISOString(),
    finished_at: new Date().toISOString(),
    base_url: args.base,
    api_base: apiBase,
    api_base_source: apiBaseSource,
    api_status: status,
    cache_mode: cacheMode,
    reps: args.reps,
    pause_ms: args.pauseMs,
    only: args.only,
    profile,
    user_agent: effectiveUa,
    limits: LIMITS,
    environment: env,
    machine_cpu_util: cpuUtil(cpuStart, os.cpus()),
    machine_cpu_util_per_rep: summarise(scenarios.flatMap((s) => s.reps.map((r) => r.machine_cpu_util))),
    scenarios,
  };
  result.summary = buildSummary(result);
  const markdown = renderMarkdown(result);
  writeFileSync(join(outDir, `${stem}.json`), `${JSON.stringify(result, null, 1)}\n`);
  writeFileSync(join(outDir, `${stem}.md`), `${markdown}\n`);
  log(`wrote ${join(outDir, `${stem}.json`)} and ${stem}.md`);
  process.stdout.write(`${markdown}\n`);
  const failed = scenarios.reduce((n, s) => n + s.reps.filter((r) => r.error).length, 0);
  if (failed > 0) process.exitCode = 2;
}

main().catch((e) => {
  process.stderr.write(`${e?.stack ?? e}\n`);
  process.exitCode = 1;
});
