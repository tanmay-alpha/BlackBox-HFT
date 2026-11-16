// Summary statistics and the Markdown report of one harness run (`run.mjs` result object).

import { quantile, summarise } from './stats.mjs';

// Load metrics come from every repetition whose page load completed, even if a later interaction
// of the same repetition failed (that failure is counted under the interaction).
const ok = (scenario) => scenario.reps.filter((r) => r.load);

function modeOf(values) {
  const counts = new Map();
  for (const v of values) counts.set(v, (counts.get(v) ?? 0) + 1);
  let best = null;
  for (const [v, n] of counts) if (best === null || n > best[1]) best = [v, n];
  return best ? { value: best[0], count: best[1], of: values.length } : null;
}

const lcpLabel = (e) => (e ? `${e.tag ?? 'removed element'}${e.id ? `#${e.id}` : ''}${e.cls ? `.${e.cls.trim().split(/\s+/).join('.')}` : ''}` : 'none');

export function buildSummary(result) {
  const load = {};
  const interactions = {};
  const api = new Map();
  const statics = new Map();

  for (const s of result.scenarios) {
    if (s.skipped) continue;
    const good = ok(s);
    const col = (f) => summarise(good.map(f));
    load[s.id] = {
      reps_ok: good.length,
      reps_failed: s.reps.length - good.length,
      ttfb_ms: col((r) => r.load.ttfb_ms),
      fcp_ms: col((r) => r.load.fcp_ms),
      lcp_ms: col((r) => r.load.lcp_ms),
      ready_ms: col((r) => r.load.ready_ms),
      tbt_ms: col((r) => r.load.tbt_ms),
      cls: col((r) => r.load.cls),
      long_task_count: col((r) => r.load.long_tasks.count),
      long_task_max_ms: col((r) => r.load.long_tasks.max_ms),
      js_encoded_bytes: col((r) => r.load.net.js_encoded_bytes),
      total_encoded_bytes: col((r) => r.load.net.total_encoded_bytes),
      request_count: col((r) => r.load.net.request_count),
      api_request_count: col((r) => r.load.net.api.count),
      api_preflight_count: col((r) => r.load.net.api.preflight_count),
      api_error_count: col((r) => r.load.net.api.error_count),
      api_encoded_bytes: col((r) => r.load.net.api.encoded_bytes),
      api_decoded_bytes: col((r) => r.load.net.api.decoded_bytes),
      api_slowest_ms: col((r) => {
        const d = r.load.net.api.durations_ms.filter((x) => x !== null);
        return d.length ? Math.max(...d) : null;
      }),
      js_heap_used_bytes: col((r) => r.load.js_heap_used_bytes),
      machine_cpu_util: col((r) => r.machine_cpu_util),
      lcp_element: modeOf(good.map((r) => lcpLabel(r.load.lcp_element))),
    };

    for (const r of good) {
      for (const q of r.load.net.api.requests) {
        if (q.type === 'Preflight') continue;
        const key = `${q.method} ${q.url}`;
        const a = api.get(key) ?? { key, durations: [], ttfb: [], decoded: [], encoded: [], scenarios: new Set(), statuses: new Set() };
        a.durations.push(q.duration_ms);
        a.ttfb.push(q.ttfb_ms);
        a.decoded.push(q.decoded_bytes);
        a.encoded.push(q.encoded_bytes);
        a.scenarios.add(s.id);
        a.statuses.add(q.failed ?? q.status);
        api.set(key, a);
      }
      for (const q of r.load.net.static) {
        const key = q.url.replace(result.base_url, '');
        const a = statics.get(key) ?? { key, type: q.type, encoded: [], decoded: [], durations: [], scenarios: new Set() };
        a.encoded.push(q.encoded_bytes);
        a.decoded.push(q.decoded_bytes);
        a.durations.push(q.duration_ms);
        a.scenarios.add(s.id);
        statics.set(key, a);
      }
    }

    for (const def of s.interactions ?? []) {
      const key = `${s.id} / ${def.id}`;
      const all = s.reps.map((r) => (r.interactions ?? []).find((x) => x.id === def.id)).filter(Boolean);
      const skipped = all.find((x) => x.skipped);
      if (skipped) {
        interactions[key] = { skipped: skipped.skipped };
        continue;
      }
      const done = all.filter((x) => !x.error);
      const col = (f) => summarise(done.map(f));
      interactions[key] = {
        reps_ok: done.length,
        reps_failed: s.reps.length - done.length,
        reps_with_event_entry: done.filter((x) => x.interaction_ms !== null).length,
        interaction_ms: col((x) => x.interaction_ms),
        result_ms: col((x) => x.result_ms),
        long_task_count: col((x) => x.long_tasks.count),
        long_task_max_ms: col((x) => x.long_tasks.max_ms),
        long_task_total_ms: col((x) => x.long_tasks.total_ms),
        max_frame_gap_ms: col((x) => x.max_frame_gap_ms),
        api_request_count: col((x) => x.net.api_count),
        api_slowest_ms: col((x) => x.net.api_slowest_ms),
      };
    }
  }

  const apiEndpoints = [...api.values()]
    .map((a) => ({
      request: a.key,
      samples: a.durations.length,
      scenarios: [...a.scenarios],
      statuses: [...a.statuses],
      duration_ms: summarise(a.durations),
      ttfb_ms: summarise(a.ttfb),
      decoded_bytes_median: quantile(a.decoded, 0.5),
      encoded_bytes_median: quantile(a.encoded, 0.5),
    }))
    .sort((x, y) => (y.duration_ms.median ?? 0) - (x.duration_ms.median ?? 0));
  const staticResources = [...statics.values()]
    .map((a) => ({
      url: a.key,
      type: a.type,
      samples: a.encoded.length,
      scenarios: [...a.scenarios],
      encoded_bytes_median: quantile(a.encoded, 0.5),
      decoded_bytes_median: quantile(a.decoded, 0.5),
      duration_ms: summarise(a.durations),
    }))
    .sort((x, y) => (y.encoded_bytes_median ?? 0) - (x.encoded_bytes_median ?? 0));

  // Bytes on the wire over every recorded repetition (page loads plus interactions; warm-mode
  // priming loads and loads that never reached the ready marker are not included).
  const totals = { total_encoded_bytes: 0, api_encoded_bytes: 0, api_requests: 0 };
  for (const s of result.scenarios) {
    for (const r of s.reps ?? []) {
      if (!r.load) continue;
      totals.total_encoded_bytes += r.load.net.total_encoded_bytes;
      totals.api_encoded_bytes += r.load.net.api.encoded_bytes;
      totals.api_requests += r.load.net.api.count;
      for (const i of r.interactions ?? []) {
        if (!i.net) continue;
        totals.total_encoded_bytes += i.net.total_encoded_bytes;
        totals.api_encoded_bytes += i.net.api_encoded_bytes;
        totals.api_requests += i.net.api_count;
      }
    }
  }

  return { load, interactions, totals, api_endpoints: apiEndpoints, static_resources: staticResources };
}

// ----- Markdown ---------------------------------------------------------------------------------

const DASH = '–';
const num = (v, digits = 0) => (v === null || v === undefined ? DASH : v.toFixed(digits));
const triple = (s, digits = 0) => (!s || s.n === 0 ? DASH : `${num(s.median, digits)} / ${num(s.p75, digits)} / ${num(s.max, digits)}`);
const kib = (v) => (v === null || v === undefined ? DASH : (v / 1024).toFixed(1));
const mib = (v) => (v === null || v === undefined ? DASH : (v / 1024 / 1024).toFixed(1));
const pct = (v) => (v === null || v === undefined ? DASH : `${(v * 100).toFixed(0)} %`);
const table = (head, rows) => [`| ${head.join(' | ')} |`, `| ${head.map(() => '---').join(' | ')} |`, ...rows.map((r) => `| ${r.join(' | ')} |`)].join('\n');
const cut = (s, n) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);

