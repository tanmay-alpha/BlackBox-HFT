import { useCallback, useEffect, useMemo, useState } from 'react';
import type { FormEvent } from 'react';
import { Link, useParams, useSearchParams } from 'react-router-dom';

import { api } from '../../api/client';
import type { DatasetManifest, ReplayWindow } from '../../api/types';
import { DASH, fmtDuration, fmtInt, fmtPrice, fmtQty } from '../../components/fmt';
import { PageHeader, usePageTitle } from '../../components/page';
import { Badge, ClockText, Empty, ErrorBox, Panel, Spinner, useAsync } from '../../components/ui';
import { DepthLadder } from './DepthLadder';
import { HeatmapView } from './HeatmapView';
import { frameIndexAt, seriesIndexAt } from './heatmap';
import type { IntensityScale } from './heatmap';

const NS_PER_S = 1_000_000_000;
const DEFAULT_LENGTH_NS = 60 * NS_PER_S;
const DEFAULT_LEVELS = 25;
/** Sent explicitly so the truncation notice can name the cap. */
const MAX_TRADES = 5000;

/** `0` stands for "the whole dataset". */
const LENGTHS: { ns: number; label: string }[] = [
  { ns: 250_000_000, label: '250 ms' },
  { ns: 1 * NS_PER_S, label: '1 s' },
  { ns: 5 * NS_PER_S, label: '5 s' },
  { ns: 15 * NS_PER_S, label: '15 s' },
  { ns: 60 * NS_PER_S, label: '60 s' },
  { ns: 300 * NS_PER_S, label: '5 min' },
  { ns: 900 * NS_PER_S, label: '15 min' },
  { ns: 3600 * NS_PER_S, label: '60 min' },
  { ns: 0, label: 'entire dataset' },
];
const LEVEL_CHOICES = [10, 25, 50, 100];
const LADDER_ROWS = [5, 10, 20];
const SPEEDS = [0.1, 0.5, 1, 5, 20, 100];

interface WindowQuery {
  t0: number;
  t1: number;
  /** Requested length; 0 means the whole dataset. */
  lengthNs: number;
  levels: number;
}

function intParam(raw: string | null): number | null {
  if (raw === null || raw.trim() === '') return null;
  const n = Number(raw);
  return Number.isFinite(n) ? Math.round(n) : null;
}

/** The applied window: URL parameters where present, dataset defaults otherwise. */
function resolveQuery(ds: DatasetManifest, sp: URLSearchParams): WindowQuery {
  const lenParam = intParam(sp.get('len'));
  const lengthNs = lenParam !== null && lenParam >= 0 ? lenParam : DEFAULT_LENGTH_NS;
  const levelsParam = intParam(sp.get('levels'));
  const levels = Math.min(100, Math.max(1, levelsParam ?? DEFAULT_LEVELS));
  if (lengthNs === 0) {
    return { t0: ds.start, t1: ds.end, lengthNs, levels };
  }
  const t0Param = intParam(sp.get('t0'));
  const latestStart = Math.max(ds.start, ds.end - 1);
  const t0 = Math.min(latestStart, Math.max(ds.start, t0Param ?? ds.start));
  return { t0, t1: Math.min(ds.end, t0 + lengthNs), lengthNs, levels };
}

function secondsText(ns: number): string {
  return String(Number((ns / NS_PER_S).toFixed(6)));
}

