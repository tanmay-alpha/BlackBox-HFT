// Replay Terminal: one playback clock drives the dataset book heatmap with the run's own simulated
// orders on top, the depth ladder, the flight-recorder event log, the account state and the
// selected order's lifecycle. Data is fetched in windows of at most 60 s around the cursor and
// refetched when the cursor leaves the window; the whole run is never loaded.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { Link, useParams, useSearchParams } from 'react-router-dom';

import { api, reportUrl } from '../../api/client';
import type {
  DatasetManifest,
  FlightEvent,
  ReplayWindow,
  RunSummary,
  RunTimeline,
} from '../../api/types';
import { assumptionsLabel, OUTCOME_MEANING, strategyLabel } from '../../components/describe';
import { DASH, fmtDuration, fmtInt, fmtPrice, fmtQty, words } from '../../components/fmt';
import { PageHeader, usePageTitle } from '../../components/page';
import {
  Badge,
  BasisBadge,
  ClockText,
  Empty,
  ErrorBox,
  HashText,
  Panel,
  SideBadge,
  Spinner,
  Stat,
  useAsync,
} from '../../components/ui';
import { eventSummary, eventTitle } from '../forensics/eventFormat';
import { outcomeKind } from '../forensics/labels';
import { TwoClockTimeline } from '../forensics/TwoClockTimeline';
import { frameIndexAt, seriesIndexAt } from '../replay/heatmap';
import { fmtQuoteSigned } from '../runs/format';
import { EventLog, lastEventAtOrBefore } from './EventLog';
import { ordersRestingAt } from './overlay';
import { TerminalHeatmap } from './TerminalHeatmap';
import { TerminalLadder } from './TerminalLadder';
import './terminal.css';

const NS_PER_S = 1_000_000_000;
/** Longest span fetched per request. */
const WINDOW_NS = 60 * NS_PER_S;
/** Where the cursor sits inside a window fetched while moving forward. */
const LEAD_NS = 5 * NS_PER_S;
const LEVELS = 25;
const MAX_TRADES = 5000;
const MAX_EVENTS = 5000;
const LADDER_ROWS = 10;
const SPEEDS = [0.1, 0.25, 0.5, 1, 2, 5, 20];

interface Win {
  t0: number;
  t1: number;
}

interface Loaded {
  win: Win;
  tl: RunTimeline;
  rp: ReplayWindow;
}

function intParam(raw: string | null): number | null {
  if (raw === null || raw.trim() === '') return null;
  const n = Number(raw);
  return Number.isFinite(n) ? Math.round(n) : null;
}

/** A window of at most `WINDOW_NS` inside the dataset with the cursor `lead` ns after its start. */
function windowAt(t: number, lead: number, ds: DatasetManifest): Win {
  const t0 = Math.round(Math.max(ds.start, Math.min(t - lead, ds.end - WINDOW_NS)));
  return { t0, t1: Math.round(Math.min(ds.end, t0 + WINDOW_NS)) };
}

/** The first event after `t` (`dir` 1) or the last event before it (`dir` -1). */
function findStep(events: FlightEvent[], t: number, dir: 1 | -1): FlightEvent | null {
  let i = lastEventAtOrBefore(events, t);
  if (dir === 1) return events[i + 1] ?? null;
  while (i >= 0 && events[i].ts >= t) i -= 1;
  return i >= 0 ? events[i] : null;
}

export default function ReplayTerminalPage() {
  const { runId = '' } = useParams();
  usePageTitle(runId ? `Replay terminal ${runId}` : 'Replay terminal');
  const run = useAsync(() => api.run(runId), [runId]);
  const dsId = run.data?.manifest.dataset.id ?? null;
  const dataset = useAsync(
    () => (dsId === null ? Promise.resolve(null) : api.dataset(dsId)),
    [dsId],
  );

  return (
    <div className="col">
      <PageHeader
        title={
          <>
            Replay terminal{' '}
            <span className="mono dim">{run.data?.label || runId}</span>
          </>
        }
        lead="The historical book as observed, with this run's simulated orders and fills replayed on top. Simulated orders never entered the historical book and did not move prices."
        actions={
          <>
            <Link className="btn" to={`/runs/${encodeURIComponent(runId)}`}>
              Run overview
            </Link>
            <a className="btn" href={reportUrl(runId)} download>
              Download report (.md)
            </a>
          </>
        }
      />
      {run.error ? <ErrorBox error={run.error} /> : null}
      {dataset.error ? <ErrorBox error={dataset.error} /> : null}
      {run.loading || (dsId !== null && dataset.loading) ? <Spinner label="Loading run and dataset" /> : null}
      {run.data && dataset.data ? (
        <Terminal key={runId} run={run.data} ds={dataset.data} />
      ) : null}
    </div>
  );
}

