// Shared presentational primitives and the data-loading hook used by every page.

import { useCallback, useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';

import { ApiError } from '../api/client';
import type { Basis, Side } from '../api/types';
import { DASH, fmtClock, shortHash } from './fmt';

// ----- Panel ------------------------------------------------------------------------------

export interface PanelProps {
  title: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
  className?: string;
}

export function Panel({ title, actions, children, className }: PanelProps) {
  return (
    <section className={className ? `panel ${className}` : 'panel'}>
      <header className="panel-head">
        <h2 className="panel-title">{title}</h2>
        {actions ? <div className="panel-actions">{actions}</div> : null}
      </header>
      <div className="panel-body">{children}</div>
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
  observed: 'Present in the historical market data.',
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
          <Badge kind={b}>{b}</Badge>
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

// ----- Stat -------------------------------------------------------------------------------

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

// ----- Spinner / ErrorBox / Empty ---------------------------------------------------------

export function Spinner({ label = 'Loading' }: { label?: string }) {
  return (
    <span className="spinner" role="status" aria-live="polite">
      <span className="spinner-mark" aria-hidden="true" />
      <span>{label}</span>
    </span>
  );
}

/**
 * Shows a failure exactly as reported. For API errors the server's code and message are
 * printed verbatim (an `invalid_config` message lists every problem, one per line).
 */
export function ErrorBox({ error }: { error: unknown }) {
  if (error === null || error === undefined) return null;
  const message = error instanceof Error ? error.message : String(error);
  const api = error instanceof ApiError ? error : null;
  return (
    <div className="error-box" role="alert">
      <div className="error-box-head">
        <Badge kind="error">error</Badge>
        {api ? (
          <span className="mono">
            {api.code}
            {api.status > 0 ? ` (HTTP ${api.status})` : ''}
          </span>
        ) : (
          <span>Unexpected failure</span>
        )}
      </div>
      <div className="error-box-message">{message}</div>
    </div>
  );
}

export function Empty({ children }: { children?: ReactNode }) {
  return <div className="empty">{children}</div>;
}

// ----- small text helpers -----------------------------------------------------------------

/** `mm:ss.mmm` with the microsecond digits dimmed. `null` renders an em dash. */
export function ClockText({ ns }: { ns: number | null }) {
  if (ns === null) return <span className="mono dim">{DASH}</span>;
  const text = fmtClock(ns);
  return (
    <span className="mono nowrap" title={`${ns} ns after dataset origin`}>
      {text.slice(0, -3)}
      <span className="dim">{text.slice(-3)}</span>
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
 * (newer deps, a reload, or unmount) never writes its result, and data loaded for old deps is
 * never returned for new ones.
 */
export function useAsync<T>(fn: () => Promise<T>, deps: unknown[]): AsyncResult<T> {
  const [state, setState] = useState<AsyncState<T>>({
    data: null,
    error: null,
    loading: true,
    deps,
  });
  const [nonce, setNonce] = useState(0);
  const lastNonce = useRef(0);

  useEffect(() => {
    let superseded = false;
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
      pending = fn();
    } catch (e) {
      pending = Promise.reject(e);
    }
    pending.then(
      (data) => {
        if (!superseded) setState({ data, error: null, loading: false, deps });
      },
      (e: unknown) => {
        if (superseded) return;
        const error = e instanceof Error ? e : new Error(String(e));
        setState({ data: null, error, loading: false, deps });
      },
    );
    return () => {
      superseded = true;
    };
    // `fn` is intentionally not a dependency: callers pass an inline closure and list what it
    // reads in `deps`, exactly like `useEffect`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, nonce]);

  const reload = useCallback(() => setNonce((n) => n + 1), []);

  // Between a deps change and the effect that follows it, the stored state still belongs to the
  // previous deps; report "loading" rather than another key's data.
  if (!sameDeps(state.deps, deps)) {
    return { data: null, error: null, loading: true, reload };
  }
  return { data: state.data, error: state.error, loading: state.loading, reload };
}