function ReplayControls({
  ds,
  query,
  onApply,
  busy,
}: {
  ds: DatasetManifest;
  query: WindowQuery;
  onApply: (t0: number, lengthNs: number, levels: number) => void;
  busy: boolean;
}) {
  const [start, setStart] = useState(secondsText(query.t0));
  const [lengthNs, setLengthNs] = useState(query.lengthNs);
  const [levels, setLevels] = useState(query.levels);

  // Paging with the buttons below changes the applied window; mirror it in the inputs.
  useEffect(() => {
    setStart(secondsText(query.t0));
    setLengthNs(query.lengthNs);
    setLevels(query.levels);
  }, [query.t0, query.lengthNs, query.levels]);

  const startNs = Number(start) * NS_PER_S;
  const startValid =
    start.trim() !== '' && Number.isFinite(startNs) && startNs >= ds.start && startNs <= ds.end;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!startValid) return;
    onApply(Math.round(startNs), lengthNs, levels);
  };

  const whole = query.lengthNs === 0;
  const span = query.t1 - query.t0;

  return (
    <form className="row replay-controls" onSubmit={submit} noValidate>
      <div className="field">
        <label className="field-label" htmlFor="replay-start">
          Window start <span className="field-unit">s after origin</span>
        </label>
        <div className="field-control">
          <input
            id="replay-start"
            type="text"
            inputMode="decimal"
            value={start}
            onChange={(e) => setStart(e.target.value)}
            disabled={lengthNs === 0}
            aria-invalid={!startValid}
            aria-describedby="replay-start-help"
          />
        </div>
        <span
          className={startValid ? 'field-help' : 'field-error'}
          id="replay-start-help"
          role={startValid ? undefined : 'alert'}
        >
          Dataset spans {secondsText(ds.start)} to {secondsText(ds.end)} s.
        </span>
      </div>
      <div className="field">
        <label className="field-label" htmlFor="replay-length">
          Window length
        </label>
        <div className="field-control">
          <select
            id="replay-length"
            value={lengthNs}
            onChange={(e) => setLengthNs(Number(e.target.value))}
          >
            {LENGTHS.map((l) => (
              <option key={l.ns} value={l.ns}>
                {l.label}
              </option>
            ))}
          </select>
        </div>
        <span className="field-help">One depth frame per 1/300 of it (at least 1 ms).</span>
      </div>
      <div className="field">
        <label className="field-label" htmlFor="replay-levels">
          Depth levels per side
        </label>
        <div className="field-control">
          <select
            id="replay-levels"
            value={levels}
            onChange={(e) => setLevels(Number(e.target.value))}
          >
            {(LEVEL_CHOICES.includes(levels) ? LEVEL_CHOICES : [...LEVEL_CHOICES, levels]).map(
              (n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ),
            )}
          </select>
        </div>
        <span className="field-help">Levels beyond this are not fetched.</span>
      </div>
      <div className="row replay-controls-buttons">
        <button type="submit" className="btn-primary" disabled={!startValid || busy}>
          Load window
        </button>
        <button
          type="button"
          disabled={busy || whole || query.t0 <= ds.start}
          onClick={() => onApply(Math.max(ds.start, query.t0 - span), query.lengthNs, query.levels)}
        >
          Previous window
        </button>
        <button
          type="button"
          disabled={busy || whole || query.t1 >= ds.end}
          onClick={() => onApply(query.t1, query.lengthNs, query.levels)}
        >
          Next window
        </button>
      </div>
    </form>
  );
}

function Playback({
  win,
  cursor,
  playing,
  speed,
  onCursor,
  onPlaying,
  onSpeed,
}: {
  win: ReplayWindow;
  cursor: number;
  playing: boolean;
  speed: number;
  onCursor: (t: number) => void;
  onPlaying: (playing: boolean) => void;
  onSpeed: (speed: number) => void;
}) {
  const fi = frameIndexAt(win.depth, cursor);
  const atEnd = cursor >= win.t1;

  const stepFrame = (dir: -1 | 1) => {
    onPlaying(false);
    if (win.depth.length === 0) return;
    // Stepping back from between two frames first lands on the frame in force.
    const onFrame = fi >= 0 && win.depth[fi].ts === cursor;
    const target = dir === 1 ? fi + 1 : onFrame ? fi - 1 : fi;
    const clamped = Math.min(win.depth.length - 1, Math.max(0, target));
    onCursor(win.depth[clamped].ts);
  };

  return (
    <div className="row playback">
      <button
        type="button"
        className="btn-primary"
        aria-pressed={playing}
        onClick={() => {
          if (!playing && atEnd) onCursor(win.t0);
          onPlaying(!playing);
        }}
      >
        {playing ? 'Pause' : atEnd ? 'Replay from start' : 'Play'}
      </button>
      <button type="button" onClick={() => stepFrame(-1)} disabled={fi <= 0 && !(fi === 0 && win.depth[0].ts < cursor)}>
        Previous frame
      </button>
      <button type="button" onClick={() => stepFrame(1)} disabled={fi >= win.depth.length - 1}>
        Next frame
      </button>
      <label htmlFor="replay-speed">Speed</label>
      <select id="replay-speed" value={speed} onChange={(e) => onSpeed(Number(e.target.value))}>
        {SPEEDS.map((s) => (
          <option key={s} value={s}>
            {s}
            {'\u00d7'} recorded time
          </option>
        ))}
      </select>
      <label htmlFor="replay-scrub" className="visually-hidden">
        Cursor position in the window
      </label>
      <input
        id="replay-scrub"
        className="playback-scrub"
        type="range"
        min={win.t0}
        max={win.t1}
        step={Math.max(1, win.step_ns)}
        value={cursor}
        onChange={(e) => {
          onPlaying(false);
          onCursor(Number(e.target.value));
        }}
      />
      <span className="mono">
        cursor <ClockText ns={Math.round(cursor)} />
      </span>
    </div>
  );
}