function ProvenanceHeader({ run, ds }: { run: RunSummary; ds: DatasetManifest }) {
  const m = run.manifest;
  const c = m.config;
  return (
    <section className="term-prov" aria-label="Run assumptions and dataset provenance">
      <div className="term-prov-group">
        <span className="term-prov-label">Run</span>
        <span className="mono">{m.run_id}</span>
        <Badge kind="modeled">simulated</Badge>
        <span className="mono dim">
          {strategyLabel(c.strategy.kind)} · every {fmtDuration(c.strategy.interval_ns)} · seed {c.seed}
        </span>
      </div>
      <div className="term-prov-group">
        <span className="term-prov-label">Assumptions</span>
        <BasisBadge basis="modeled" />
        <span className="mono small">{assumptionsLabel(c)}</span>
        <span className="mono dim small">
          {m.engine.name} {m.engine.version}
        </span>
      </div>
      <div className="term-prov-group">
        <span className="term-prov-label">Dataset</span>
        {ds.kind === 'synthetic' ? (
          <Badge kind="warn" title="Generated by BlackBox from a seed. This is not market data.">
            synthetic
          </Badge>
        ) : (
          <Badge kind="observed">recorded</Badge>
        )}
        <span>{ds.title}</span>
        <span className="mono dim small">
          {ds.source.vendor} · {ds.source.exchange} · {ds.source.symbol} · local timestamps{' '}
          {ds.timestamps.local_ts} · sha256 <HashText hash={m.dataset.sha256} chars={10} />
        </span>
        {ds.events_sha256 !== m.dataset.sha256 ? (
          <Badge kind="warn" title="The dataset on the server has a different hash from the one this run used.">
            dataset hash changed since this run
          </Badge>
        ) : null}
      </div>
    </section>
  );
}

