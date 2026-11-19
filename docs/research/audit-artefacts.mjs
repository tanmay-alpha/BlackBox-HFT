// Independent, read-only audit of the published bytes and transcribed research tables.
// node docs/research/audit-artefacts.mjs [local-data-directory]
// The optional local directory contains queue-shadow/{fixed-v2,p10-oos}; no raw feeds are read.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const text = (path) => readFileSync(resolve(root, path), 'utf8');
let values = 0;
let rows = 0;
const failures = [];
function equal(actual, expected, context) {
  values++;
  if (actual !== expected) failures.push(`${context}: ${actual} != ${expected}`);
}
function numbers(actual, expected, context) {
  const tokens = actual.replaceAll(',', '').replaceAll('−', '-').match(/-?\d+(?:\.\d+)?/g) ?? [];
  equal(tokens.length, expected.length, `${context} number count`);
  tokens.forEach((token, i) => {
    if (i >= expected.length) return;
    values++;
    const precision = token.includes('.') ? token.split('.')[1].length : 0;
    if (Math.abs(Number(token) - expected[i]) > 0.500001 * 10 ** -precision) {
      failures.push(`${context}: ${token} != ${expected[i]}`);
    }
  });
}
const scalar = (v) => v.status === 'available' ? [v.value] : [];
const interval = (v) => [...scalar(v.lower), ...scalar(v.upper)];
const span = (s) => Number(s.replaceAll(',', '').match(/[\d.]+/)[0]) * (s.includes('µs') ? 1e3 : 1e6);
function hashed(path, schema, version, contract) {
  const bytes = readFileSync(resolve(root, path));
  const hash = createHash('sha256').update(bytes).digest('hex');
  equal(text(`${path}.sha256`).trim(), `${hash}  study-result.json`, `${path} SHA-256`);
  const value = JSON.parse(bytes);
  equal(value.schema, schema, `${path} schema`);
  equal(value.schema_version, version, `${path} version`);
  if (contract) assert.match(text(contract), new RegExp(`SCHEMA_VERSION = ${version};`));
  console.log(`${path}: ${bytes.length} bytes, ${hash}, ${schema} v${version}`);
  return value;
}
const study = hashed('web/public/study-results/exec-validation/study-result.json', 'blackbox.study-result', 1, 'web/src/features/study/contract.ts');
const synthetic = hashed('web/public/study-results/queue-shadow/study-result.json', 'blackbox.queue-shadow', 1, 'web/src/features/queuelab/contract.ts');
const sources = { synthetic };
if (process.argv[2]) {
  const data = resolve(process.argv[2]);
  for (const [key, dir] of Object.entries({ july: 'fixed-v2/insample', october: 'fixed-v2/oos-1030', december: 'fixed-v2/oos-1230', january2019: 'p10-oos/oos-01302019', january2020: 'p10-oos/oos-01302020' })) {
    sources[key] = hashed(resolve(data, 'queue-shadow', dir, 'study-result.json'), 'blackbox.queue-shadow', 1);
  }
}
let source;
let header;
for (const [index, line] of text('docs/research/QUEUE_SHADOW_EXPERIMENT.md').split('\n').entries()) {
  if (line.startsWith('### 6.1 ')) source = 'synthetic';
  if (line.startsWith('### 6.2 ')) source = 'july';
  if (line.startsWith('#### A.')) source = 'october';
  if (line.startsWith('#### B.')) source = 'december';
  if (line.startsWith('### 6.4 ')) source = undefined;
  if (line.startsWith('#### C.')) source = 'january2019';
  if (line.startsWith('#### D.')) source = 'january2020';
  if (line.startsWith('## 7.')) source = undefined;
  if (line.startsWith('| Symbol |')) header = line.split('|').slice(1, -1).map((v) => v.trim());
  if (!sources[source] || !/^\| (SYNTH|QQQ|AAPL|AMD|AMZN|SPY) \|/.test(line)) continue;
  const q = sources[source];
  const cols = line.split('|').slice(1, -1).map((v) => v.trim());
  const symbol = cols[0];
  const latency = span(cols[1]);
  const context = `QUEUE_SHADOW_EXPERIMENT.md:${index + 1}`;
  const cells = q.results.cells.filter((c) => c.symbol === symbol && c.latency_ns === latency);
  rows++;
  if (header[2].startsWith('τ ')) {
    q.design.horizons_ns.forEach((tau, i) => {
      const v = q.results.cancelled_volume.find((c) => c.symbol === symbol && c.latency_ns === latency && c.horizon_ns === tau);
      numbers(cols[i + 2], [...scalar(v.share_ahead), v.ahead_shares, v.ahead_shares + v.behind_shares], context);
    });
  } else if (header[2] === 'Checkpoint') {
    const checkpoint = span(cols[2]);
    const checkpoints = q.results.checkpoints.filter((c) => c.symbol === symbol && c.latency_ns === latency && c.checkpoint_ns === checkpoint);
    const ra = checkpoints.find((c) => c.model === 'risk_adverse');
    numbers(cols[3], [ra.pairs], context);
    numbers(cols[4], scalar(ra.mean_l3_ahead_shares), context);
    header.slice(5).forEach((model, i) => {
      const c = checkpoints.find((c) => c.model === model);
      numbers(cols[i + 5], [...scalar(c.mean_abs_error_shares), ...scalar(c.mean_signed_error_shares)], context);
    });
  } else {
    const tau = span(cols[2]);
    const at = cells.filter((c) => c.horizon_ns === tau);
    if (header[3] === 'n') {
      const l3 = at.find((c) => c.model === 'l3_shadow');
      numbers(cols[3], [l3.eligible], context);
      numbers(cols[4], [...scalar(l3.fill_fraction), ...interval(l3.fill_fraction_95)], context);
      numbers(cols[5], [l3.fill_causes.queue, l3.fill_causes.trade_through, l3.fill_causes.book_cross], context);
      header.slice(6).forEach((model, i) => {
        const c = at.find((c) => c.model === model).vs_l3;
        numbers(cols[i + 6], [...scalar(c.difference), ...interval(c.difference_95)], context);
      });
    } else {
      header.slice(3).forEach((model, i) => {
        const v = at.find((c) => c.model === model).km_median_ns;
        numbers(cols[i + 3], scalar(v).map((v) => v / 1e6), context);
      });
    }
  }
}
const metric = (m) => m.kind === 'deterministic_single' ? [m.value] : [m.bootstrap.mean, m.bootstrap.lower_95, m.bootstrap.upper_95];
let section = '';
for (const [index, line] of text('docs/research/EXECUTION_MODEL_VALIDATION.md').split('\n').entries()) {
  if (line.startsWith('#')) section = line;
  if (!line.startsWith('| ')) continue;
  const c = line.split('|').slice(1, -1).map((s) => s.trim().replaceAll('`', ''));
  const context = `EXECUTION_MODEL_VALIDATION.md:${index + 1}`;
  if (section === '### Sensitivity tables' && study.aggregates.some((a) => a.regime === c[0])) {
    const a = study.aggregates.find((a) => a.regime === c[0] && a.queue_model === c[1] && a.latency === c[2] && a.feed_offset_ns === Number(c[3]));
    assert.ok(a, context);
    rows++;
    numbers(c[4], [a.members.length], context);
    numbers(c[5], metric(a.metrics.fill_rate), context);
    numbers(c[6], metric(a.metrics.net_pnl), context);
  }
  if (section === '### Execution, inventory and queue summaries' && study.cells.some((a) => a.regime === c[0])) {
    const run = study.cells.find((a) => a.regime === c[0] && a.design === 'regime_baseline');
    const m = run.measures;
    rows++;
    numbers(c[1], [m.maker_fills, m.taker_fills], context);
    numbers(c[2], [m.unfilled_orders], context);
    numbers(c[3], [m.time_to_fill_p50_ns / 1e6, m.time_to_fill_p90_ns / 1e6], context);
    numbers(c[4], [m.modeled_queue_at_entry_mean], context);
    numbers(c[5], [m.mean_abs_position, m.max_abs_position], context);
    numbers(c[6], [m.fees], context);
  }
  if (/^\d+$/.test(c[0]) && /^[0-9a-f]{16}$/.test(c[1])) {
    const run = study.demo_anchors.runs.find((a) => a.run_id === c[1]);
    assert.ok(run, context);
    rows++;
    numbers(c[0], [run.index], context);
    numbers(c[6], [run.measures.fills, run.orders.submitted], context);
    numbers(c[7], [run.measures.fill_rate], context);
    numbers(c[8], [run.accounting.fees], context);
    numbers(c[9], [run.accounting.net_pnl], context);
  }
  if (/^\d vs \d /.test(c[0])) {
    const [a, b] = c[0].match(/\d/g).map(Number);
    const run = study.demo_anchors.runs.find((r) => r.index === b);
    const reference = study.demo_anchors.runs.find((r) => r.index === a);
    rows++;
    equal(run.comparison.reference_run_id, reference.run_id, context);
    equal(c[1].startsWith('aligned'), run.comparison.aligned, context);
    if (run.comparison.aligned) {
      const counts = run.comparison.lifecycle_counts;
      numbers(c[2], [counts.identical], context);
      numbers(c[3], ['same_fills_different_history', 'same_outcome_different_timing', 'same_decision_different_outcome', 'different_decision'].map((k) => counts[k]), context);
      numbers(c[4], [counts.only_a, counts.only_b], context);
    }
  }
  if (section === '### Where paths first differ' && /^[0-9a-f]{16}$/.test(c[1])) {
    const run = study.cells.find((r) => r.run_id === c[1]);
    assert.ok(run, context);
    rows++;
    const d = run.comparison.first_divergence;
    numbers(c[2], [d.order_id, Number(d.decision_ts_ns) / 1e6], context);
    const counts = run.comparison.lifecycle_counts;
    numbers(c[4], ['same_fills_different_history', 'same_outcome_different_timing', 'same_decision_different_outcome', 'different_decision'].map((k) => counts[k]), context);
  }
}
for (const [source, label] of [['october', 'OOS-A:'], ['january2019', 'OOS-C:'], ['january2020', 'OOS-D:']]) {
  if (!sources[source]) continue;
  const q = sources[source];
  const line = text('docs/research/QUEUE_SHADOW_EXPERIMENT.md').split('\n').find((l) => l.startsWith(`| ${label}`));
  const c = line.split('|').slice(1, -1).map((s) => s.trim());
  const contrasts = q.results.cells.filter((v) => v.vs_l3);
  numbers(c[3], [q.hypotheses.cells.filter((v) => v.h_a_supported).length, q.hypotheses.cells.length], `${source} H-A`);
  numbers(c[4], [q.hypotheses.cells.reduce((n, v) => n + v.closer_than_risk_adverse.length, 0), q.hypotheses.cells.length * 8], `${source} H-B1`);
  numbers(c[6], [contrasts.filter((v) => v.vs_l3.excludes_zero).length, contrasts.length], `${source} H0`);
  numbers(c[7], [Math.max(...contrasts.map((v) => Math.abs(v.vs_l3.difference.value)))], `${source} max delta`);
}
console.log(`Checked ${rows} document rows and ${values} values/counts; local queue sets: ${Object.keys(sources).join(', ')}.`);
if (failures.length) {
  console.error(failures.join('\n'));
  process.exitCode = 1;
}