function ReplayWorkspace({ ds, win }: { ds: DatasetManifest; win: ReplayWindow }) {
  const inst = ds.instrument;
  const [cursor, setCursor] = useState(win.t0);
  const [playing, setPlaying] = useState(false);
  const [speed, setSpeed] = useState(5);
  const [scale, setScale] = useState<IntensityScale>('sqrt');
  const [ladderRows, setLadderRows] = useState(10);

  // A new window starts paused at its beginning.
  useEffect(() => {
    setCursor(win.t0);
    setPlaying(false);
  }, [win]);

  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    let last = performance.now();
    const tick = (now: number) => {
      // A backgrounded tab pauses animation frames; do not jump the cursor by the whole gap.
      const elapsedMs = Math.min(100, now - last);
      last = now;
      setCursor((c) => Math.min(win.t1, c + elapsedMs * 1e6 * speed));
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, speed, win]);

  useEffect(() => {
    if (playing && cursor >= win.t1) setPlaying(false);
  }, [playing, cursor, win]);

  const placeCursor = useCallback((t: number) => {
    setPlaying(false);
    setCursor(t);
  }, []);

  const frameIdx = frameIndexAt(win.depth, cursor);
  const frame = frameIdx >= 0 ? win.depth[frameIdx] : null;
  const bboIdx = seriesIndexAt(win.bbo.ts, cursor);
  const bidTick = bboIdx >= 0 ? win.bbo.bid_tick[bboIdx] : null;
  const askTick = bboIdx >= 0 ? win.bbo.ask_tick[bboIdx] : null;
  const hasAnything = win.depth.length > 0 || win.bbo.ts.length > 0 || win.trades.ts.length > 0;

  return (
    <div className="col">
      {win.bbo_downsampled ? (
        <p className="notice" role="note">
          <Badge kind="warn">downsampled</Badge> The best bid/ask lines are downsampled to one entry
          per {fmtDuration(win.step_ns)}: each entry is the top of book at the end of its step, so
          changes within a step are not drawn. Load a shorter window to see every change.
        </p>
      ) : null}
      {win.trades_truncated ? (
        <p className="notice" role="note">
          <Badge kind="warn">trades truncated</Badge> This window held more than{' '}
          {fmtInt(MAX_TRADES)} trades; only the {fmtInt(win.trades.ts.length)} largest by quantity
          are drawn. Smaller trades in this window are missing from the view. Load a shorter window
          to see all of them.
        </p>
      ) : null}

      {!hasAnything ? (
        <Empty>
          This window contains no depth frames, top-of-book entries or trades. Choose a different
          start or a longer window.
        </Empty>
      ) : (
        <div className="replay-layout">
          <Panel
            title={
              <>
                Order book heatmap <Badge kind="observed">observed L2</Badge>
              </>
            }
            actions={
              <>
                <label htmlFor="replay-scale">Brightness</label>
                <select
                  id="replay-scale"
                  value={scale}
                  onChange={(e) => setScale(e.target.value === 'log' ? 'log' : 'sqrt')}
                >
                  <option value="sqrt">square-root scale</option>
                  <option value="log">log scale</option>
                </select>
              </>
            }
          >
            <div className="col">
              <p className="note">
                Reconstructed from Level-2 Market-By-Price updates: each cell is the displayed
                aggregate quantity at one price in one depth frame ({fmtInt(win.depth.length)}{' '}
                frames, one per {fmtDuration(win.step_ns)}; a frame holds until the next). Individual
                orders, hidden liquidity and queue positions are not visible in this data.
              </p>
              <Playback
                win={win}
                cursor={cursor}
                playing={playing}
                speed={speed}
                onCursor={setCursor}
                onPlaying={setPlaying}
                onSpeed={setSpeed}
              />
              <HeatmapView
                win={win}
                instrument={inst}
                scale={scale}
                cursor={cursor}
                follow={playing}
                onCursor={placeCursor}
              />
            </div>
          </Panel>

          <Panel
            title="Depth ladder at cursor"
            actions={
              <>
                <label htmlFor="ladder-rows">Levels</label>
                <select
                  id="ladder-rows"
                  value={ladderRows}
                  onChange={(e) => setLadderRows(Number(e.target.value))}
                >
                  {LADDER_ROWS.map((n) => (
                    <option key={n} value={n}>
                      top {n}
                    </option>
                  ))}
                </select>
              </>
            }
          >
            <div className="col">
              <dl className="kv">
                <dt>Cursor</dt>
                <dd>
                  <ClockText ns={Math.round(cursor)} />
                </dd>
                <dt>Best bid</dt>
                <dd className="buy">
                  {bboIdx >= 0 && bidTick !== null && bidTick !== undefined
                    ? `${fmtPrice(bidTick, inst)} \u00d7 ${fmtQty(win.bbo.bid_qty[bboIdx], inst)}`
                    : DASH}
                </dd>
                <dt>Best ask</dt>
                <dd className="sell">
                  {bboIdx >= 0 && askTick !== null && askTick !== undefined
                    ? `${fmtPrice(askTick, inst)} \u00d7 ${fmtQty(win.bbo.ask_qty[bboIdx], inst)}`
                    : DASH}
                </dd>
              </dl>
              <p className="note">
                Top of book is read from the quote series at the cursor
                {win.bbo_downsampled ? ' (downsampled)' : ''}; the ladder below shows the most
                recent depth frame, so the two can differ within one frame step.
              </p>
              <DepthLadder frame={frame} instrument={inst} rows={ladderRows} />
            </div>
          </Panel>
        </div>
      )}
    </div>
  );
}

