// Order Book Lab, with run (UI_DESIGN_SPEC.md §11.3): one playback clock drives the dataset book
// heatmap with the run's own simulated orders on top, the depth ladder, the account state, the
// flight-recorder event log and the selected order's lifecycle. Data is fetched in windows of at
// most 10 s around the cursor (every timeline page of the window, see `timelinePaging.ts`) and
// refetched when the cursor leaves the window; the whole run is never loaded.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { FormEvent, ReactNode } from 'react';
import { Link, useParams, useSearchParams } from 'react-router-dom';

import { api, saveText } from '../../api/client';
import type { RunDetail } from '../../api/model';
import type { DatasetManifest, FlightEvent, OrderDetail, OrderRecord, ReplayWindow } from '../../api/types';
import type { RunTimeline } from '../../api/model';
import { DataTable } from '../../components/DataTable';
import type { Column } from '../../components/DataTable';
import { assumptionsLabel, OUTCOME_MEANING, strategyLabel, tifLabel } from '../../components/describe';
import { DASH, fmtDuration, fmtInt, fmtMoney, fmtPnl, fmtPrice, fmtQty, words } from '../../components/fmt';
import { digitsForUnit } from '../../components/ns';
import { PaneTabs, usePhone } from '../../components/PaneTabs';
import { SplitPane } from '../../components/SplitPane';
import { Tabs } from '../../components/Tabs';
import { Toolbar, ToolbarDivider } from '../../components/Toolbar';
import {
  Badge,
  BasisBadge,
  ClockText,
  Empty,
  ErrorBox,
  HashText,
  Notice,
  Panel,
  SideBadge,
  Spinner,
  Stat,
  TimeDigitsContext,
  useAsync,
} from '../../components/ui';
import type { AsyncResult } from '../../components/ui';
import { useInspector } from '../../shell/inspector';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import { eventSummary, eventTitle } from '../forensics/eventFormat';
import { outcomeKind } from '../forensics/labels';
import { TwoClockTimeline } from '../forensics/TwoClockTimeline';
import { BookModeRun } from '../replay/BookMode';
import { frameIndexAt, seriesIndexAt } from '../replay/heatmap';
import { parseSeconds, secondsText } from '../replay/seconds';
import { RunContextLinks } from '../runs/RunContextLinks';
import { EventLog } from './EventLog';
import { ordersRestingAt } from './overlay';
import { TerminalHeatmap } from './TerminalHeatmap';
import { TerminalLadder } from './TerminalLadder';
import { fetchTimelineWindow, findStep } from './timelinePaging';
import type { TimelineProgress } from './timelinePaging';
import '../replay/replay.css';
import './terminal.css';

const NS_PER_S = 1_000_000_000;
/**
 * Longest span fetched per request. 10 s keeps a window's fills, equity and orders (sent once,
 * on its first timeline page) far below the anonymous 1 MiB response cap: on the 120 s demo
 * run a 10 s window carries ~2.2k tape events and ~0.2 MB of context (~0.8 MB in total, so
 * usually one page), while a 60 s window's context alone exceeds 1 MiB. It also bounds a
 * seek's download to about a second on a slow link.
 *
 * Window bounds and every ts are RelNs: ns relative to the dataset's `origin_ns`, not epoch ns.
 * Datasets span minutes to hours (the demo run: 0.1 s to 120.1 s), far below 2^53 ns (~104
 * days), so they are exact JS numbers; the normalizer (`nsToSafeNumber`) rejects any that is not.
 */
const WINDOW_NS = 10 * NS_PER_S;
/** Where the cursor sits inside a window fetched while moving forward. */
const LEAD_NS = 2 * NS_PER_S;
const LEVELS = 25;
const MAX_TRADES = 5000;
/** Events per timeline page (the server's default); a window takes as many pages as it needs. */
const PAGE_EVENTS = 5000;
const LADDER_ROWS = 10;
const SPEEDS = [0.1, 0.25, 0.5, 1, 2, 5, 20];

const LEAD =
  "The historical book as observed, with this run's simulated orders and fills replayed on top. Simulated orders never entered the historical book and did not move prices.";

interface Win {
  t0: number;
  t1: number;
}

interface Loaded {
  win: Win;
  tl: RunTimeline;
  rp: ReplayWindow;
}

