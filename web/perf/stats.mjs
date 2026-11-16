// Pure helpers of the performance harness: metric derivation from raw observer entries and the
// summary statistics. No browser or file access here.

/** Linear-interpolated quantile (the "type 7" definition used by NumPy and R) of finite numbers. */
export function quantile(values, q) {
  const v = values.filter((x) => typeof x === 'number' && Number.isFinite(x)).sort((a, b) => a - b);
  if (v.length === 0) return null;
  const pos = (v.length - 1) * q;
  const lo = Math.floor(pos);
  const hi = Math.ceil(pos);
  return v[lo] + (v[hi] - v[lo]) * (pos - lo);
}

export function summarise(values) {
  const v = values.filter((x) => typeof x === 'number' && Number.isFinite(x));
  if (v.length === 0) return { n: 0, median: null, p75: null, max: null };
  return { n: v.length, median: quantile(v, 0.5), p75: quantile(v, 0.75), max: Math.max(...v) };
}

/**
 * Cumulative Layout Shift as defined on web.dev: shifts without recent input are grouped into
 * session windows (a window ends after a 1 s gap or once it spans 5 s); CLS is the largest
 * window's sum. `shifts` are `{ t, v, input }` up to `until` (ms on the page timeline).
 */
export function cls(shifts, until = Infinity) {
  let best = 0;
  let sum = 0;
  let first = 0;
  let prev = -Infinity;
  for (const s of [...shifts].filter((x) => !x.input && x.t <= until).sort((a, b) => a.t - b.t)) {
    if (s.t - prev < 1000 && s.t - first < 5000) {
      sum += s.v;
    } else {
      sum = s.v;
      first = s.t;
    }
    prev = s.t;
    if (sum > best) best = sum;
  }
  return best;
}

/** Long tasks `{ t, d }` that start in `[from, to)`: count, longest, summed duration, blocking time over `threshold`. */
export function longTasks(tasks, from, to, threshold) {
  const inRange = tasks.filter((x) => x.t >= from && x.t < to);
  return {
    count: inRange.length,
    max_ms: inRange.length ? Math.max(...inRange.map((x) => x.d)) : 0,
    total_ms: inRange.reduce((a, x) => a + x.d, 0),
    blocking_ms: inRange.reduce((a, x) => a + Math.max(0, x.d - threshold), 0),
  };
}

/** CPU utilisation between two `os.cpus()` samples: 1 − idle share of all logical CPUs. */
export function cpuUtil(before, after) {
  let idle = 0;
  let total = 0;
  for (let i = 0; i < Math.min(before.length, after.length); i += 1) {
    for (const k of Object.keys(after[i].times)) {
      const d = after[i].times[k] - before[i].times[k];
      total += d;
      if (k === 'idle') idle += d;
    }
  }
  return total > 0 ? 1 - idle / total : null;
}