export function renderMarkdown(result) {
  const { summary: sum, profile: p, environment: env, api_status: st } = result;
  const out = [];
  out.push(`# Browser performance: ${result.label} / ${p.name} / ${result.cache_mode} cache`);
  out.push('');
  out.push('## Environment');
  out.push('');
  out.push('```');
  out.push(`label            ${result.label}`);
  out.push(`started          ${result.started_at}`);
  out.push(`finished         ${result.finished_at}`);
  out.push(`base URL         ${result.base_url}`);
  out.push(`API base         ${result.api_base} (${result.api_base_source})`);
  out.push(`API git_sha      ${st.git_sha ?? 'unknown'}`);
  out.push(`API /status      ${st.ms.map((x) => num(x)).join(' ms, ')} ms (HTTP ${st.http_status.join(', ')}; 3 sequential GETs from Node, unthrottled, before the first page load)`);
  out.push(`profile          ${p.name}: viewport ${p.viewport.width}x${p.viewport.height}, DPR ${p.deviceScaleFactor}, touch ${p.hasTouch ? 'yes' : 'no'}, mobile ${p.isMobile ? 'yes' : 'no'}`);
  out.push(`user agent       ${result.user_agent}`);
  out.push(`CPU throttle     ${p.cpuThrottle}x (Emulation.setCPUThrottlingRate)`);
  out.push(`network          ${p.network.name}`);
  out.push(`                 Network.emulateNetworkConditions: latency ${p.network.latencyMs} ms, download ${p.network.downloadBytesPerSec} B/s, upload ${p.network.uploadBytesPerSec} B/s`);
  out.push(`cache mode       ${result.cache_mode}${result.cache_mode === 'cold' ? ' (new browser context per repetition: empty HTTP cache and storage)' : ' (one browser context, one priming load per scenario before the measured repetitions)'}`);
  out.push(`repetitions      ${result.reps}, pause between page loads ${result.pause_ms} ms`);
  out.push(`browser          Chromium ${env.chromium_version} (${env.headless ? 'headless' : 'headed'}), @playwright/test ${env.playwright_version}`);
  out.push(`node             ${env.node_version}`);
  out.push(`OS               ${env.os}`);
  out.push(`CPU              ${env.cpu_model}, ${env.cpu_count} logical CPUs, ${env.memory_gib} GiB RAM`);
  out.push(`machine CPU use  whole run ${pct(result.machine_cpu_util)}; per scenario repetition median ${pct(result.machine_cpu_util_per_rep.median)}, max ${pct(result.machine_cpu_util_per_rep.max)}`);
  out.push(`run transfer     ${mib(sum.totals.total_encoded_bytes)} MiB total, of which API ${mib(sum.totals.api_encoded_bytes)} MiB in ${sum.totals.api_requests} requests (all recorded repetitions, loads + interactions)`);
  out.push('```');
  out.push('');

  const loadScenarios = result.scenarios.filter((s) => s.load && !s.skipped);
  out.push('## Page loads: timings');
  out.push('');
  out.push('Milliseconds from navigation start, `median / p75 / max` over the successful repetitions. CLS is unitless. TBT is lab Total Blocking Time between FCP and the end of the measurement window (ready marker, then 2 s without a long task or a network request). Not field data.');
  out.push('');
  out.push(
    table(
      ['scenario', 'ok', 'TTFB', 'FCP', 'LCP', 'ready', 'TBT', 'CLS', 'long tasks', 'longest task'],
      loadScenarios.map((s) => {
        const x = sum.load[s.id];
        return [
          s.id,
          `${x.reps_ok}/${x.reps_ok + x.reps_failed}`,
          triple(x.ttfb_ms),
          triple(x.fcp_ms),
          triple(x.lcp_ms),
          triple(x.ready_ms),
          triple(x.tbt_ms),
          triple(x.cls, 3),
          triple(x.long_task_count),
          triple(x.long_task_max_ms),
        ];
      }),
    ),
  );
  out.push('');
  out.push('## Page loads: bytes, requests, heap');
  out.push('');
  out.push('Medians over the successful repetitions, up to the end of the measurement window. KiB = 1024 bytes; transferred = encoded bytes on the wire. API slowest = the longest API request of a repetition.');
  out.push('');
  out.push(
    table(
      ['scenario', 'JS KiB', 'total KiB', 'requests', 'API requests', 'API errors (max)', 'API preflights', 'API KiB transferred', 'API KiB decoded', 'API slowest ms (median / p75 / max)', 'JS heap MiB'],
      loadScenarios.map((s) => {
        const x = sum.load[s.id];
        return [
          s.id,
          kib(x.js_encoded_bytes.median),
          kib(x.total_encoded_bytes.median),
          num(x.request_count.median),
          num(x.api_request_count.median),
          num(x.api_error_count.max),
          num(x.api_preflight_count.median),
          kib(x.api_encoded_bytes.median),
          kib(x.api_decoded_bytes.median),
          triple(x.api_slowest_ms),
          mib(x.js_heap_used_bytes.median),
        ];
      }),
    ),
  );
  out.push('');
  out.push('## Page loads: LCP element');
  out.push('');
  out.push(
    table(
      ['scenario', 'most frequent LCP element', 'repetitions'],
      loadScenarios.map((s) => {
        const m = sum.load[s.id].lcp_element;
        return [s.id, m ? `\`${cut(m.value, 90)}\`` : DASH, m ? `${m.count}/${m.of}` : DASH];
      }),
    ),
  );
  out.push('');

  out.push('## Interactions');
  out.push('');
  out.push('`interaction_ms` is the largest Event Timing `duration` among the `event` and `first-input` entries with an `interactionId` after the input (lab interaction latency, not field INP; durations are rounded to 8 ms; `event` entries under 16 ms are not reported, the page\'s single `first-input` entry is reported at any duration; null means no entry, not zero). `result` is the time from the trusted input event to the visible result. `median / p75 / max`; long tasks and frame gap cover input to result plus the settle time.');
  out.push('');
  const iRows = [];
  for (const s of result.scenarios) {
    if (s.skipped) continue;
    for (const def of s.interactions ?? []) {
      const x = sum.interactions[`${s.id} / ${def.id}`];
      if (!x) continue;
      if (x.skipped) {
        iRows.push([`${s.id} / ${def.id}`, 'skipped', DASH, DASH, DASH, DASH, DASH, DASH, DASH]);
        continue;
      }
      iRows.push([
        `${s.id} / ${def.id}`,
        `${x.reps_ok}/${x.reps_ok + x.reps_failed}`,
        `${x.reps_with_event_entry}/${x.reps_ok}`,
        triple(x.interaction_ms),
        triple(x.result_ms),
        triple(x.long_task_count),
        triple(x.long_task_max_ms),
        triple(x.max_frame_gap_ms),
        num(x.api_request_count.median),
      ]);
    }
  }
  out.push(table(['scenario / interaction', 'ok', 'with Event Timing entry', 'interaction_ms', 'result ms', 'long tasks', 'longest task ms', 'max frame gap ms', 'API requests (median)'], iRows));
  out.push('');

  out.push('## Slowest API requests during page loads');
  out.push('');
  out.push('All successful repetitions of all scenarios, grouped by method and URL, sorted by median duration (request sent to last byte, under the emulated network).');
  out.push('');
  out.push(
    table(
      ['request', 'samples', 'duration ms (median / p75 / max)', 'time to headers ms (median)', 'KiB decoded', 'KiB transferred', 'status'],
      sum.api_endpoints
        .slice(0, 25)
        .map((a) => [`\`${cut(a.request, 110)}\``, String(a.samples), triple(a.duration_ms), num(a.ttfb_ms.median), kib(a.decoded_bytes_median), kib(a.encoded_bytes_median), a.statuses.join(', ')]),
    ),
  );
  out.push('');
  out.push('## Largest non-API resources during page loads');
  out.push('');
  out.push(
    table(
      ['resource', 'type', 'KiB transferred (median)', 'KiB decoded (median)', 'duration ms (median / p75 / max)', 'scenarios'],
      sum.static_resources
        .slice(0, 20)
        .map((a) => [`\`${cut(a.url, 80)}\``, a.type, kib(a.encoded_bytes_median), kib(a.decoded_bytes_median), triple(a.duration_ms), String(a.scenarios.length)]),
    ),
  );
  out.push('');

  out.push('## Skipped and failed');
  out.push('');
  const notes = [];
  for (const s of result.scenarios) {
    if (s.skipped) notes.push(`- \`${s.id}\`: skipped: ${s.skipped}`);
    for (const def of s.interactions ?? []) {
      const x = sum.interactions[`${s.id} / ${def.id}`];
      if (x?.skipped) notes.push(`- \`${s.id} / ${def.id}\`: skipped: ${x.skipped}`);
    }
    for (const r of s.reps ?? []) {
      if (r.error) notes.push(`- \`${s.id}\` repetition ${r.rep}: failed at ${r.error.stage}: ${cut(r.error.message.replace(/\s+/g, ' '), 300)}`);
      for (const i of r.interactions ?? []) {
        if (!r.error && i.error) notes.push(`- \`${s.id} / ${i.id}\` repetition ${r.rep}: ${cut(i.error.replace(/\s+/g, ' '), 300)}`);
        if (!i.skipped && !i.error && i.interaction_ms === null) notes.push(`- \`${s.id} / ${i.id}\` repetition ${r.rep}: no Event Timing entry (interaction_ms is null)`);
      }
      if (r.load && !r.load.quiet_reached) notes.push(`- \`${s.id}\` repetition ${r.rep}: the ${result.limits.quietMs} ms quiet window was not reached within ${result.limits.quietCapMs} ms`);
    }
  }
  out.push(notes.length ? notes.join('\n') : 'None.');
  out.push('');

  out.push('## Scenarios');
  out.push('');
  out.push(table(['scenario', 'path', 'ready marker'], result.scenarios.map((s) => [s.id, `\`${s.path}\``, s.ready_desc])));
  out.push('');
  return out.join('\n');
}
