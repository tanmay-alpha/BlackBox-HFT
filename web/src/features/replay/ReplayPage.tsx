// Order Book Lab, market only (UI_DESIGN_SPEC.md §11.3): the reconstructed book of one dataset as the
// data shows it, before any strategy is simulated. Toolbar: window, playback, brightness and the
// `Market only | With run` switch; body: heatmap | depth ladder.

import { useCallback, useEffect, useMemo, useState } from 'react';
import type { FormEvent } from 'react';
import { Link, useParams, useSearchParams } from 'react-router-dom';

import { api } from '../../api/client';
import type { DatasetManifest, ReplayWindow } from '../../api/types';
import { DASH, fmtDuration, fmtInt, fmtPrice, fmtQty } from '../../components/fmt';
import { digitsForUnit } from '../../components/ns';
import { SplitPane } from '../../components/SplitPane';
import { ToolbarDivider } from '../../components/Toolbar';
import {
  Badge,
  ClockText,
  Empty,
  ErrorBox,
  HashText,
  Notice,
  Panel,
  Spinner,
  TimeDigitsContext,
  useAsync,
} from '../../components/ui';
import { useInspector } from '../../shell/inspector';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import { BookModeMarket } from './BookMode';
import { DepthLadder } from './DepthLadder';
import { frameIndexAt, seriesIndexAt } from './heatmap';
import type { HeatmapStats, IntensityScale } from './heatmap';
import { HeatmapLegend, HeatmapView } from './HeatmapView';
import './replay.css';
import { parseSeconds, secondsText } from './seconds';

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

const LENGTH_HELP = 'One depth frame per 1/300 of it (at least 1 ms).';
const LEVELS_HELP = 'Levels beyond this are not fetched.';

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

function WindowControls({
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

  const startNs = parseSeconds(start);
  const startValid = startNs !== null && startNs >= ds.start && startNs <= ds.end;
  const spanText = `Dataset spans ${secondsText(ds.start)} to ${secondsText(ds.end)} s.`;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!startValid || startNs === null) return;
    onApply(startNs, lengthNs, levels);
  };

  const whole = query.lengthNs === 0;
  const span = query.t1 - query.t0;

  return (
    <>
      <form className="row" onSubmit={submit} noValidate aria-label="Replay window">
        <label className="book-field" htmlFor="replay-start" title={spanText}>
          Window start <span className="unit">s after origin</span>
        </label>
        <input
          id="replay-start"
          type="text"
          inputMode="decimal"
          value={start}
          onChange={(e) => setStart(e.target.value)}
          disabled={lengthNs === 0}
          aria-invalid={!startValid}
          aria-describedby="replay-start-help"
          title={spanText}
        />
        <span id="replay-start-help" className={startValid ? 'visually-hidden' : 'field-error small'} role={startValid ? undefined : 'alert'}>
          {spanText}
        </span>
        <label className="book-field" htmlFor="replay-length" title={LENGTH_HELP}>
          Window length
        </label>
        <select id="replay-length" value={lengthNs} title={LENGTH_HELP} onChange={(e) => setLengthNs(Number(e.target.value))}>
          {LENGTHS.map((l) => (
            <option key={l.ns} value={l.ns}>
              {l.label}
            </option>
          ))}
        </select>
        <label className="book-field" htmlFor="replay-levels" title={LEVELS_HELP}>
          Depth levels per side
        </label>
        <select id="replay-levels" value={levels} title={LEVELS_HELP} onChange={(e) => setLevels(Number(e.target.value))}>
          {(LEVEL_CHOICES.includes(levels) ? LEVEL_CHOICES : [...LEVEL_CHOICES, levels]).map((n) => (
            <option key={n} value={n}>
              {n}
            </option>
          ))}
        </select>
        <button type="submit" disabled={!startValid || busy}>
          Load window
        </button>
      </form>
      <button
        type="button"
        disabled={busy || whole || query.t0 <= ds.start}
        onClick={() => onApply(Math.max(ds.start, query.t0 - span), query.lengthNs, query.levels)}
      >
        Previous window
      </button>
      <button type="button" disabled={busy || whole || query.t1 >= ds.end} onClick={() => onApply(query.t1, query.lengthNs, query.levels)}>
        Next window
      </button>
    </>
  );
}

function PlaybackControls({
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
    <>
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
      <label className="book-field" htmlFor="replay-speed">
        Speed
      </label>
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
        className="book-scrub"
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
      <span className="book-cursor">
        cursor <ClockText ns={Math.round(cursor)} />
      </span>
    </>
  );
}

