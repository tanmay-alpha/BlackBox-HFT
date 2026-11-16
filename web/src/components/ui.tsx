// Shared presentational primitives and the data-loading hook used by every workspace
// (UI_DESIGN_SPEC.md §6.3, §6.6, §6.9, §6.10).

import { createContext, useCallback, useContext, useEffect, useId, useRef, useState } from 'react';
import type { ReactNode } from 'react';

import { describeError, isAbort } from '../api/client';
import type { Basis, Side } from '../api/types';
import { DASH, fmtClock, shortHash } from './fmt';
import { isWakingError, useApiPhase } from '../shell/health';
import { relParts, type NsDigits, type NsInput } from './ns';

// ----- Panel ------------------------------------------------------------------------------

export interface PanelProps {
  title: ReactNode;
  /** Right-aligned header controls. */
  actions?: ReactNode;
  children?: ReactNode;
  className?: string;
  /** One-line honesty caption under the header ("Observed L2. Downsampled 1:200 ms."). */
  caption?: ReactNode;
  /** Evidence basis of the panel's content, shown as a badge after the title. */
  basis?: Basis;
  /** Body without padding (tables, charts). */
  flush?: boolean;
  /** Body fills the remaining height and scrolls internally (docked panels). */
  fill?: boolean;
  /** Collapse to the header with a `^` toggle. */
  collapsible?: boolean;
}

/** A titled region with a quiet header, optional caption and body on `--bg-1`. */
export function Panel({ title, actions, children, className, caption, basis, flush, fill, collapsible }: PanelProps) {
  const [collapsed, setCollapsed] = useState(false);
  const bodyId = useId();
  const classes = ['panel', fill ? 'panel-fill' : '', className ?? ''].filter(Boolean).join(' ');
  return (
    <section className={classes}>
      <header className="panel-head">
        <h2 className="panel-title">{title}</h2>
        {basis ? <BasisBadge basis={basis} /> : null}
        <div className="panel-actions">
          {actions}
          {collapsible ? (
            <button
              type="button"
              className="btn-icon"
              aria-expanded={!collapsed}
              aria-controls={bodyId}
              aria-label={collapsed ? 'Expand panel' : 'Collapse panel'}
              title={collapsed ? 'Expand panel' : 'Collapse panel'}
              onClick={() => setCollapsed((c) => !c)}
            >
              {collapsed ? '\u02c5' : '\u02c4'}
            </button>
          ) : null}
        </div>
      </header>
      {caption && !collapsed ? <p className="panel-caption">{caption}</p> : null}
      <div id={bodyId} className={flush ? 'panel-body panel-body-flush' : 'panel-body'} hidden={collapsed}>
        {children}
      </div>
    </section>
  );
}

// ----- Badge ------------------------------------------------------------------------------

export type BadgeKind =
  | 'observed'
  | 'modeled'
  | 'derived'
  | 'unobservable'
  | 'engine'
  | 'strategy'
  | 'recorder'
  | 'buy'
  | 'sell'
  | 'ok'
  | 'warn'
  | 'error'
  | 'neutral';

export interface BadgeProps {
  kind: BadgeKind;
  children: ReactNode;
  /** Tooltip explaining the badge. */
  title?: string;
}

export function Badge({ kind, children, title }: BadgeProps) {
  return (
    <span className={`badge badge-${kind}`} title={title}>
      {children}
    </span>
  );
}

/** What each evidence basis means. The single wording used by badges and the legend. */
export const BASIS_MEANING: Record<Basis, string> = {
  observed: 'Present in the replayed market data (recorded or synthetic, as the dataset declares).',
  modeled:
    'Output of the configured simulation model (queue position, latency, fills). An estimate, not a fact.',
  derived: 'Arithmetic over observed and modeled values.',
  unobservable: 'Not knowable from the available data.',
};

const BASIS_ORDER: Basis[] = ['observed', 'modeled', 'derived', 'unobservable'];

/** Badge for one of the four evidence bases, with its meaning as the tooltip. */
export function BasisBadge({ basis }: { basis: Basis }) {
  return (
    <Badge kind={basis} title={BASIS_MEANING[basis]}>
      {basis}
    </Badge>
  );
}

/** Legend explaining the four evidence bases. */
export function BasisLegend() {
  return (
    <ul className="basis-legend" aria-label="Evidence basis legend">
      {BASIS_ORDER.map((b) => (
        <li key={b}>
          <span>
            <Badge kind={b}>{b}</Badge>
          </span>
          <span>{BASIS_MEANING[b]}</span>
        </li>
      ))}
    </ul>
  );
}

