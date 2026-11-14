// The public latency experiment, the console's landing page (`/`): "What changes when a
// market-making strategy receives and executes orders more slowly?" The stored answer comes
// first, then the synchronized replay (chart + book at one shared time), the order lifecycle in
// both scenarios, the orders of the replay window and a four-step guide. All view state (step,
// scenario filter, order, replay time) lives in the URL query, so leaving for the forensics page
// and coming back restores the same view; the page's scroll position is restored too.

import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode, type RefObject } from 'react';
import { Link, useLocation, useSearchParams } from 'react-router-dom';

import { Spinner, useAsync } from '../../components/ui';
import { useAnnounce } from '../../shell/announce';
import { paths } from '../../shell/paths';
import { useSettings } from '../../shell/settings';
import { useDocumentTitle } from '../../shell/Workspace';
import { divergences, loadMoment, momentWindow, StoryUnavailable, type Moment, type MomentWindow } from './story';
import {
  answerSummary,
  fmtSec,
  loadExperiment,
  loadOrderById,
  pairClassText,
  PairIndex,
  scenarioName,
  shows,
  windowOrders,
  type Experiment,
  type Focus,
  type WindowOrder,
} from './data';
import { LifecyclePanel } from './LifecyclePanel';
import { PlainError } from './PlainError';
import { ReplayPanel } from './ReplayPanel';
import { StepGuide, STEPS } from './StepGuide';
import { WindowOrders } from './WindowOrders';
import './explore.css';

const QUESTION = 'What changes when a market-making strategy receives and executes orders more slowly?';
/** Replay window around the shared time (≤ 1 s keeps one window's market and timelines small). */
const BEFORE_NS = 250_000_000;
const AFTER_NS = 600_000_000;
const SCROLL_KEY = 'bb.explore.scroll.v1';

/** URL-backed view state. Invalid or missing values fall back to defaults. */
function useViewState() {
  const [params, setParams] = useSearchParams();
  const stepRaw = Number(params.get('step'));
  const step = Number.isInteger(stepRaw) && stepRaw >= 1 && stepRaw <= 4 ? stepRaw : 1;
  const sc = params.get('scenario');
  const focus: Focus = sc === 'fast' || sc === 'slow' ? sc : 'both';
  const orderRaw = params.get('order');
  const order = orderRaw !== null && /^\d{1,9}$/.test(orderRaw) ? Number(orderRaw) : null;
  const tRaw = params.get('t');
  const t = tRaw !== null && /^\d{1,15}$/.test(tRaw) ? Number(tRaw) : null;
  const update = useCallback(
    (patch: Record<string, string | number | null>) =>
      setParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          for (const [k, v] of Object.entries(patch)) {
            if (v === null) next.delete(k);
            else next.set(k, String(v));
          }
          return next;
        },
        { replace: true },
      ),
    [setParams],
  );
  return { step, focus, order, t, update };
}

export default function ExplorePage() {
  useDocumentTitle('Latency experiment');
  const exp = useAsync((signal) => loadExperiment(signal), []);
  const scroller = useRef<HTMLDivElement | null>(null);
  return (
    <div ref={scroller} className="ex">
      {exp.data ? (
        <Loaded exp={exp.data} scroller={scroller} />
      ) : (
        <>
          <Header />
          <div className="ex-state">
            {exp.error ? (
              exp.error instanceof StoryUnavailable ? (
                <div role="status">
                  <p>{exp.error.message}</p>
                  <button type="button" onClick={exp.reload}>
                    Check again
                  </button>
                </div>
              ) : (
                <PlainError error={exp.error} onRetry={exp.reload} what="The stored experiment" />
              )
            ) : (
              <Spinner label="Loading the stored experiment" />
            )}
          </div>
        </>
      )}
    </div>
  );
}