function Terminal({ run, ds }: { run: RunSummary; ds: DatasetManifest }) {
  const runId = run.manifest.run_id;
  const inst = ds.instrument;
  const [searchParams, setSearchParams] = useSearchParams();
  const initialT = useMemo(() => intParam(searchParams.get('t')), []); // eslint-disable-line react-hooks/exhaustive-deps
  const initialOrder = useMemo(() => intParam(searchParams.get('order')), []); // eslint-disable-line react-hooks/exhaustive-deps

  const clampT = useCallback((t: number) => Math.min(ds.end, Math.max(ds.start, t)), [ds]);
  const [cursor, setCursorRaw] = useState<number | null>(initialT === null ? null : clampT(initialT));
  const setCursor = useCallback((t: number) => setCursorRaw(clampT(t)), [clampT]);
  const [selectedOrder, setSelectedOrder] = useState<number | null>(initialOrder);
  const [win, setWin] = useState<Win | null>(initialT === null ? null : windowAt(clampT(initialT), LEAD_NS, ds));
  const [wantDefault, setWantDefault] = useState(initialT === null && initialOrder === null);
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<unknown>(null);
  const [reloadNonce, setReloadNonce] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [speed, setSpeed] = useState(1);
  const [typeFilter, setTypeFilter] = useState('');
  const [orderFilter, setOrderFilter] = useState('');
  const [search, setSearch] = useState('');
  const [pendingStep, setPendingStep] = useState<1 | -1 | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [jumpText, setJumpText] = useState('');
  const [jumpOrderText, setJumpOrderText] = useState(initialOrder === null ? '' : String(initialOrder));
  const [jumpError, setJumpError] = useState<unknown>(null);
  const jumpRef = useRef<HTMLInputElement | null>(null);
  const [focus, setFocus] = useState<{ t: number; key: number } | null>(null);

  // ----- window loading ------------------------------------------------------------------
  // No time or order requested: start at the run start, learned from a count-only timeline call.
  useEffect(() => {
    if (!wantDefault || win !== null) return;
    let stale = false;
    setLoading(true);
    setLoadError(null);
    api.timeline(runId, { max_events: 0 }).then(
      (tl) => {
        if (stale) return;
        const t = clampT(tl.run_start);
        setCursorRaw(t);
        setWin(windowAt(t, 0, ds));
      },
      (e: unknown) => {
        if (stale) return;
        setLoadError(e);
        setLoading(false);
      },
    );
    return () => {
      stale = true;
    };
  }, [runId, ds, wantDefault, win, clampT, reloadNonce]);

  useEffect(() => {
    if (win === null) return;
    let stale = false;
    setLoading(true);
    setLoadError(null);
    Promise.all([
      api.timeline(runId, { t0: win.t0, t1: win.t1, max_events: MAX_EVENTS }),
      api.replay(ds.id, { t0: win.t0, t1: win.t1, levels: LEVELS, max_trades: MAX_TRADES }),
    ]).then(
      ([tl, rp]) => {
        if (stale) return;
        setLoaded({ win, tl, rp });
        setLoading(false);
      },
      (e: unknown) => {
        if (stale) return;
        setLoadError(e);
        setLoading(false);
      },
    );
    return () => {
      stale = true;
    };
    // `win` is listed by value so an identical window is not fetched twice.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [runId, ds.id, win?.t0, win?.t1, reloadNonce]);

  // Refetch when the cursor leaves the requested window.
  useEffect(() => {
    if (cursor === null || win === null) return;
    if (cursor >= win.t0 && cursor <= win.t1) return;
    const next = windowAt(cursor, cursor > win.t1 ? LEAD_NS : WINDOW_NS - LEAD_NS, ds);
    setWin((prev) => (prev && prev.t0 === next.t0 && prev.t1 === next.t1 ? prev : next));
  }, [cursor, win, ds]);

  // ----- jump to order -------------------------------------------------------------------
  const jumpToOrder = useCallback(
    async (orderId: number) => {
      setJumpError(null);
      try {
        const detail = await api.orderDetail(runId, orderId);
        const t = clampT(detail.order.decision_ts);
        setPlaying(false);
        setSelectedOrder(orderId);
        setJumpOrderText(String(orderId));
        setCursorRaw(t);
        const next = windowAt(t, LEAD_NS, ds);
        setWin((prev) => (prev && prev.t0 === next.t0 && prev.t1 === next.t1 ? prev : next));
        setFocus((f) => ({ t, key: (f?.key ?? 0) + 1 }));
        return true;
      } catch (e) {
        setJumpError(e);
        return false;
      }
    },
    [runId, ds, clampT],
  );

  useEffect(() => {
    if (initialT === null && initialOrder !== null) {
      void jumpToOrder(initialOrder).then((ok) => {
        if (!ok) setWantDefault(true);
      });
    }
    // Mount only: later URL changes are handled below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ----- URL sync ------------------------------------------------------------------------
  const written = useRef<{ t: number | null; order: number | null }>({ t: initialT, order: initialOrder });
  useEffect(() => {
    if (playing || cursor === null) return;
    const id = window.setTimeout(() => {
      const t = Math.round(cursor);
      written.current = { t, order: selectedOrder };
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set('t', String(t));
          if (selectedOrder === null) next.delete('order');
          else next.set('order', String(selectedOrder));
          return next;
        },
        { replace: true },
      );
    }, 300);
    return () => window.clearTimeout(id);
  }, [cursor, selectedOrder, playing, setSearchParams]);

  // Links into this page from elsewhere (same run, new t/order) move the terminal.
  useEffect(() => {
    const t = intParam(searchParams.get('t'));
    const order = intParam(searchParams.get('order'));
    const w = written.current;
    if (order !== w.order && order !== null && t === null) {
      written.current = { ...w, order };
      void jumpToOrder(order);
      return;
    }
    if (t !== null && t !== w.t) {
      written.current = { t, order };
      setPlaying(false);
      setCursor(t);
      if (order !== w.order) setSelectedOrder(order);
    }
  }, [searchParams, jumpToOrder, setCursor]);

  // ----- playback ------------------------------------------------------------------------
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    let last = performance.now();
    const tick = (now: number) => {
      const elapsedMs = Math.min(100, now - last);
      last = now;
      setCursorRaw((c) => (c === null ? c : Math.min(ds.end, c + elapsedMs * 1e6 * speed)));
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, speed, ds.end]);

  useEffect(() => {
    if (playing && cursor !== null && cursor >= ds.end) setPlaying(false);
  }, [playing, cursor, ds.end]);

  // ----- events --------------------------------------------------------------------------
  const tl = loaded?.tl ?? null;
  const rp = loaded?.rp ?? null;
  const sortedEvents = useMemo(
    () => (tl ? [...tl.events].sort((a, b) => a.ts - b.ts || a.seq - b.seq) : []),
    [tl],
  );
  const eventTypes = useMemo(
    () => [...new Set(sortedEvents.map((e) => e.body.type))].sort(),
    [sortedEvents],
  );
  const orderFilterId = /^\d+$/.test(orderFilter.trim()) ? Number(orderFilter.trim()) : null;
  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return sortedEvents.filter(
      (e) =>
        (typeFilter === '' || e.body.type === typeFilter) &&
        (orderFilterId === null || e.order_id === orderFilterId) &&
        (q === '' ||
          e.body.type.includes(q) ||
          eventTitle(e).toLowerCase().includes(q) ||
          eventSummary(e, inst).toLowerCase().includes(q)),
    );
  }, [sortedEvents, typeFilter, orderFilterId, search, inst]);

  const hopBeyond = useCallback(
    (dir: 1 | -1, l: Loaded) => {
      const edge = dir === 1 ? l.win.t1 : l.win.t0;
      if ((dir === 1 && edge >= ds.end) || (dir === -1 && edge <= ds.start)) {
        setPendingStep(null);
        setNotice(`No ${dir === 1 ? 'later' : 'earlier'} event matching the filters in this dataset span.`);
        return;
      }
      setPendingStep(dir);
      setNotice(`Searching the ${dir === 1 ? 'next' : 'previous'} window for a matching event${'\u2026'}`);
      setCursor(dir === 1 ? edge + 1 : edge - 1);
    },
    [ds, setCursor],
  );

  const step = useCallback(
    (dir: 1 | -1) => {
      setPlaying(false);
      setNotice(null);
      if (!loaded || cursor === null) return;
      const ev = findStep(filtered, cursor, dir);
      if (ev) {
        setCursor(ev.ts);
        return;
      }
      hopBeyond(dir, loaded);
    },
    [loaded, cursor, filtered, hopBeyond, setCursor],
  );

  // A step that left the window continues once the next window has arrived.
  useEffect(() => {
    if (pendingStep === null || !loaded || cursor === null) return;
    if (cursor < loaded.win.t0 || cursor > loaded.win.t1) return;
    const ev = findStep(filtered, cursor, pendingStep);
    if (ev) {
      setPendingStep(null);
      setNotice(null);
      setCursor(ev.ts);
      return;
    }
    hopBeyond(pendingStep, loaded);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loaded, pendingStep]);

  const togglePlay = useCallback(() => {
    setNotice(null);
    setPendingStep(null);
    setPlaying((p) => !p);
  }, []);

  // ----- keyboard ------------------------------------------------------------------------
  const keys = useRef({ step, togglePlay });
  keys.current = { step, togglePlay };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const target = e.target instanceof HTMLElement ? e.target : null;
      if (target?.closest('input, select, textarea, [contenteditable="true"]')) return;
      if (e.key === ' ') {
        if (target?.closest('button, a')) return;
        e.preventDefault();
        keys.current.togglePlay();
      } else if (e.key === 'ArrowRight') {
        e.preventDefault();
        keys.current.step(1);
      } else if (e.key === 'ArrowLeft') {
        e.preventDefault();
        keys.current.step(-1);
      } else if (e.key === 'j' || e.key === 'J') {
        e.preventDefault();
        jumpRef.current?.focus();
        jumpRef.current?.select();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const selectOrder = useCallback((orderId: number) => {
    setSelectedOrder(orderId);
    setJumpOrderText(String(orderId));
  }, []);

  const submitJump = (e: FormEvent) => {
    e.preventDefault();
    const s = Number(jumpText.trim());
    if (jumpText.trim() === '' || !Number.isFinite(s)) return;
    setPlaying(false);
    setPendingStep(null);
    setNotice(null);
    setCursor(Math.round(s * NS_PER_S));
  };

  const submitJumpOrder = (e: FormEvent) => {
    e.preventDefault();
    const text = jumpOrderText.trim();
    if (!/^\d+$/.test(text)) return;
    void jumpToOrder(Number(text));
  };

  // ----- cursor-derived state ------------------------------------------------------------
  const cur = cursor ?? ds.start;
  const frame = rp ? (() => {
    const i = frameIndexAt(rp.depth, cur);
    return i >= 0 ? rp.depth[i] : null;
  })() : null;
  const bboIdx = rp ? seriesIndexAt(rp.bbo.ts, cur) : -1;
  const resting = useMemo(() => (tl ? ordersRestingAt(tl.orders, cur) : []), [tl, cur]);
  const equityTs = useMemo(() => (tl ? tl.equity.map((s) => s.ts) : []), [tl]);
  const sampleIdx = seriesIndexAt(equityTs, cur);
  const sample = tl && sampleIdx >= 0 ? tl.equity[sampleIdx] : null;
  const jumpValid = jumpText.trim() !== '' && Number.isFinite(Number(jumpText.trim()));
  const cursorInWindow = loaded !== null && cur >= loaded.win.t0 && cur <= loaded.win.t1;

  return (
    <div className="col">
      <ProvenanceHeader run={run} ds={ds} />

      <section className="term-controls" aria-label="Playback controls">
        <div className="row term-transport">
          <button type="button" className="btn-primary" aria-pressed={playing} onClick={togglePlay} disabled={!loaded}>
            {playing ? 'Pause' : 'Play'}
          </button>
          <button type="button" onClick={() => step(-1)} disabled={!loaded}>
            {'\u2190'} Previous event
          </button>
          <button type="button" onClick={() => step(1)} disabled={!loaded}>
            Next event {'\u2192'}
          </button>
          <label htmlFor="term-speed">Speed</label>
          <select id="term-speed" value={speed} onChange={(e) => setSpeed(Number(e.target.value))}>
            {SPEEDS.map((s) => (
              <option key={s} value={s}>
                {s}
                {'\u00d7'} recorded time
              </option>
            ))}
          </select>
          <span className="term-cursor mono">
            cursor <ClockText ns={cursor === null ? null : Math.round(cursor)} />{' '}
            <span className="dim">exchange clock</span>
          </span>
          {loading ? <Spinner label="Loading window" /> : null}
        </div>
        <label htmlFor="term-scrub" className="visually-hidden">
          Cursor position in the dataset
        </label>
        <input
          id="term-scrub"
          className="term-scrub"
          type="range"
          min={ds.start}
          max={ds.end}
          step={1_000_000}
          value={cur}
          onChange={(e) => {
            setPlaying(false);
            setCursor(Number(e.target.value));
          }}
        />
        <div className="row term-jumps">
          <form className="row" onSubmit={submitJump}>
            <label htmlFor="term-jump">
              Jump to time <span className="dim">(s after origin, <kbd>J</kbd>)</span>
            </label>
            <input
              id="term-jump"
              ref={jumpRef}
              type="text"
              inputMode="decimal"
              placeholder={((cursor ?? ds.start) / NS_PER_S).toFixed(3)}
              value={jumpText}
              onChange={(e) => setJumpText(e.target.value)}
              size={12}
            />
            <button type="submit" disabled={!jumpValid}>
              Go
            </button>
          </form>
          <form className="row" onSubmit={submitJumpOrder}>
            <label htmlFor="term-jump-order">Jump to order</label>
            <input
              id="term-jump-order"
              type="text"
              inputMode="numeric"
              placeholder="order id"
              value={jumpOrderText}
              onChange={(e) => setJumpOrderText(e.target.value)}
              size={10}
            />
            <button type="submit" disabled={!/^\d+$/.test(jumpOrderText.trim())}>
              Go
            </button>
          </form>
          <span className="dim small">
            Keys: <kbd>Space</kbd> play/pause, <kbd>{'\u2190'}</kbd> <kbd>{'\u2192'}</kbd> previous/next
            event (respects the event filters), <kbd>J</kbd> jump to time.
          </span>
        </div>
        {notice ? <p className="note" role="status">{notice}</p> : null}
        {jumpError ? <ErrorBox error={jumpError} /> : null}
      </section>

      {loadError ? (
        <div className="col">
          <ErrorBox error={loadError} />
          <div className="row">
            <button type="button" onClick={() => setReloadNonce((n) => n + 1)}>
              Retry window
            </button>
          </div>
        </div>
      ) : null}
      {!loaded && !loadError ? <Spinner label="Loading the first window" /> : null}

      {loaded && tl && rp ? (
        <>
          <p className="mono dim small">
            window <ClockText ns={tl.t0} /> {'\u2192'} <ClockText ns={tl.t1} /> ·{' '}
            {fmtInt(tl.events_total)} tape events · {fmtInt(tl.fills.length)} simulated fills ·{' '}
            {fmtInt(tl.orders.length)} orders alive · {fmtInt(rp.depth.length)} depth frames ·{' '}
            {fmtInt(rp.trades.ts.length)} market trades · run span <ClockText ns={tl.run_start} />{' '}
            {'\u2192'} <ClockText ns={tl.run_end} />
            {cursorInWindow ? '' : ' · cursor outside the loaded window, fetching'}
          </p>
          {tl.events_truncated ? (
            <p className="notice" role="note">
              <Badge kind="warn">events truncated</Badge> This window holds {fmtInt(tl.events_total)}{' '}
              tape events; only the earliest {fmtInt(tl.events.length)} were returned. Later events in
              this window are missing from the log and from event stepping.
            </p>
          ) : null}
          {rp.trades_truncated ? (
            <p className="notice" role="note">
              <Badge kind="warn">trades truncated</Badge> More than {fmtInt(MAX_TRADES)} market trades
              in this window; only the {fmtInt(rp.trades.ts.length)} largest are drawn.
            </p>
          ) : null}
          {rp.bbo_downsampled ? (
            <p className="notice" role="note">
              <Badge kind="warn">downsampled</Badge> Best bid/ask lines are downsampled to one entry
              per {fmtDuration(rp.step_ns)}.
            </p>
          ) : null}

          <div className="term-main">
            <Panel
              title={
                <>
                  Book heatmap <Badge kind="observed">observed L2</Badge> with own orders{' '}
                  <Badge kind="modeled">modeled</Badge>
                </>
              }
            >
              <TerminalHeatmap
                win={rp}
                instrument={inst}
                orders={tl.orders}
                fills={tl.fills}
                selectedOrder={selectedOrder}
                cursor={cur}
                follow={playing}
                focus={loaded.win === win ? focus : null}
                onCursor={(t) => {
                  setPlaying(false);
                  setCursor(t);
                }}
              />
            </Panel>
            <div className="col">
              <Panel title="Depth at cursor">
                <div className="col">
                  <dl className="kv">
                    <dt>Best bid</dt>
                    <dd className="buy">
                      {bboIdx >= 0 && rp.bbo.bid_tick[bboIdx] !== null
                        ? `${fmtPrice(rp.bbo.bid_tick[bboIdx], inst)} \u00d7 ${fmtQty(rp.bbo.bid_qty[bboIdx], inst)}`
                        : DASH}
                    </dd>
                    <dt>Best ask</dt>
                    <dd className="sell">
                      {bboIdx >= 0 && rp.bbo.ask_tick[bboIdx] !== null
                        ? `${fmtPrice(rp.bbo.ask_tick[bboIdx], inst)} \u00d7 ${fmtQty(rp.bbo.ask_qty[bboIdx], inst)}`
                        : DASH}
                    </dd>
                  </dl>
                  <TerminalLadder
                    frame={frame}
                    instrument={inst}
                    rows={LADDER_ROWS}
                    resting={resting}
                    selectedOrder={selectedOrder}
                    onSelectOrder={selectOrder}
                  />
                </div>
              </Panel>
              <Panel title={<>Account at cursor <BasisBadge basis="derived" /></>}>
                {sample ? (
                  <div className="col">
                    <div className="stats">
                      <Stat label="Position" value={fmtQty(sample.position, inst)} hint="From simulated fills" />
                      <Stat label="Known to strategy" value={fmtQty(sample.known_position, inst)} />
                      <Stat label="Open orders" value={fmtInt(sample.open_orders)} />
                      <Stat label="Realized" value={fmtQuoteSigned(sample.realized_pnl)} />
                      <Stat label="Unrealized" value={fmtQuoteSigned(sample.unrealized_pnl)} />
                      <Stat label="Fees" value={fmtQuoteSigned(sample.fees)} />
                      <Stat label="Equity" value={fmtQuoteSigned(sample.equity)} />
                    </div>
                    <p className="note">
                      Last account sample at or before the cursor: <ClockText ns={sample.ts} /> on the
                      local clock (strategy tick {fmtInt(sample.tick_no)}). Samples are periodic, so
                      changes between samples are not shown here.
                    </p>
                  </div>
                ) : (
                  <Empty>No account sample at or before the cursor in this window.</Empty>
                )}
              </Panel>
            </div>
          </div>

          <div className="grid-2">
            <Panel
              title="Event timeline"
              actions={
                <>
                  <label htmlFor="term-ev-type" className="visually-hidden">
                    Event type
                  </label>
                  <select id="term-ev-type" value={typeFilter} onChange={(e) => setTypeFilter(e.target.value)}>
                    <option value="">All types</option>
                    {eventTypes.map((t) => (
                      <option key={t} value={t}>
                        {words(t)}
                      </option>
                    ))}
                  </select>
                  <label htmlFor="term-ev-order" className="visually-hidden">
                    Order id
                  </label>
                  <input
                    id="term-ev-order"
                    type="text"
                    inputMode="numeric"
                    placeholder="order id"
                    value={orderFilter}
                    onChange={(e) => setOrderFilter(e.target.value)}
                    size={9}
                  />
                  <label htmlFor="term-ev-search" className="visually-hidden">
                    Search events
                  </label>
                  <input
                    id="term-ev-search"
                    type="search"
                    placeholder="search"
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    size={12}
                  />
                </>
              }
            >
              <EventLog
                events={filtered}
                cursor={cur}
                instrument={inst}
                selectedOrder={selectedOrder}
                onSelectOrder={selectOrder}
                onJump={(ev) => {
                  setPlaying(false);
                  setCursor(ev.ts);
                  if (ev.order_id !== null) selectOrder(ev.order_id);
                }}
              />
            </Panel>
            <div className="col">
              <SelectedOrder
                runId={runId}
                orderId={selectedOrder}
                onCursor={(t) => {
                  setPlaying(false);
                  setCursor(t);
                }}
              />
              <OrdersInWindow
                tl={tl}
                cursor={cur}
                selectedOrder={selectedOrder}
                inst={inst}
                onSelect={(id, t) => {
                  selectOrder(id);
                  setPlaying(false);
                  setCursor(t);
                }}
              />
            </div>
          </div>
        </>
      ) : null}
    </div>
  );
}

