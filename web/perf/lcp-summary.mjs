// Summarise the exact navigation files emitted by lcp-ab.mjs.
// Usage: node perf/lcp-summary.mjs [label=p10-ab] [reps=3]
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const label = process.argv[2] ?? 'p10-ab';
const reps = Number(process.argv[3] ?? 3);
if (!/^[\w.-]+$/.test(label) || !Number.isInteger(reps) || reps < 3) throw new Error('Expected label and reps >= 3');
const median = (values) => {
  if (values.some((x) => typeof x !== 'number' || !Number.isFinite(x))) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};
const result = { label, reps, scenarios: {} };
const cpu = [];
for (const scenario of ['explore-load', 'replay-load', 'shell-palette', 'shell-help']) {
  const builds = {};
  for (const build of ['base', 'final']) {
    const rows = [];
    for (let rep = 1; rep <= reps; rep += 1) {
      const path = resolve(root, `data/perf/${label}-${build}-${rep}-${scenario}/mobile.json`);
      const run = JSON.parse(readFileSync(path, 'utf8'));
      const sample = run.scenarios.find((s) => s.id === scenario)?.reps[0];
      if (!sample || sample.error) throw new Error(`Failed navigation: ${path}: ${JSON.stringify(sample?.error)}`);
      if (sample.load.net.api.error_count) throw new Error(`API error in ${path}`);
      const opening = sample.interactions?.[0];
      if (opening?.error) throw new Error(`Failed interaction: ${path}: ${opening.error}`);
      cpu.push(run.machine_cpu_util);
      result.environment ??= run.environment;
      rows.push({
        lcp_ms: sample.load.lcp_ms,
        fcp_ms: sample.load.fcp_ms,
        tbt_ms: sample.load.tbt_ms,
        cls: sample.load.cls,
        js_bytes: sample.load.net.js_encoded_bytes,
        ready_ms: sample.load.ready_ms,
        ...(opening ? {
          open_ms: opening.result_ms,
          interaction_ms: opening.interaction_ms,
          open_js_bytes: opening.net.requests.filter((r) => r.type === 'Script').reduce((n, r) => n + r.encoded_bytes, 0),
        } : {}),
      });
    }
    builds[build] = {
      samples: rows,
      median: Object.fromEntries(Object.keys(rows[0]).map((key) => [key, median(rows.map((r) => r[key]))])),
      cls_max: Math.max(...rows.map((r) => r.cls)),
    };
  }
  result.scenarios[scenario] = builds;
}
result.machine_cpu_util = { min: Math.min(...cpu), median: median(cpu), max: Math.max(...cpu) };
console.log(JSON.stringify(result, null, 2));
