// Summarise the runs written by session.mjs: per profile, scenario and build, the median of every load metric
// (CLS: maximum) and of every interaction's result and Event Timing duration, plus the raw samples.
//
//   node perf/session-summary.mjs <label> <build[,build...]> [reps=3]
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const [label, buildList, repArg] = process.argv.slice(2);
const reps = Number(repArg ?? 3);
if (!label || !/^[\w.-]+$/.test(label) || !buildList || !Number.isInteger(reps) || reps < 3) throw new Error('usage: <label> <builds> [reps>=3]');
const builds = buildList.split(',');
const median = (xs) => {
  const v = xs.filter((x) => typeof x === 'number' && Number.isFinite(x)).sort((a, b) => a - b);
  if (v.length !== xs.length || !v.length) return null;
  const m = Math.floor(v.length / 2);
  return v.length % 2 ? v[m] : (v[m - 1] + v[m]) / 2;
};
const out = { label, reps, profiles: {} };
const cpu = [];
for (const profile of ['mobile', 'desktop']) {
  const scenarios = {};
  for (const build of builds) {
    for (let rep = 1; rep <= reps; rep += 1) {
      const path = resolve(root, `data/perf/${label}-${build}-${rep}/${profile}.json`);
      if (!existsSync(path)) continue;
      const run = JSON.parse(readFileSync(path, 'utf8'));
      cpu.push(run.machine_cpu_util);
      out.environment ??= run.environment;
      for (const s of run.scenarios) {
        const sample = s.reps[0];
        const row = ((scenarios[s.id] ??= {})[build] ??= { samples: [] });
        if (!sample || sample.error) {
          row.samples.push({ rep, error: sample?.error?.message ?? 'missing' });
          continue;
        }
        const l = sample.load;
        row.samples.push({
          rep,
          lcp_ms: l.lcp_ms,
          lcp_element: l.lcp_element ? `${l.lcp_element.tag}${l.lcp_element.cls ? `.${l.lcp_element.cls.trim().split(/\s+/).join('.')}` : ''}` : null,
          fcp_ms: l.fcp_ms,
          tbt_ms: l.tbt_ms,
          blocking_ms: l.long_tasks?.blocking_ms ?? null,
          cls: l.cls,
          ready_ms: l.ready_ms,
          js_bytes: l.net.js_encoded_bytes,
          api_errors: l.net.api.error_count,
          interactions: Object.fromEntries(
            (sample.interactions ?? []).filter((i) => !i.skipped).map((i) => [i.id, { result_ms: i.result_ms, interaction_ms: i.interaction_ms, long_task_ms: i.long_tasks?.total_ms ?? null, error: i.error ?? null }]),
          ),
        });
      }
    }
  }
  for (const byBuild of Object.values(scenarios)) {
    for (const row of Object.values(byBuild)) {
      const ok = row.samples.filter((r) => !r.error);
      row.ok = ok.length;
      if (!ok.length) continue;
      row.median = Object.fromEntries(['lcp_ms', 'fcp_ms', 'tbt_ms', 'blocking_ms', 'ready_ms', 'js_bytes'].map((k) => [k, median(ok.map((r) => r[k]))]));
      row.cls_max = Math.max(...ok.map((r) => r.cls));
      row.api_errors = ok.reduce((n, r) => n + (r.api_errors ?? 0), 0);
      const ids = [...new Set(ok.flatMap((r) => Object.keys(r.interactions)))];
      row.interactions = Object.fromEntries(
        ids.map((id) => {
          const xs = ok.map((r) => r.interactions[id]).filter(Boolean);
          return [id, { n: xs.length, errors: xs.filter((x) => x.error).length, result_ms: median(xs.map((x) => x.result_ms)), interaction_ms: median(xs.map((x) => x.interaction_ms)), long_task_ms: median(xs.map((x) => x.long_task_ms)) }];
        }),
      );
    }
  }
  out.profiles[profile] = scenarios;
}
out.machine_cpu_util = cpu.length ? { min: Math.min(...cpu), median: median(cpu), max: Math.max(...cpu) } : null;
console.log(JSON.stringify(out, null, 2));