function SelectedOrder({
  runId,
  orderId,
  onCursor,
}: {
  runId: string;
  orderId: number | null;
  onCursor: (t: number) => void;
}) {
  const detail = useAsync(
    () => (orderId === null ? Promise.resolve(null) : api.orderDetail(runId, orderId)),
    [runId, orderId],
  );
  const [selectedSeq, setSelectedSeq] = useState<number | null>(null);
  const cited = useMemo(() => new Set<number>(), []);
  const d = detail.data;
  return (
    <Panel
      title={
        <>
          Selected order {orderId === null ? null : <span className="mono">#{orderId}</span>}{' '}
          <BasisBadge basis="modeled" />
        </>
      }
      actions={
        orderId === null ? null : (
          <Link className="btn" to={`/runs/${encodeURIComponent(runId)}/orders/${orderId}`}>
            Full forensics
          </Link>
        )
      }
    >
      {orderId === null ? (
        <Empty>Select an order in the ladder, the event log or the order list, or jump to one by id.</Empty>
      ) : null}
      {detail.loading && orderId !== null ? <Spinner label="Loading order lifecycle" /> : null}
      {detail.error ? <ErrorBox error={detail.error} /> : null}
      {d ? (
        <div className="col">
          <div className="row">
            <SideBadge side={d.order.side} />
            <span className="mono">
              {fmtQty(d.order.qty, d.instrument)} @ {fmtPrice(d.order.price_tick, d.instrument)} ·{' '}
              {d.order.tif} · {d.order.ord_type}
            </span>
            <Badge kind={outcomeKind(d.order.outcome)} title={OUTCOME_MEANING[d.order.outcome]}>
              {words(d.order.outcome)}
            </Badge>
          </div>
          <p>{d.explanation.headline}</p>
          <div className="row term-order-times small">
            <button type="button" className="btn-link" onClick={() => onCursor(d.order.decision_ts)}>
              decision <ClockText ns={d.order.decision_ts} /> (local)
            </button>
            {d.order.exch_arrival_ts !== null ? (
              <button type="button" className="btn-link" onClick={() => onCursor(d.order.exch_arrival_ts as number)}>
                arrival <ClockText ns={d.order.exch_arrival_ts} /> (exch)
              </button>
            ) : null}
            {d.order.first_fill_ts !== null ? (
              <button type="button" className="btn-link" onClick={() => onCursor(d.order.first_fill_ts as number)}>
                first fill <ClockText ns={d.order.first_fill_ts} /> (exch)
              </button>
            ) : null}
            {d.order.terminal_ts !== null ? (
              <button type="button" className="btn-link" onClick={() => onCursor(d.order.terminal_ts as number)}>
                terminal <ClockText ns={d.order.terminal_ts} /> (exch)
              </button>
            ) : null}
          </div>
          {d.events.length > 0 ? (
            <TwoClockTimeline
              events={d.events}
              originNs={d.order.decision_ts}
              instrument={d.instrument}
              side={d.order.side}
              mode="linear"
              selectedSeq={selectedSeq}
              citedSeqs={cited}
              onSelect={(seq) => {
                setSelectedSeq(seq);
                const ev = d.events.find((e) => e.seq === seq);
                if (ev) onCursor(ev.ts);
              }}
            />
          ) : (
            <Empty>This order has no tape events.</Empty>
          )}
        </div>
      ) : null}
    </Panel>
  );
}

function OrdersInWindow({
  tl,
  cursor,
  selectedOrder,
  inst,
  onSelect,
}: {
  tl: RunTimeline;
  cursor: number;
  selectedOrder: number | null;
  inst: DatasetManifest['instrument'];
  onSelect: (orderId: number, t: number) => void;
}) {
  return (
    <Panel title={<>Orders alive in this window <BasisBadge basis="modeled" /></>}>
      {tl.orders.length === 0 ? (
        <Empty>No orders of this run were alive in this window.</Empty>
      ) : (
        <div className="table-scroll term-orders">
          <table className="table">
            <thead>
              <tr>
                <th scope="col">Order</th>
                <th scope="col">Side</th>
                <th scope="col" className="num">
                  Price
                </th>
                <th scope="col" className="num">
                  Qty
                </th>
                <th scope="col">Outcome</th>
                <th scope="col">Decision (local)</th>
                <th scope="col">Arrival (exch)</th>
              </tr>
            </thead>
            <tbody>
              {tl.orders.map((o) => {
                const live =
                  o.exch_arrival_ts !== null &&
                  o.exch_arrival_ts <= cursor &&
                  (o.terminal_ts === null || o.terminal_ts > cursor);
                return (
                  <tr
                    key={o.order_id}
                    className={o.order_id === selectedOrder ? 'selected' : live ? 'term-live' : undefined}
                  >
                    <th scope="row">
                      <button type="button" className="btn-link mono" onClick={() => onSelect(o.order_id, o.decision_ts)}>
                        {o.order_id}
                      </button>
                    </th>
                    <td>
                      <SideBadge side={o.side} />
                    </td>
                    <td className="num">{fmtPrice(o.price_tick, inst)}</td>
                    <td className="num">{fmtQty(o.qty, inst)}</td>
                    <td>
                      <Badge kind={outcomeKind(o.outcome)} title={OUTCOME_MEANING[o.outcome]}>
                        {words(o.outcome)}
                      </Badge>
                    </td>
                    <td>
                      <ClockText ns={o.decision_ts} />
                    </td>
                    <td>
                      <ClockText ns={o.exch_arrival_ts} />
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </Panel>
  );
}