export default function ReplayPage() {
  const { datasetId = '' } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();

  const dataset = useAsync((signal) => api.dataset(datasetId, { signal }), [datasetId]);
  const runs = useAsync((signal) => api.runs({ dataset_id: datasetId }, { signal }), [datasetId]);
  const version = dataset.data?.current_version ?? null;
  const ds = version?.manifest ?? null;

  const query = useMemo(() => (ds ? resolveQuery(ds, searchParams) : null), [ds, searchParams]);
  const t0 = query?.t0 ?? null;
  const t1 = query?.t1 ?? null;
  const levels = query?.levels ?? null;

  const replay = useAsync<ReplayWindow | null>(
    (signal) =>
      t0 === null || t1 === null || levels === null
        ? Promise.resolve(null)
        : api.replay(datasetId, { t0, t1, levels, max_trades: MAX_TRADES }, { signal }),
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
  const inst = ds?.instrument ?? null;
  const [cursor, setCursor] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [speed, setSpeed] = useState(5);
  const [scale, setScale] = useState<IntensityScale>('sqrt');
  const [ladderRows, setLadderRows] = useState(10);
  const [stats, setStats] = useState<HeatmapStats | null>(null);

  // A new window starts paused at its beginning.
  useEffect(() => {
    if (!win) return;
    setCursor(win.t0);
    setPlaying(false);
  }, [win]);

  useEffect(() => {
    if (!playing || !win) return;
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
    if (playing && win && cursor >= win.t1) setPlaying(false);
  }, [playing, cursor, win]);

  const placeCursor = useCallback((t: number) => {
    setPlaying(false);
    setCursor(t);
  }, []);

  useInspector(
    'Market replay',
    <div className="col prose">
      <p>
        The book reconstructed from the dataset&apos;s events, before any strategy is simulated. Times are on the exchange clock,
        relative to the dataset origin.
      </p>
      {win ? (
        <p>
          Reconstructed from Level-2 Market-By-Price updates: each cell is the displayed aggregate quantity at one price
          in one depth frame ({fmtInt(win.depth.length)} frames, one per {fmtDuration(win.step_ns)}; a frame holds until
          the next). Individual orders, hidden liquidity and queue positions are not visible in this data.
        </p>
      ) : null}
      <p className="dim">
        Window length: {LENGTH_HELP} Depth levels per side: {LEVELS_HELP}
      </p>
      {win && inst && stats ? <HeatmapLegend stats={stats} scale={scale} inst={inst} levels={win.levels} /> : null}
    </div>,
    [win, inst, stats, scale],
  );

  const frameIdx = win ? frameIndexAt(win.depth, cursor) : -1;
  const frame = win && frameIdx >= 0 ? win.depth[frameIdx] : null;
  const bboIdx = win ? seriesIndexAt(win.bbo.ts, cursor) : -1;
  const bidTick = win && bboIdx >= 0 ? win.bbo.bid_tick[bboIdx] : null;
  const askTick = win && bboIdx >= 0 ? win.bbo.ask_tick[bboIdx] : null;
  const hasAnything = win ? win.depth.length > 0 || win.bbo.ts.length > 0 || win.trades.ts.length > 0 : false;

  const toolbar = (
    <>
      {ds && query ? <WindowControls ds={ds} query={query} onApply={apply} busy={replay.loading} /> : null}
      {win ? (
        <>
          <ToolbarDivider />
          <PlaybackControls
            win={win}
            cursor={cursor}
            playing={playing}
            speed={speed}
            onCursor={setCursor}
            onPlaying={setPlaying}
            onSpeed={setSpeed}
          />
          <ToolbarDivider />
          <label className="book-field" htmlFor="replay-scale">
            Brightness
          </label>
          <select id="replay-scale" value={scale} onChange={(e) => setScale(e.target.value === 'log' ? 'log' : 'sqrt')}>
            <option value="sqrt">square-root scale</option>
            <option value="log">log scale</option>
          </select>
        </>
      ) : null}
      <ToolbarDivider />
      <BookModeMarket datasetId={datasetId} runs={runs} cursor={win ? cursor : null} />
      <Link className="btn" to={paths.datasets()}>
        All datasets
      </Link>
      <Link className="btn" to={paths.strategy({ dataset: datasetId })}>
        Open in Strategy Lab
      </Link>
    </>
  );

  let body;
  if (dataset.error) {
    body = (
      <div className="book-state">
        <ErrorBox error={dataset.error} onRetry={dataset.reload} />
      </div>
    );
  } else if (dataset.data && !version) {
    body = (
      <div className="book-state">
        <Empty>Dataset {datasetId} has no current version to replay.</Empty>
      </div>
    );
  } else if (!ds || !inst || !version) {
    body = (
      <div className="book-state">
        <Spinner label="Loading dataset manifest" />
      </div>
    );
  } else if (replay.error) {
    body = (
      <div className="book-state">
        <ErrorBox error={replay.error} onRetry={replay.reload} />
      </div>
    );
  } else if (!win) {
    body = (
      <div className="book-state">
        <Spinner label="Reconstructing book for the window" />
      </div>
    );
  } else {
    body = (
      <div className="book-body">
        <p className="book-window">
          {ds.kind === 'synthetic' ? (
            <Badge kind="warn" title="Generated by BlackBox from a seed. This is not market data.">
              synthetic {'\u2014'} generated, not market data
            </Badge>
          ) : (
            <Badge kind="observed">recorded market data</Badge>
          )}{' '}
          {ds.title} · v{version.version} · sha256 <HashText hash={version.events_sha256} chars={10} /> · window <ClockText ns={win.t0} /> {'\u2192'} <ClockText ns={win.t1} /> · {fmtInt(win.depth.length)}{' '}
          depth frames · {fmtInt(win.bbo.ts.length)} top-of-book entries · {fmtInt(win.trades.ts.length)} trades · top{' '}
          {win.levels} levels per side
        </p>
        {win.bbo_downsampled ? (
          <Notice tag="DOWNSAMPLED">
            The best bid/ask lines are downsampled to one entry per {fmtDuration(win.step_ns)}: each entry is the top of
            book at the end of its step, so changes within a step are not drawn. Load a shorter window to see every change.
          </Notice>
        ) : null}
        {win.trades_truncated ? (
          <Notice tag="TRUNCATED">
            This window held more than {fmtInt(MAX_TRADES)} trades; only the {fmtInt(win.trades.ts.length)} largest by
            quantity are drawn. Smaller trades in this window are missing from the view. Load a shorter window to see all of
            them.
          </Notice>
        ) : null}
        {!hasAnything ? (
          <div className="book-state">
            <Empty>
              This window contains no depth frames, top-of-book entries or trades. Choose a different start or a longer
              window.
            </Empty>
          </div>
        ) : (
          <div className="book-panes">
            <SplitPane
              id="book.market"
              panes={[
                {
                  id: 'heatmap',
                  label: 'Order book heatmap',
                  defaultSize: '72%',
                  minSize: '40%',
                  stackHeight: 'max(280px, 55dvh)',
                  content: (
                    <Panel
                      title="Order book heatmap"
                      basis="observed"
                      fill
                      flush
                      caption={
                        <>
                          Reconstructed L2 · {fmtInt(win.depth.length)} frames, one per {fmtDuration(win.step_ns)}
                          {win.bbo_downsampled ? ` · best bid/ask downsampled 1:${fmtDuration(win.step_ns)}` : ''} · legend
                          and method in the inspector
                        </>
                      }
                    >
                      <HeatmapView
                        win={win}
                        instrument={inst}
                        scale={scale}
                        cursor={cursor}
                        follow={playing}
                        onCursor={placeCursor}
                        onStats={setStats}
                      />
                    </Panel>
                  ),
                },
                {
                  id: 'ladder',
                  label: 'Depth ladder',
                  defaultSize: '28%',
                  minSize: 260,
                  content: (
                    <Panel
                      title="Depth ladder at cursor"
                      basis="observed"
                      fill
                      actions={
                        <>
                          <label htmlFor="ladder-rows" className="book-field">
                            Levels
                          </label>
                          <select id="ladder-rows" value={ladderRows} onChange={(e) => setLadderRows(Number(e.target.value))}>
                            {LADDER_ROWS.map((n) => (
                              <option key={n} value={n}>
                                top {n}
                              </option>
                            ))}
                          </select>
                        </>
                      }
                    >
                      <dl className="kv ladder-bbo">
                        <dt>Cursor</dt>
                        <dd>
                          <ClockText ns={Math.round(cursor)} />
                        </dd>
                        <dt>Best bid</dt>
                        <dd className="bid">
                          {bboIdx >= 0 && bidTick !== null && bidTick !== undefined
                            ? `${fmtPrice(bidTick, inst)} \u00d7 ${fmtQty(win.bbo.bid_qty[bboIdx], inst)}`
                            : DASH}
                        </dd>
                        <dt>Best ask</dt>
                        <dd className="ask">
                          {bboIdx >= 0 && askTick !== null && askTick !== undefined
                            ? `${fmtPrice(askTick, inst)} \u00d7 ${fmtQty(win.bbo.ask_qty[bboIdx], inst)}`
                            : DASH}
                        </dd>
                      </dl>
                      <p className="note">
                        Top of book is read from the quote series at the cursor
                        {win.bbo_downsampled ? ' (downsampled)' : ''}; the ladder below shows the most recent depth frame,
                        so the two can differ within one frame step.
                      </p>
                      <DepthLadder frame={frame} instrument={inst} rows={ladderRows} />
                    </Panel>
                  ),
                },
              ]}
            />
          </div>
        )}
      </div>
    );
  }

  return (
    <TimeDigitsContext.Provider value={ds ? digitsForUnit(ds.timestamps.input_unit) : 9}>
      <Workspace context={datasetId} layout toolbar={toolbar}>
        {body}
      </Workspace>
    </TimeDigitsContext.Provider>
  );
}