type BottomTab = 'events' | 'orders' | 'provenance';
/** Panes of the phone layout (below 600 px), one at a time. */
type PhoneTab = 'book' | 'events' | 'order';

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

function RunLinks({ runId }: { runId: string }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const download = async () => {
    setPending(true);
    setError(null);
    try {
      saveText(await api.runReport(runId), `run-${runId}-report.md`);
    } catch (e) {
      setError(e);
    } finally {
      setPending(false);
    }
  };
  return (
    <>
      <RunContextLinks runId={runId} omit={['replay']} />
      <button type="button" onClick={() => void download()} disabled={pending}>
        {pending ? 'Downloading report\u2026' : 'Download report (.md)'}
      </button>
      {error !== null ? <ErrorBox error={error} /> : null}
    </>
  );
}

export default function ReplayTerminalPage() {
  const { runId = '' } = useParams();
  const run = useAsync((signal) => api.run(runId, { signal }), [runId]);
  const runDataset = run.data?.dataset ?? null;
  const dsId = runDataset?.id ?? null;
  // The manifest of the exact dataset version this run used.
  const dataset = useAsync(
    (signal) =>
      runDataset === null
        ? Promise.resolve(null)
        : api.datasetVersion(runDataset.id, runDataset.version, { signal }).then((v) => v.manifest),
    [runDataset?.id, runDataset?.version],
  );

  if (run.data && dataset.data) {
    return (
      <TimeDigitsContext.Provider value={digitsForUnit(dataset.data.timestamps.input_unit)}>
        <Terminal key={runId} run={run.data} ds={dataset.data} />
      </TimeDigitsContext.Provider>
    );
  }
  return (
    <Workspace
      context={run.data?.label ?? runId}
      toolbar={
        <>
          {dsId !== null ? <BookModeRun datasetId={dsId} query={null} /> : null}
          <RunLinks runId={runId} />
        </>
      }
    >
      <div className="book-state">
        {run.error ? <ErrorBox error={run.error} onRetry={run.reload} /> : null}
        {dataset.error ? <ErrorBox error={dataset.error} onRetry={dataset.reload} /> : null}
        {!run.error && !dataset.error ? <Spinner label="Loading run and dataset" /> : null}
      </div>
    </Workspace>
  );
}

function Provenance({ run, ds }: { run: RunDetail; ds: DatasetManifest }) {
  const c = run.config;
  return (
    <section className="term-prov" aria-label="Run assumptions and dataset provenance">
      <div className="term-prov-group">
        <span className="term-prov-label">Run</span>
        <span className="mono">{run.run_id}</span>
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
          {run.engine.name} {run.engine.version}
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
          {ds.source.vendor} · {ds.source.exchange} · {ds.source.symbol} · local timestamps {ds.timestamps.local_ts} ·
          version {run.dataset.version} · sha256 <HashText hash={run.dataset.events_sha256} chars={10} />
        </span>
      </div>
    </section>
  );
}