export function SideBadge({ side }: { side: Side | null }) {
  if (side === null) return <Badge kind="neutral">n/a</Badge>;
  return <Badge kind={side}>{side}</Badge>;
}

/** Status pill: uppercase text plus a shape, so state never depends on colour (§6.6). */
export function Pill({ tone, children, title }: { tone: 'ok' | 'warn' | 'error' | 'neutral' | 'info'; children: ReactNode; title?: string }) {
  return (
    <span className={`pill pill-${tone}`} title={title}>
      {children}
    </span>
  );
}

// ----- Stat (KPI strip cell) --------------------------------------------------------------

export interface StatProps {
  label: ReactNode;
  value: ReactNode;
  hint?: ReactNode;
}

export function Stat({ label, value, hint }: StatProps) {
  return (
    <div className="stat">
      <span className="stat-label">{label}</span>
      <span className="stat-value">{value}</span>
      {hint !== undefined && hint !== null ? <span className="stat-hint">{hint}</span> : null}
    </div>
  );
}

// ----- Loading / ErrorBox / Empty / Notice ------------------------------------------------

/** Seconds since mount, updated once a second (for "Loading… 3 s"). */
export function useElapsedSeconds(): number {
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    const start = Date.now();
    const timer = window.setInterval(() => setElapsed(Math.floor((Date.now() - start) / 1000)), 1000);
    return () => window.clearInterval(timer);
  }, []);
  return elapsed;
}

/**
 * Static loading line with an elapsed-time counter; no animation (§4.5, §6.9). While the API is
 * waking it says so (§10.1).
 */
export function Spinner({ label = 'Loading' }: { label?: string }) {
  const elapsed = useElapsedSeconds();
  const waking = useApiPhase() === 'waking';
  return (
    <span className="spinner" role="status">
      {label}
      {'\u2026'}
      {elapsed > 0 ? <span className="mono">{` ${elapsed} s`}</span> : null}
      {waking ? ' \u2014 the API is waking up (free host; first response can take about a minute)' : null}
    </span>
  );
}

/**
 * Shows a failure exactly as reported. For API errors the server's code, message and request id
 * are printed verbatim, with the envelope's `details` (limits, quotas, problems) as facts
 * (API_CONTRACT §2). An `invalid_config` lists every problem, one per line.
 */