function Header({ answer, actions }: { answer?: ReactNode; actions?: ReactNode }) {
  return (
    <header className="ex-head">
      <div className="ex-head-top">
        <p className="ex-flags">
          <span className="ex-eyebrow">Latency experiment</span>
          <span className="ex-flag ex-flag-data">Synthetic data</span>
          <span className="ex-flag">Simulated historical replay</span>
          <span className="ex-flag">No market impact</span>
        </p>
        {actions ? <div className="ex-head-actions">{actions}</div> : null}
      </div>
      <h1 className="ex-question">{QUESTION}</h1>
      {answer}
    </header>
  );
}

interface Pending {
  dir: 'next' | 'prev';
  from: number;
}

function Loaded({ exp, scroller }: { exp: Experiment; scroller: RefObject<HTMLDivElement | null> }) {
  const { story } = exp;
  const inst = exp.dataset.instrument;
  const view = useViewState();
  const { update } = view;
  const announce = useAnnounce();
  const { settings } = useSettings();
  const location = useLocation();

  const names = useMemo(() => ({ fast: scenarioName(story.fast), slow: scenarioName(story.slow) }), [story]);
  const summary = useMemo(() => answerSummary(story), [story]);
  const { first, firstOutcome } = useMemo(() => divergences(story), [story]);

  // ----- one shared time; the replay window follows it -----
  const t = view.t ?? first?.decision_ts ?? 0;
  const [win, setWin] = useState<MomentWindow>(() => momentWindow(t, BEFORE_NS, AFTER_NS));
  const activeWin = t >= win.t0 && t <= win.t1 ? win : momentWindow(t, BEFORE_NS, AFTER_NS);
  useEffect(() => {
    if (activeWin !== win) setWin(activeWin);
  }, [activeWin, win]);
  const momentRes = useAsync((signal) => loadMoment(story, activeWin, signal), [story, activeWin.t0, activeWin.t1]);
  const lastMoment = useRef<(Moment & { complete: boolean }) | null>(null);
  if (momentRes.data) lastMoment.current = momentRes.data;
  const moment = momentRes.data ?? lastMoment.current;
  const pick = useCallback((ts: number) => update({ t: Math.max(0, Math.round(ts)) }), [update]);

  // ----- selected order (loaded only on selection) -----
  const order = useAsync(
    (signal) => (view.order === null ? Promise.resolve(null) : loadOrderById(story, view.order, signal)),
    [story, view.order],
  );
  const orderData = order.data;
  const tMissing = view.t === null;
  useEffect(() => {
    // An order chosen without a time (typed id): start the replay at its decision.
    if (!tMissing || !orderData || view.order === null) return;
    const d = orderData.fast ?? orderData.slow;
    if (d) update({ t: d.order.decision_ts });
  }, [orderData, tMissing, update, view.order]);
  const orderDecision = orderData ? ((orderData.fast ?? orderData.slow)?.order.decision_ts ?? null) : null;

  // ----- stored comparison classes, read only as far as a view needs them -----
  const pairIndex = useMemo(
    () => (story.comparisonId && story.comparison?.alignment ? new PairIndex(story.comparisonId, story.comparison.alignment.pairs_truncated) : null),
    [story],
  );
  const [classesWanted, setClassesWanted] = useState(false);
  const rows = useMemo(() => (momentRes.data ? windowOrders(momentRes.data) : []), [momentRes.data]);
  const windowHorizon = classesWanted && rows.length > 0 ? rows[rows.length - 1].decisionTs : null;
  const need = windowHorizon ?? -1;
  const pairsRes = useAsync((signal) => (pairIndex && need >= 0 ? pairIndex.coverUpTo(need, signal) : Promise.resolve()), [pairIndex, need]);
  const pairsReady = !pairsRes.loading && !pairsRes.error;
  const classText = useCallback(
    (r: WindowOrder) => {
      const p = pairIndex?.lookup(r.orderId, r.decisionTs) ?? null;
      return p === 'identical' ? 'Identical in both scenarios' : p ? pairClassText(story, p.class) : null;
    },
    // A new function once more pages are read, so the table re-renders with them.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [pairIndex, story, pairsReady],
  );
  // The verdict of the selected order: the divergence pairs are part of the stored comparison;
  // other orders' classes are known once the comparison's pages were read for the order list.
  const knownPair = [first, firstOutcome].find((p) => p !== null && p.order_id === view.order) ?? null;
  const selectedPair =
    knownPair ?? (view.order !== null && orderDecision !== null && pairIndex && pairsReady ? pairIndex.lookup(view.order, orderDecision) : null);

  // ----- focus and scroll after a jump -----
  const replayRef = useRef<HTMLElement | null>(null);
  const barRef = useRef<HTMLElement | null>(null);
  const lifeHeading = useRef<HTMLHeadingElement | null>(null);
  const [revealReq, setRevealReq] = useState(0);
  useEffect(() => {
    if (revealReq === 0) return;
    const s = scroller.current;
    const heading = lifeHeading.current;
    const replay = replayRef.current;
    if (!s || !heading || !replay) return;
    heading.focus({ preventScroll: true });
    // Scroll the page's own scroller only (never the document): keep as much of the replay in view
    // as fits while the lifecycle heading and its first rows are visible.
    const box = s.getBoundingClientRect();
    const top = (el: Element) => el.getBoundingClientRect().top - box.top + s.scrollTop;
    const bar = barRef.current && getComputedStyle(barRef.current).position === 'sticky' ? barRef.current.offsetHeight : 0;
    const want = Math.max(top(replay) - bar - 8, top(heading) - s.clientHeight + 260);
    s.scrollTo({ top: Math.max(0, Math.min(want, top(heading) - bar - 8)) });
  }, [revealReq, scroller]);

  const selectOrder = useCallback(
    (orderId: number, ts: number | null, how: 'jump' | 'quiet') => {
      update({ order: orderId, t: ts, step: 3 });
      announce(`Order ${orderId} selected${ts !== null ? `; the replay moved to ${fmtSec(ts, 3)}` : ''}. Its lifecycle in both scenarios is shown.`);
      if (how === 'jump') setRevealReq((n) => n + 1);
    },
    [update, announce],
  );
  const selectRow = useCallback((r: WindowOrder) => selectOrder(r.orderId, r.decisionTs, 'jump'), [selectOrder]);
  const selectFromReplay = useCallback(
    (orderId: number) => {
      const r = rows.find((x) => x.orderId === orderId);
      selectOrder(orderId, r ? r.decisionTs : null, 'quiet');
    },
    [rows, selectOrder],
  );

  // ----- previous / next order across the window edges -----
  const navOrders = useMemo(() => rows.filter((o) => (shows(view.focus, 'fast') && o.fast) || (shows(view.focus, 'slow') && o.slow)), [rows, view.focus]);
  const [pending, setPending] = useState<Pending | null>(null);
  useEffect(() => {
    if (!pending || !momentRes.data || momentRes.data.window.t0 !== activeWin.t0) return;
    const found = pending.dir === 'next' ? navOrders.find((o) => o.decisionTs > pending.from) : [...navOrders].reverse().find((o) => o.decisionTs < pending.from);
    setPending(null);
    if (found) selectOrder(found.orderId, found.decisionTs, 'quiet');
    else announce(`No ${pending.dir === 'next' ? 'later' : 'earlier'} order in this part of the run.`);
  }, [pending, momentRes.data, navOrders, activeWin.t0, selectOrder, announce]);
  const stepOrder = (dir: 'next' | 'prev') => {
    if (!momentRes.data) return;
    const idx = view.order === null ? -1 : navOrders.findIndex((o) => o.orderId === view.order);
    const candidate =
      idx < 0
        ? dir === 'next'
          ? navOrders.find((o) => o.decisionTs >= t)
          : [...navOrders].reverse().find((o) => o.decisionTs <= t)
        : navOrders[dir === 'next' ? idx + 1 : idx - 1];
    if (candidate) {
      selectOrder(candidate.orderId, candidate.decisionTs, 'quiet');
      return;
    }
    const from = idx >= 0 ? navOrders[idx].decisionTs : t;
    if (dir === 'prev' && activeWin.t0 <= 0) {
      announce('This is the first order of the run.');
      return;
    }
    setPending({ dir, from });
    pick(dir === 'next' ? activeWin.t1 + BEFORE_NS : Math.max(0, activeWin.t0 - AFTER_NS - 1));
  };

  // ----- steps -----
  const stepButtons = useRef<(HTMLButtonElement | null)[]>([]);
  const goStep = useCallback(
    (n: number, focusButton = false) => {
      const s = Math.min(4, Math.max(1, n));
      update({ step: s });
      announce(`Step ${s} of 4: ${STEPS[s - 1].title}.`);
      if (focusButton) stepButtons.current[s - 1]?.focus();
    },
    [update, announce],
  );
  useEffect(() => {
    if (!settings.singleKeys) return;
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey || e.defaultPrevented) return;
      const el = e.target as HTMLElement | null;
      if (el && (el.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName))) return;
      const inSteps = stepButtons.current.some((b) => b === el);
      if (e.key === '[') goStep(view.step - 1, inSteps);
      else if (e.key === ']') goStep(view.step + 1, inSteps);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [goStep, view.step, settings.singleKeys]);
  const onStepsKey = (e: KeyboardEvent<HTMLOListElement>) => {
    const moves: Record<string, number> = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 };
    if (e.key in moves) goStep(view.step + moves[e.key], true);
    else if (e.key === 'Home') goStep(1, true);
    else if (e.key === 'End') goStep(4, true);
    else return;
    e.preventDefault();
  };

  // ----- scroll position survives a visit to the forensics page and back -----
  useEffect(() => {
    const s = scroller.current;
    if (!s) return;
    let frame = 0;
    const save = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => sessionStorage.setItem(SCROLL_KEY, JSON.stringify({ search: window.location.search, top: s.scrollTop })));
    };
    s.addEventListener('scroll', save, { passive: true });
    return () => {
      cancelAnimationFrame(frame);
      s.removeEventListener('scroll', save);
    };
  }, [scroller]);
  const restored = useRef(false);
  const contentReady = momentRes.data !== null && (view.order === null || order.data !== null);
  useEffect(() => {
    if (restored.current || !contentReady) return;
    restored.current = true;
    const s = scroller.current;
    try {
      const saved = JSON.parse(sessionStorage.getItem(SCROLL_KEY) ?? 'null') as { search: string; top: number } | null;
      if (s && saved && saved.search === location.search && saved.top > 0) s.scrollTo({ top: saved.top });
    } catch {
      // A malformed saved position is ignored.
    }
  }, [contentReady, location.search, scroller]);

  const runs = [story.fast.runId, story.slow.runId];
  const end = exp.dataset.end;
  const marks = useMemo(() => {
    const m = new Map<number, string>();
    if (firstOutcome) m.set(firstOutcome.order_id, 'first outcome divergence');
    if (first) m.set(first.order_id, 'first divergence');
    return m;
  }, [first, firstOutcome]);

  return (
    <>
      <Header
        answer={
          <div className="ex-answer">
            <p>
              <strong>{summary.headline}</strong> {summary.gap}
            </p>
            <p className="ex-muted">
              A smaller loss is still a loss. This is one strategy on one synthetic dataset, replayed without market impact; it does not show that
              slower (or faster) execution is better in general.
            </p>
          </div>
        }
        actions={
          <>
            <Link className="btn" to={paths.comparePair(story.fast.runId, story.slow.runId)}>
              Stored comparison
            </Link>
            <Link className="btn" to={paths.compareSet({ runs, demo: story.demo.id })}>
              Detailed research mode
            </Link>
          </>
        }
      />

      <nav ref={barRef} className="ex-bar" aria-label="Replay controls">
        <div className="ex-group" role="group" aria-label="Scenarios shown">
          <span className="ex-group-label">Scenario</span>
          {(['both', 'fast', 'slow'] as const).map((k) => (
            <button
              key={k}
              type="button"
              className="ex-seg"
              aria-pressed={view.focus === k}
              onClick={() => {
                update({ scenario: k === 'both' ? null : k });
                announce(k === 'both' ? 'Showing both scenarios.' : `Showing only the ${names[k]} scenario.`);
              }}
            >
              {k === 'both' ? (
                'Both'
              ) : (
                <>
                  <span className={`ex-swatch ex-swatch-${k}`} aria-hidden="true" /> {names[k]}
                </>
              )}
            </button>
          ))}
        </div>
        <div className="ex-group" role="group" aria-label="Jump to a divergence">
          <button type="button" className="btn-primary" disabled={!first} onClick={() => first && selectOrder(first.order_id, first.decision_ts, 'jump')} title="The earliest order whose timing or outcome differs">
            First divergence{first ? ` · ${fmtSec(first.decision_ts, 3)}` : ''}
          </button>
          <button
            type="button"
            disabled={!firstOutcome}
            onClick={() => firstOutcome && selectOrder(firstOutcome.order_id, firstOutcome.decision_ts, 'jump')}
            title="The earliest order whose decision or fills differ"
          >
            First different outcome{firstOutcome ? ` · ${fmtSec(firstOutcome.decision_ts, 3)}` : ''}
          </button>
        </div>
        <div className="ex-group" role="group" aria-label="Step through orders">
          <button type="button" onClick={() => stepOrder('prev')} disabled={!momentRes.data}>
            ← Previous order
          </button>
          <button type="button" onClick={() => stepOrder('next')} disabled={!momentRes.data}>
            Next order →
          </button>
        </div>
        <Seek t={t} end={end} stepNs={story.fast.run.config.strategy.interval_ns} onCommit={pick} />
      </nav>

      <div className="ex-stage">
        <ReplayPanel
          inst={inst}
          names={names}
          moment={moment}
          loading={momentRes.loading}
          error={momentRes.error}
          onRetry={momentRes.reload}
          t={t}
          focus={view.focus}
          selected={view.order}
          onSelectOrder={selectFromReplay}
          onPick={pick}
          chartRef={replayRef}
          between={
        <section className="ex-life-sec ex-panel" aria-labelledby="ex-life-title">
          <h2 id="ex-life-title" ref={lifeHeading} tabIndex={-1}>
            {view.order === null ? 'Order lifecycle' : `Order ${view.order} in both scenarios`}
          </h2>
          {view.order === null ? (
            <div className="ex-empty">
              <p>No order selected yet. Start with the first order whose handling differs between the scenarios, or pick any order in the replay.</p>
              {first ? (
                <button type="button" className="btn-primary ex-cta" onClick={() => selectOrder(first.order_id, first.decision_ts, 'jump')}>
                  Show the first divergence (order {first.order_id})
                </button>
              ) : (
                <p>The stored comparison has no divergence.</p>
              )}
            </div>
          ) : order.error ? (
            <PlainError error={order.error} onRetry={order.reload} what={`Order ${view.order}`} />
          ) : !order.data ? (
            <Spinner label={`Loading order ${view.order} in both scenarios`} />
          ) : (
            <LifecyclePanel story={story} orderId={view.order} details={order.data} pair={selectedPair} inst={inst} names={names} focus={view.focus} onPick={pick} />
          )}
        </section>
          }
        />

        <section className="ex-orders-sec ex-panel" aria-labelledby="ex-orders-title">
          <div className="ex-panel-head">
            <h2 id="ex-orders-title">Orders in this window</h2>
            {pairIndex && !classesWanted ? (
              <button type="button" className="btn-link" onClick={() => setClassesWanted(true)}>
                Show how each order compares
              </button>
            ) : null}
            {classesWanted && pairsRes.loading ? <Spinner label="Reading the stored comparison" /> : null}
          </div>
          {pairsRes.error ? <PlainError error={pairsRes.error} onRetry={pairsRes.reload} what="Comparison classes" /> : null}
          {momentRes.data ? (
            <WindowOrders
              rows={rows}
              focus={view.focus}
              names={names}
              inst={inst}
              classText={classesWanted ? classText : null}
              marks={marks}
              selected={view.order}
              onSelectOrder={selectRow}
            />
          ) : (
            <p className="ex-muted">Waiting for the replay window.</p>
          )}
        </section>
      </div>

      <section className="ex-guide ex-panel" aria-labelledby="ex-guide-title">
        <h2 id="ex-guide-title" className="visually-hidden">
          Guide
        </h2>
        <nav aria-label="Guided steps">
          <ol className="ex-steps" onKeyDown={onStepsKey}>
            {STEPS.map((s, i) => (
              <li key={s.n}>
                <button
                  ref={(el) => {
                    stepButtons.current[i] = el;
                  }}
                  type="button"
                  className="ex-step"
                  aria-current={view.step === s.n ? 'step' : undefined}
                  tabIndex={view.step === s.n ? 0 : -1}
                  onClick={() => goStep(s.n)}
                >
                  <span className="ex-step-n" aria-hidden="true">
                    {s.n}
                  </span>
                  <span>
                    <span className="ex-step-title">{s.title}</span>
                    <span className="ex-step-hint">{s.hint}</span>
                  </span>
                </button>
              </li>
            ))}
          </ol>
          {settings.singleKeys ? (
            <p className="ex-kbd-hint">
              Arrow keys move between steps; <kbd>[</kbd> and <kbd>]</kbd> work anywhere on the page.
            </p>
          ) : null}
        </nav>
        <div className="ex-guide-body">
          <h3 className="ex-guide-title">
            <span className="ex-muted">Step {view.step} of 4</span> {STEPS[view.step - 1].title}
          </h3>
          <StepGuide
            exp={exp}
            step={view.step}
            summary={summary}
            names={names}
            focus={view.focus}
            t={t}
            orderId={view.order}
            order={order}
            onSelectOrder={(id, ts) => selectOrder(id, ts, 'jump')}
            onPick={pick}
          />
          <div className="ex-guide-nav">
            {view.step > 1 ? (
              <button type="button" onClick={() => goStep(view.step - 1)}>
                &larr; {STEPS[view.step - 2].title}
              </button>
            ) : null}
            {view.step < 4 ? (
              <button type="button" className="btn-primary" onClick={() => goStep(view.step + 1)}>
                Next: {STEPS[view.step].title} &rarr;
              </button>
            ) : null}
          </div>
        </div>
      </section>

      <footer className="ex-foot">
        Simulated historical replay of a synthetic dataset. Simulated orders do not affect the observed market; results do not predict live
        performance, and a smaller loss is not a profit. <Link to={paths.overview()}>All experiments</Link>
      </footer>
    </>
  );
}

/** Whole-run time slider; commits after the thumb rests so dragging does not load every window. */
function Seek({ t, end, stepNs, onCommit }: { t: number; end: number; stepNs: number; onCommit: (t: number) => void }) {
  const [draft, setDraft] = useState<number | null>(null);
  useEffect(() => {
    if (draft === null) return;
    const id = window.setTimeout(() => {
      onCommit(draft);
      setDraft(null);
    }, 350);
    return () => window.clearTimeout(id);
  }, [draft, onCommit]);
  const shown = draft ?? t;
  return (
    <div className="ex-group ex-seek">
      <label htmlFor="ex-seek">Replay time</label>
      <input
        id="ex-seek"
        type="range"
        min={0}
        max={end}
        step={stepNs}
        value={Math.min(end, Math.max(0, shown))}
        aria-valuetext={`${(shown / 1e9).toFixed(1)} seconds of ${(end / 1e9).toFixed(1)}`}
        onChange={(e) => setDraft(Number(e.target.value))}
      />
      <output htmlFor="ex-seek" className="ex-num">
        {(shown / 1e9).toFixed(1)} / {(end / 1e9).toFixed(1)} s
      </output>
    </div>
  );
}