function Terminal({ run, ds }: { run: RunDetail; ds: DatasetManifest }) {
  const runId = run.run_id;
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
  const [progress, setProgress] = useState<TimelineProgress | null>(null);
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
  const jumpOrderRef = useRef<HTMLInputElement | null>(null);
  const [focus, setFocus] = useState<{ t: number; key: number } | null>(null);
  const [tab, setTab] = useState<BottomTab>('events');
  const phone = usePhone();
  const [phoneTab, setPhoneTab] = useState<PhoneTab>('book');

  // ----- window loading ------------------------------------------------------------------
  // No time or order requested: start at the run start, learned from a count-only timeline call
  // over the empty-width window at the dataset start (its fills/equity/orders are tiny; the
  // default 60 s window's may exceed the anonymous response cap).
  useEffect(() => {
    if (!wantDefault || win !== null) return;
    const ctl = new AbortController();
    setLoading(true);
    setLoadError(null);
    api.timeline(runId, { t0: ds.start, t1: ds.start, max_events: 0 }, { signal: ctl.signal }).then(
      (tl) => {
        if (ctl.signal.aborted) return;
        const t = clampT(tl.run_start);
        setCursorRaw(t);
        setWin(windowAt(t, 0, ds));
      },
      (e: unknown) => {
        if (ctl.signal.aborted) return;
        setLoadError(e);
        setLoading(false);
      },
    );
    return () => ctl.abort();
  }, [runId, ds, wantDefault, win, clampT, reloadNonce]);

  // Each window loads all of its timeline pages, one after another, next to its book replay.
  // A new window (seek, playback, jump) or run aborts the previous load before any of its
  // pages lands, so windows and runs never mix and a partial window is never shown.
  useEffect(() => {
    if (win === null) return;
    const ctl = new AbortController();
    setLoading(true);
    setLoadError(null);
    setProgress(null);
    Promise.all([
      fetchTimelineWindow(
        win,
        (cursor, signal) => api.timeline(runId, { t0: win.t0, t1: win.t1, max_events: PAGE_EVENTS, cursor }, { signal }),
        ctl.signal,
        (p) => {
          if (!ctl.signal.aborted) setProgress(p);
        },
      ),
      api.replay(ds.id, { t0: win.t0, t1: win.t1, levels: LEVELS, max_trades: MAX_TRADES }, { signal: ctl.signal, version: run.dataset.version }),
    ]).then(
      ([tl, rp]) => {
        if (ctl.signal.aborted) return;
        setLoaded({ win, tl, rp });
        setLoading(false);
        setProgress(null);
      },
      (e: unknown) => {
        if (ctl.signal.aborted) return;
        ctl.abort(); // stop the sibling request
        setLoadError(e);
        setLoading(false);
        setProgress(null);
      },
    );
    return () => ctl.abort();
    // `win` is listed by value so an identical window is not fetched twice.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [runId, ds.id, run.dataset.version, win?.t0, win?.t1, reloadNonce]);

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
  const sortedEvents = useMemo(() => (tl ? [...tl.events].sort((a, b) => a.ts - b.ts || a.seq - b.seq) : []), [tl]);
  const eventTypes = useMemo(() => [...new Set(sortedEvents.map((e) => e.body.type))].sort(), [sortedEvents]);
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

  /** The previous/next order of the run (by decision time) among the orders of the loaded window. */
  const stepOrder = useCallback(
    (dir: 1 | -1) => {
      setNotice(null);
      if (!tl || cursor === null) return;
      const byDecision = [...tl.orders].sort((a, b) => a.decision_ts - b.decision_ts || a.order_id - b.order_id);
      const o =
        dir === 1
          ? byDecision.find((x) => x.decision_ts > cursor)
          : [...byDecision].reverse().find((x) => x.decision_ts < cursor);
      if (o) void jumpToOrder(o.order_id);
      else setNotice(`No ${dir === 1 ? 'later' : 'earlier'} order of this run in the loaded window.`);
    },
    [tl, cursor, jumpToOrder],
  );

  // ----- keyboard ------------------------------------------------------------------------
  const keys = useRef({ step, togglePlay, stepOrder });
  keys.current = { step, togglePlay, stepOrder };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const target = e.target instanceof HTMLElement ? e.target : null;
      if (target?.closest('input, select, textarea, [contenteditable="true"]')) return;
      if (e.key === ' ') {
        if (target?.closest('button, a, [role="grid"], [role="tab"]')) return;
        e.preventDefault();
        keys.current.togglePlay();
      } else if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
        if (target?.closest('[role="toolbar"], [role="grid"], [role="tablist"], [role="separator"]')) return;
        e.preventDefault();
        keys.current.step(e.key === 'ArrowRight' ? 1 : -1);
      } else if (e.key === 'j' || e.key === 'J') {
        e.preventDefault();
        jumpRef.current?.focus();
        jumpRef.current?.select();
      } else if (e.key === 'o' || e.key === 'O') {
        e.preventDefault();
        jumpOrderRef.current?.focus();
        jumpOrderRef.current?.select();
      } else if (e.key === ']' || e.key === '[') {
        e.preventDefault();
        keys.current.stepOrder(e.key === ']' ? 1 : -1);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const selectOrder = useCallback((orderId: number) => {
    setSelectedOrder(orderId);
    setJumpOrderText(String(orderId));
  }, []);

  const moveCursor = useCallback(
    (t: number) => {
      setPlaying(false);
      setCursor(t);
    },
    [setCursor],
  );

  const jumpNs = parseSeconds(jumpText.trim());
  const submitJump = (e: FormEvent) => {
    e.preventDefault();
    if (jumpNs === null) return;
    setPlaying(false);
    setPendingStep(null);
    setNotice(null);
    setCursor(jumpNs);
  };

  const submitJumpOrder = (e: FormEvent) => {
    e.preventDefault();
    const text = jumpOrderText.trim();
    if (!/^\d+$/.test(text)) return;
    void jumpToOrder(Number(text));
  };

  // ----- selected order ------------------------------------------------------------------
  const detail = useAsync(
    (signal) => (selectedOrder === null ? Promise.resolve(null) : api.orderDetail(runId, selectedOrder, { signal })),
    [runId, selectedOrder],
  );

  useInspector(
    selectedOrder === null ? 'Order Book Lab \u00b7 with run' : `Order #${selectedOrder}`,
    selectedOrder === null ? (
      <div className="col prose">
        <p>{LEAD}</p>
        <p className="dim">
          Keys: <kbd>Space</kbd> play/pause, <kbd>{'\u2190'}</kbd> <kbd>{'\u2192'}</kbd> previous/next event (respects the
          event filters), <kbd>J</kbd> jump to time, <kbd>O</kbd> jump to order, <kbd>[</kbd> <kbd>]</kbd> previous/next
          order in the loaded window.
        </p>
        <p className="dim">Select an order in the ladder, the event log or the order list to see its lifecycle here.</p>
      </div>
    ) : (
      <OrderSummary runId={runId} orderId={selectedOrder} detail={detail} onCursor={moveCursor} />
    ),
    [selectedOrder, detail.data, detail.error, detail.loading],
  );

  // ----- cursor-derived state ------------------------------------------------------------
  const cur = cursor ?? ds.start;
  const frame = rp
    ? (() => {
        const i = frameIndexAt(rp.depth, cur);
        return i >= 0 ? rp.depth[i] : null;
      })()
    : null;
  const bboIdx = rp ? seriesIndexAt(rp.bbo.ts, cur) : -1;
  const resting = useMemo(() => (tl ? ordersRestingAt(tl.orders, cur) : []), [tl, cur]);
  const equityTs = useMemo(() => (tl ? tl.equity.map((s) => s.ts) : []), [tl]);
  const sampleIdx = seriesIndexAt(equityTs, cur);
  const sample = tl && sampleIdx >= 0 ? tl.equity[sampleIdx] : null;
  const cursorInWindow = loaded !== null && cur >= loaded.win.t0 && cur <= loaded.win.t1;
  const marketQuery = loaded ? { t0: loaded.win.t0, len: loaded.win.t1 - loaded.win.t0, levels: LEVELS } : null;
  // The event log only switches to a window once all of its pages arrived; until then the
  // label counts them.
  const loadingLabel =
    progress === null
      ? `Loading ${loaded ? 'window' : 'the first window'}`
      : `Loading window events ${fmtInt(progress.loaded)} / ${fmtInt(progress.total)}`;

  const speedControl = (
    <span className="term-field">
      <label className="book-field" htmlFor="term-speed">
        Speed
      </label>
      <select id="term-speed" value={speed} onChange={(e) => setSpeed(Number(e.target.value))} title="Playback speed relative to recorded time">
        {SPEEDS.map((s) => (
          <option key={s} value={s}>
            {s}
            {'\u00d7'}
          </option>
        ))}
      </select>
    </span>
  );
  const jumpForms = (
    <>
      <form className="row term-jump" onSubmit={submitJump} aria-label="Jump to time">
        <label className="book-field" htmlFor="term-jump">
          Jump to time <span className="unit">s</span>
        </label>
        <input
          id="term-jump"
          ref={jumpRef}
          type="text"
          inputMode="decimal"
          placeholder={secondsText(cur)}
          value={jumpText}
          onChange={(e) => setJumpText(e.target.value)}
          size={9}
          title="Seconds after the dataset origin (key J)"
        />
        <button type="submit" disabled={jumpNs === null}>
          Go
        </button>
      </form>
      <form className="row term-jump" onSubmit={submitJumpOrder} aria-label="Jump to order">
        <label className="book-field" htmlFor="term-jump-order">
          Jump to order
        </label>
        <input
          id="term-jump-order"
          ref={jumpOrderRef}
          type="text"
          inputMode="numeric"
          placeholder="order id"
          value={jumpOrderText}
          onChange={(e) => setJumpOrderText(e.target.value)}
          size={7}
          title="Order id (key O)"
        />
        <button type="submit" disabled={!/^\d+$/.test(jumpOrderText.trim())}>
          Go
        </button>
      </form>
    </>
  );
  const runLinks = (
    <details className="ws-more term-more">
      <summary>{phone ? 'Speed, jump, links' : 'Run links'}</summary>
      <div className="ws-more-body">
        {phone ? speedControl : null}
        {phone ? jumpForms : null}
        {phone ? <BookModeRun datasetId={ds.id} query={marketQuery} /> : null}
        <RunLinks runId={runId} />
      </div>
    </details>
  );
  const toolbar = (
    <>
      <div className="term-group term-transport" role="group" aria-label="Playback">
        <button
          type="button"
          className="btn-primary term-play"
          aria-pressed={playing}
          onClick={togglePlay}
          disabled={!loaded}
          title={playing ? 'Pause playback (Space)' : 'Play from the cursor (Space)'}
        >
          <span className="term-play-icon" aria-hidden="true">
            {playing ? '\u275a\u275a' : '\u25b6'}
          </span>
          {playing ? 'Pause' : 'Play'}
        </button>
        <button type="button" onClick={() => step(-1)} disabled={!loaded} aria-label="Previous event" title={'Previous event (\u2190)'}>
          {'\u2190'}
          <span className="term-step-label"> Prev</span>
        </button>
        <button type="button" onClick={() => step(1)} disabled={!loaded} aria-label="Next event" title={'Next event (\u2192)'}>
          <span className="term-step-label">Next </span>
          {'\u2192'}
        </button>
      </div>
      {phone ? null : speedControl}
      <span className={playing ? 'term-state is-playing' : 'term-state'} role="status">
        {playing ? `Playing ${speed}\u00d7` : 'Paused'}
      </span>
      <div className="term-group term-scrubber">
        <label htmlFor="term-scrub" className="visually-hidden">
          Cursor position in the dataset
        </label>
        <input
          id="term-scrub"
          className="book-scrub"
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
        <span className="book-cursor" title="Playback cursor on the exchange clock">
          <ClockText ns={cursor === null ? null : Math.round(cursor)} /> <span className="term-clock-tag">exch</span>
        </span>
      </div>
      {loading ? <Spinner label={loadingLabel} /> : null}
      {phone ? null : (
        <>
          <ToolbarDivider />
          {jumpForms}
          <ToolbarDivider />
          <BookModeRun datasetId={ds.id} query={marketQuery} />
        </>
      )}
      {runLinks}
    </>
  );

  const ordersColumns: Column<OrderRecord>[] = [
    {
      id: 'order',
      header: 'Order',
      rowHeader: true,
      width: 90,
      cell: (o) => (
        <button type="button" className="btn-link mono" onClick={() => selectAndMove(o)}>
          {o.order_id}
        </button>
      ),
      sortValue: (o) => o.order_id,
      raw: (o) => String(o.order_id),
    },
    { id: 'side', header: 'Side', width: 64, cell: (o) => <SideBadge side={o.side} /> },
    { id: 'price', header: 'Price', numeric: true, cell: (o) => fmtPrice(o.price_tick, inst), sortValue: (o) => o.price_tick },
    { id: 'qty', header: 'Qty', numeric: true, cell: (o) => fmtQty(o.qty, inst), sortValue: (o) => o.qty },
    {
      id: 'outcome',
      header: 'Outcome',
      cell: (o) => (
        <Badge kind={outcomeKind(o.outcome)} title={OUTCOME_MEANING[o.outcome]}>
          {words(o.outcome)}
        </Badge>
      ),
      sortValue: (o) => o.outcome,
    },
    { id: 'decision', header: 'Decision (local)', cell: (o) => <ClockText ns={o.decision_ts} />, sortValue: (o) => o.decision_ts },
    { id: 'arrival', header: 'Arrival (exch)', cell: (o) => <ClockText ns={o.exch_arrival_ts} />, sortValue: (o) => o.exch_arrival_ts },
  ];

  function selectAndMove(o: OrderRecord) {
    selectOrder(o.order_id);
    moveCursor(o.decision_ts);
  }

  let body: ReactNode;
  if (!loaded || !tl || !rp) {
    body = (
      <div className="book-state col">
        {loadError ? (
          <ErrorBox error={loadError} onRetry={() => setReloadNonce((n) => n + 1)} />
        ) : (
          <Spinner label={loadingLabel} />
        )}
        {jumpError ? <ErrorBox error={jumpError} /> : null}
      </div>
    );
  } else {
    const heatmapPanel = (
      <Panel
        title="Book heatmap"
        basis="observed"
        fill
        flush
        actions={<Badge kind="modeled">own orders modeled</Badge>}
        caption={rp.bbo_downsampled ? `Best bid/ask lines downsampled 1:${fmtDuration(rp.step_ns)}` : undefined}
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
          onCursor={moveCursor}
          loadingLabel={loading ? loadingLabel : null}
        />
      </Panel>
    );
    const ladderPanel = (
      <Panel title="Depth at cursor" basis="observed" fill className="term-depth">
        <dl className="term-bbo" aria-label="Best bid and ask at the cursor">
          <div className="term-bbo-side term-bbo-bid">
            <dt>Best bid</dt>
            <dd>
              <span className="term-bbo-px bid">
                {bboIdx >= 0 && rp.bbo.bid_tick[bboIdx] !== null ? fmtPrice(rp.bbo.bid_tick[bboIdx], inst) : DASH}
              </span>
              <span className="term-bbo-qty">
                {bboIdx >= 0 && rp.bbo.bid_tick[bboIdx] !== null ? `\u00d7 ${fmtQty(rp.bbo.bid_qty[bboIdx], inst)}` : ''}
              </span>
            </dd>
          </div>
          <div className="term-bbo-side term-bbo-ask">
            <dt>Best ask</dt>
            <dd>
              <span className="term-bbo-px ask">
                {bboIdx >= 0 && rp.bbo.ask_tick[bboIdx] !== null ? fmtPrice(rp.bbo.ask_tick[bboIdx], inst) : DASH}
              </span>
              <span className="term-bbo-qty">
                {bboIdx >= 0 && rp.bbo.ask_tick[bboIdx] !== null ? `\u00d7 ${fmtQty(rp.bbo.ask_qty[bboIdx], inst)}` : ''}
              </span>
            </dd>
          </div>
        </dl>
        <div className="term-ladder-scroll">
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
    );
    const accountPanel = (
      <Panel title="Account at cursor" basis="derived" fill>
        {sample ? (
          <>
            <div className="stats">
              <Stat label="Position" value={fmtQty(sample.position, inst)} hint="From simulated fills" />
              <Stat label="Known to strategy" value={fmtQty(sample.known_position, inst)} />
              <Stat label="Open orders" value={fmtInt(sample.open_orders)} />
              <Stat label="Realized" value={fmtPnl(sample.realized_pnl, 2)} hint="quote, before fees" />
              <Stat label="Unrealized" value={fmtPnl(sample.unrealized_pnl, 2)} hint="quote, before fees" />
              <Stat label="Fees" value={fmtMoney(sample.fees, 2)} hint="Quote currency; positive is charged, negative is a rebate." />
              <Stat label={'Net P&L / equity'} value={fmtMoney(sample.equity, 2)} hint="quote" />
            </div>
            <p className="note">
              Exchange-truth fills marked at the local mid; realized + unrealized {'\u2212'} fees. {'\u2018'}Known to
              strategy{'\u2019'} includes only received fill notifications.
            </p>
            <p className="note">
              Last account sample at or before the cursor: <ClockText ns={sample.ts} /> on the local clock (strategy tick{' '}
              {fmtInt(sample.tick_no)}). Samples are periodic, so changes between samples are not shown here.
            </p>
          </>
        ) : (
          <Empty>No account sample at or before the cursor in this window.</Empty>
        )}
      </Panel>
    );
    const tabs = (
      <Tabs<BottomTab>
        label="Run detail"
        className="term-tabs"
        active={tab}
        onChange={setTab}
        tabs={[
          { id: 'events', label: 'Event log', count: fmtInt(filtered.length) },
          { id: 'orders', label: 'Orders alive', count: fmtInt(tl.orders.length) },
          { id: 'provenance', label: 'Run and dataset' },
        ]}
      >
        {tab === 'events' ? (
          <>
            <Toolbar label="Event filters" className="term-filters">
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
            </Toolbar>
            <EventLog
              events={filtered}
              cursor={cur}
              instrument={inst}
              selectedOrder={selectedOrder}
              onSelectOrder={selectOrder}
              onJump={(ev) => {
                moveCursor(ev.ts);
                if (ev.order_id !== null) selectOrder(ev.order_id);
              }}
            />
          </>
        ) : tab === 'orders' ? (
          <DataTable
            label="Orders alive in this window"
            rows={tl.orders}
            rowKey={(o) => String(o.order_id)}
            columns={ordersColumns}
            fill
            selectedKey={selectedOrder === null ? null : String(selectedOrder)}
            onSelect={selectAndMove}
            rowClassName={(o) =>
              o.exch_arrival_ts !== null && o.exch_arrival_ts <= cur && (o.terminal_ts === null || o.terminal_ts > cur)
                ? 'term-live'
                : undefined
            }
            empty="No orders of this run were alive in this window."
            footer={
              <span className="dim small">
                <BasisBadge basis="modeled" /> Highlighted rows rest at the simulated exchange at the cursor.
              </span>
            }
          />
        ) : (
          <div className="term-tab-scroll">
            <Provenance run={run} ds={ds} />
          </div>
        )}
      </Tabs>
    );

    const windowRest = (
      <>
        {fmtInt(tl.events_total)} tape events · {fmtInt(tl.fills.length)} simulated fills · {fmtInt(tl.orders.length)} orders alive ·{' '}
        {fmtInt(rp.depth.length)} depth frames · {fmtInt(rp.trades.ts.length)} market trades · run span <ClockText ns={tl.run_start} />{' '}
        {'\u2192'} <ClockText ns={tl.run_end} />
      </>
    );
    const windowSpan = (
      <>
        window <ClockText ns={tl.t0} /> {'\u2192'} <ClockText ns={tl.t1} />
        {cursorInWindow ? '' : ' · cursor outside the loaded window, fetching'}
      </>
    );
    const selectedOrderPane = <SelectedOrder runId={runId} orderId={selectedOrder} detail={detail} onCursor={moveCursor} />;
    body = (
      <div className="book-body">
        <h1 className="term-title">
          Order book replay <span className="dim">{'\u00b7'}</span> <span className="mono">{run.label ?? runId}</span>
        </h1>
        {phone ? (
          <details className="book-window">
            <summary>{windowSpan}</summary>
            {windowRest}
          </details>
        ) : (
          <p className="book-window">
            {windowSpan} · {windowRest}
          </p>
        )}
        {notice ? (
          <p className="book-window" role="status">
            {notice}
          </p>
        ) : null}
        {loadError ? <ErrorBox error={loadError} onRetry={() => setReloadNonce((n) => n + 1)} /> : null}
        {jumpError ? <ErrorBox error={jumpError} /> : null}
        {rp.trades_truncated ? (
          <Notice tag="TRUNCATED">
            More than {fmtInt(MAX_TRADES)} market trades in this window; only the {fmtInt(rp.trades.ts.length)} largest are
            drawn.
          </Notice>
        ) : null}
        {rp.bbo_downsampled ? (
          <Notice tag="DOWNSAMPLED">Best bid/ask lines are downsampled to one entry per {fmtDuration(rp.step_ns)}.</Notice>
        ) : null}
        <div className="book-panes">
          {phone ? (
            <PaneTabs<PhoneTab>
              label="Order Book Replay panes"
              active={phoneTab}
              onChange={setPhoneTab}
              tabs={[
                {
                  id: 'book',
                  label: 'Book',
                  content: (
                    <SplitPane
                      id="book.run.phone"
                      orientation="vertical"
                      panes={[
                        { id: 'heatmap', label: 'Book heatmap', stackHeight: 'max(360px, 64dvh)', content: heatmapPanel },
                        { id: 'ladder', label: 'Depth at cursor', stackHeight: 'max(420px, 70dvh)', content: ladderPanel },
                        { id: 'account', label: 'Account at cursor', stackHeight: 360, content: accountPanel },
                      ]}
                    />
                  ),
                },
                { id: 'events', label: 'Events', count: fmtInt(filtered.length), content: tabs },
                { id: 'order', label: selectedOrder === null ? 'Order' : `Order ${selectedOrder}`, content: selectedOrderPane },
              ]}
            />
          ) : (
          <SplitPane
            id="book.run"
            orientation="vertical"
            panes={[
              {
                id: 'top',
                label: 'Book',
                defaultSize: '66%',
                minSize: '30%',
                content: (
                  <SplitPane
                    id="book.run.top"
                    panes={[
                      { id: 'heatmap', label: 'Book heatmap', defaultSize: '70%', minSize: '35%', stackHeight: 'max(320px, 60dvh)', content: heatmapPanel },
                      {
                        id: 'side',
                        label: 'Depth and account',
                        defaultSize: '30%',
                        minSize: 300,
                        content: (
                          <SplitPane
                            id="book.run.side"
                            orientation="vertical"
                            panes={[
                              { id: 'ladder', label: 'Depth at cursor', defaultSize: '72%', minSize: '20%', content: ladderPanel },
                              {
                                id: 'account',
                                label: 'Account at cursor',
                                defaultSize: '28%',
                                minSize: '15%',
                                collapsible: true,
                                content: accountPanel,
                              },
                            ]}
                          />
                        ),
                      },
                    ]}
                  />
                ),
              },
              {
                id: 'bottom',
                label: 'Events and orders',
                defaultSize: '34%',
                minSize: '20%',
                content: (
                  <SplitPane
                    id="book.run.bottom"
                    panes={[
                      { id: 'tabs', label: 'Event log and orders', defaultSize: '62%', minSize: '35%', content: tabs },
                      {
                        id: 'order',
                        label: 'Selected order',
                        defaultSize: '38%',
                        minSize: 280,
                        content: selectedOrderPane,
                      },
                    ]}
                  />
                ),
              },
            ]}
          />
          )}
        </div>
      </div>
    );
  }

  return (
    <Workspace context={run.label ?? runId} layout toolbar={toolbar}>
      {body}
    </Workspace>
  );
}

function OrderTimes({ d, onCursor }: { d: OrderDetail; onCursor: (t: number) => void }) {
  return (
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
  );
}

function OrderHead({ d }: { d: OrderDetail }) {
  return (
    <div className="row">
      <SideBadge side={d.order.side} />
      <span className="mono">
        {fmtQty(d.order.qty, d.instrument)} @ {fmtPrice(d.order.price_tick, d.instrument)} · {tifLabel(d.order.tif)} ·{' '}
        {d.order.ord_type}
      </span>
      <Badge kind={outcomeKind(d.order.outcome)} title={OUTCOME_MEANING[d.order.outcome]}>
        {words(d.order.outcome)}
      </Badge>
    </div>
  );
}

/** The selected order in the inspector: summary, lifecycle times and the forensics link. */
function OrderSummary({
  runId,
  orderId,
  detail,
  onCursor,
}: {
  runId: string;
  orderId: number;
  detail: AsyncResult<OrderDetail | null>;
  onCursor: (t: number) => void;
}) {
  const d = detail.data;
  return (
    <div className="col">
      {detail.loading && !d ? <Spinner label="Loading order lifecycle" /> : null}
      {detail.error ? <ErrorBox error={detail.error} onRetry={detail.reload} /> : null}
      {d ? (
        <>
          <OrderHead d={d} />
          <p>{d.explanation.headline}</p>
          <OrderTimes d={d} onCursor={onCursor} />
        </>
      ) : null}
      <Link to={paths.order(runId, orderId)}>Open in Execution Forensics</Link>
    </div>
  );
}

function SelectedOrder({
  runId,
  orderId,
  detail,
  onCursor,
}: {
  runId: string;
  orderId: number | null;
  detail: AsyncResult<OrderDetail | null>;
  onCursor: (t: number) => void;
}) {
  const [selectedSeq, setSelectedSeq] = useState<number | null>(null);
  const cited = useMemo(() => new Set<number>(), []);
  const d = detail.data;
  return (
    <Panel
      title={orderId === null ? 'Selected order' : `Selected order #${orderId}`}
      basis="modeled"
      fill
      actions={
        orderId === null ? null : (
          <Link className="btn" to={paths.order(runId, orderId)}>
            Full forensics
          </Link>
        )
      }
    >
      {orderId === null ? <Empty>Select an order in the ladder, the event log or the order list, or jump to one by id.</Empty> : null}
      {detail.loading && orderId !== null ? <Spinner label="Loading order lifecycle" /> : null}
      {detail.error ? <ErrorBox error={detail.error} onRetry={detail.reload} /> : null}
      {d ? (
        <>
          <OrderHead d={d} />
          <p>{d.explanation.headline}</p>
          <OrderTimes d={d} onCursor={onCursor} />
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
        </>
      ) : null}
    </Panel>
  );
}