export default function ReplayPage() {
  const { datasetId = '' } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  usePageTitle(`Replay ${datasetId}`);

  const dataset = useAsync(() => api.dataset(datasetId), [datasetId]);
  const ds = dataset.data;

  const query = useMemo(() => (ds ? resolveQuery(ds, searchParams) : null), [ds, searchParams]);
  const t0 = query?.t0 ?? null;
  const t1 = query?.t1 ?? null;
  const levels = query?.levels ?? null;

  const replay = useAsync<ReplayWindow | null>(
    () =>
      t0 === null || t1 === null || levels === null
        ? Promise.resolve(null)
        : api.replay(datasetId, { t0, t1, levels, max_trades: MAX_TRADES }),
    [datasetId, t0, t1, levels],
  );

  const apply = useCallback(
    (nextT0: number, lengthNs: number, nextLevels: number) => {
      const next = new URLSearchParams();
      if (lengthNs !== 0) next.set('t0', String(nextT0));
      next.set('len', String(lengthNs));
      next.set('levels', String(nextLevels));
      setSearchParams(next);
    },
    [setSearchParams],
  );

  const win = replay.data;

  return (
    <div className="col">
      <PageHeader
        title={
          <>
            Market replay <span className="mono dim">{datasetId}</span>
          </>
        }
        lead="The historical book as the data shows it, before any strategy is simulated. Times are on the exchange clock, relative to the dataset origin."
        actions={
          <>
            <Link className="btn" to="/datasets">
              All datasets
            </Link>
            <Link className="btn btn-primary" to={`/lab?dataset=${encodeURIComponent(datasetId)}`}>
              Open in Strategy Lab
            </Link>
          </>
        }
      />

      {dataset.error ? <ErrorBox error={dataset.error} /> : null}
      {dataset.loading && !ds ? <Spinner label="Loading dataset manifest" /> : null}

      {ds && query ? (
        <>
          <Panel
            title={
              <>
                <span>{ds.title}</span>
                {ds.kind === 'synthetic' ? (
                  <Badge
                    kind="warn"
                    title="Generated by BlackBox from a seed. This is not market data."
                  >
                    synthetic {'\u2014'} generated, not market data
                  </Badge>
                ) : (
                  <Badge kind="observed">recorded market data</Badge>
                )}
              </>
            }
          >
            <ReplayControls ds={ds} query={query} onApply={apply} busy={replay.loading} />
          </Panel>

          {replay.error ? <ErrorBox error={replay.error} /> : null}
          {replay.loading && !win ? <Spinner label="Reconstructing book for the window" /> : null}
          {win ? (
            <>
              <p className="mono dim small">
                window <ClockText ns={win.t0} /> {'\u2192'} <ClockText ns={win.t1} /> ·{' '}
                {fmtInt(win.depth.length)} depth frames · {fmtInt(win.bbo.ts.length)} top-of-book
                entries · {fmtInt(win.trades.ts.length)} trades · top {win.levels} levels per side
              </p>
              <ReplayWorkspace ds={ds} win={win} />
            </>
          ) : null}
        </>
      ) : null}
    </div>
  );
}