export function ErrorBox({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  const [copied, setCopied] = useState(false);
  if (error === null || error === undefined) return null;
  const view = describeError(error);
  const copy = () => {
    void navigator.clipboard?.writeText([view.head, view.message, ...view.facts].join('\n')).then(() => setCopied(true));
  };
  return (
    <div className="error-box" role="alert" data-error-state={view.state}>
      <div className="error-box-head">{view.head}</div>
      <div className="error-box-message">{view.message}</div>
      {view.facts.length > 0 ? (
        <ul className="error-box-facts mono">
          {view.facts.map((f) => (
            <li key={f}>{f}</li>
          ))}
        </ul>
      ) : null}
      {view.action ? <div className="error-box-action">{view.action}</div> : null}
      <div className="error-box-actions">
        {onRetry && view.state !== 'not_found' ? (
          <button type="button" onClick={onRetry}>
            Retry
          </button>
        ) : null}
        <button type="button" onClick={copy}>
          {copied ? 'Copied' : 'Copy details'}
        </button>
      </div>
    </div>
  );
}

export function Empty({ children }: { children?: ReactNode }) {
  return <div className="empty">{children}</div>;
}

export type NoticeTone = 'warn' | 'info' | 'modeled' | 'error';

/** Inline notice: a 2 px left rule, an uppercase tag and one sentence (§6.10). Not dismissible. */
export function Notice({ tag, tone = 'warn', children }: { tag: string; tone?: NoticeTone; children: ReactNode }) {
  return (
    <div className={tone === 'warn' ? 'notice' : `notice notice-${tone}`} role="note">
      <span className="notice-tag">{tag}</span>
      <span>{children}</span>
    </div>
  );
}

// ----- time and hash text -----------------------------------------------------------------

/**
 * Fraction digits of the dataset in view, from its declared timestamp resolution
 * (`digitsForUnit(manifest.timestamps.input_unit)`). Unknown resolution shows all nine digits
 * with the sub-microsecond group dimmed (§7.3, §16).
 */
export const TimeDigitsContext = createContext<NsDigits>(9);

/**
 * Dataset-relative time, `+mm:ss.mmm uuu`: the sub-millisecond groups are dimmed and the
 * exact nanosecond value is in the tooltip. `null` renders an em dash.
 */
export function ClockText({ ns, digits }: { ns: NsInput | null; digits?: NsDigits }) {
  const contextDigits = useContext(TimeDigitsContext);
  if (ns === null) return <span className="mono dim">{DASH}</span>;
  const d = digits ?? contextDigits;
  let parts;
  try {
    parts = relParts(ns, d);
  } catch {
    return <span className="mono dim">{fmtClock(ns, d)}</span>;
  }
  const [ms, ...sub] = parts.groups;
  return (
    <span className="mono nowrap" title={`${String(ns)} ns after dataset origin`}>
      {parts.sign}
      {parts.clock}
      {ms !== undefined ? `.${ms}` : null}
      {sub.length > 0 ? <span className="dim">{sub.map((g) => `\u2009${g}`).join('')}</span> : null}
    </span>
  );
}

/** Truncated hex digest; the full value is available on hover and to assistive technology. */
export function HashText({ hash, chars = 12 }: { hash: string; chars?: number }) {
  const truncated = hash.length > chars;
  return (
    <span className="mono" title={hash}>
      {shortHash(hash, chars)}
      {truncated ? <span className="dim" aria-hidden="true">{'\u2026'}</span> : null}
      {truncated ? <span className="visually-hidden">{hash.slice(chars)}</span> : null}
    </span>
  );
}

// ----- useAsync ---------------------------------------------------------------------------

export interface AsyncResult<T> {
  data: T | null;
  error: Error | null;
  loading: boolean;
  /** Runs `fn` again with the same dependencies, keeping the current data until it settles. */
  reload: () => void;
}

interface AsyncState<T> {
  data: T | null;
  error: Error | null;
  loading: boolean;
  /** The dependency values this state belongs to. */
  deps: unknown[];
}

function sameDeps(a: unknown[], b: unknown[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i += 1) {
    if (!Object.is(a[i], b[i])) return false;
  }
  return true;
}

/**
 * Runs `fn` whenever `deps` change and exposes its result. A call that has been superseded
 * (newer deps, a reload, or unmount) is aborted through `signal` and never writes its result,
 * and data loaded for old deps is never returned for new ones. A call that failed because the
 * API was asleep (cold start) is retried once when the health poll next reports the API up, so
 * a page opened during a cold start fills in without a click; nothing else is retried.
 */
export function useAsync<T>(fn: (signal: AbortSignal) => Promise<T>, deps: unknown[]): AsyncResult<T> {
  const [state, setState] = useState<AsyncState<T>>({
    data: null,
    error: null,
    loading: true,
    deps,
  });
  const [nonce, setNonce] = useState(0);
  const lastNonce = useRef(0);

  useEffect(() => {
    const ctl = new AbortController();
    const isReload = lastNonce.current !== nonce;
    lastNonce.current = nonce;
    setState((prev) => ({
      data: isReload && sameDeps(prev.deps, deps) ? prev.data : null,
      error: null,
      loading: true,
      deps,
    }));

    let pending: Promise<T>;
    try {
      pending = fn(ctl.signal);
    } catch (e) {
      pending = Promise.reject(e);
    }
    pending.then(
      (data) => {
        if (!ctl.signal.aborted) setState({ data, error: null, loading: false, deps });
      },
      (e: unknown) => {
        if (ctl.signal.aborted || isAbort(e)) return;
        const error = e instanceof Error ? e : new Error(String(e));
        setState({ data: null, error, loading: false, deps });
      },
    );
    return () => ctl.abort();
    // `fn` is intentionally not a dependency: callers pass an inline closure and list what it
    // reads in `deps`, exactly like `useEffect`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, nonce]);

  const reload = useCallback(() => setNonce((n) => n + 1), []);

  const phase = useApiPhase();
  const prevPhase = useRef(phase);
  const failedAsleep = state.error !== null && isWakingError(state.error);
  useEffect(() => {
    const was = prevPhase.current;
    prevPhase.current = phase;
    if (phase === 'ok' && was !== null && was !== 'ok' && failedAsleep) reload();
  }, [phase, failedAsleep, reload]);

  // Between a deps change and the effect that follows it, the stored state still belongs to the
  // previous deps; report "loading" rather than another key's data.
  if (!sameDeps(state.deps, deps)) {
    return { data: null, error: null, loading: true, reload };
  }
  return { data: state.data, error: state.error, loading: state.loading, reload };
}
