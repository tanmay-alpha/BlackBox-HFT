// Prototype B of the public latency experiment: a research workspace. A question header with the
// stored answer, a guided four-step rail and resizable, synchronized panels (equity, replay, order
// lifecycle, step guide). All view state (step, scenario focus, order, replay time) lives in the
// URL query, so leaving for the forensics page and coming back restores the same view.

import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { Group, Panel, Separator, type Layout } from 'react-resizable-panels';

import { ErrorBox, Spinner, useAsync } from '../../../components/ui';
import { paths } from '../../../shell/paths';
import { divergences, momentWindow, StoryUnavailable, type MomentWindow } from '../story';
import { answerSummary, loadEquity, loadOrderById, loadWorkspace, scenarioName, type Focus, type ScenarioKey, type Workspace } from './data';
import { EquityPanel } from './EquityPanel';
import { LifecyclePanel } from './LifecyclePanel';
import { ReplayPanel } from './ReplayPanel';
import { StepGuide, STEPS } from './StepGuide';
import './workspace.css';

const QUESTION = 'What changes when a market-making strategy receives and executes orders more slowly?';
const WIDE_QUERY = '(min-width: 1100px)';
const LAYOUT_KEYS = { rows: 'bb.explore.b.rows.v1', top: 'bb.explore.b.top.v1', bottom: 'bb.explore.b.bottom.v1' } as const;
type LayoutId = keyof typeof LAYOUT_KEYS;

/** Panels each step points the reader at; the others stay visible but unemphasized. */
const STEP_PANELS: Record<number, string[]> = {
  1: ['guide', 'equity'],
  2: ['guide', 'equity', 'replay'],
  3: ['replay', 'lifecycle'],
  4: ['guide'],
};

function useWide(): boolean {
  const [wide, setWide] = useState(() => window.matchMedia(WIDE_QUERY).matches);
  useEffect(() => {
    const mq = window.matchMedia(WIDE_QUERY);
    const on = () => setWide(mq.matches);
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  }, []);
  return wide;
}

function savedLayout(id: LayoutId): Layout | undefined {
  try {
    const raw = localStorage.getItem(LAYOUT_KEYS[id]);
    return raw ? (JSON.parse(raw) as Layout) : undefined;
  } catch {
    return undefined;
  }
}

/** URL-backed view state. Invalid or missing values fall back to defaults. */
function useViewState() {
  const [params, setParams] = useSearchParams();
  const stepRaw = Number(params.get('step'));
  const step = Number.isInteger(stepRaw) && stepRaw >= 1 && stepRaw <= 4 ? stepRaw : 1;
  const sc = params.get('scenario');
  const focus: Focus = sc === 'fast' || sc === 'slow' ? sc : 'both';
  const orderRaw = params.get('order');
  const order = orderRaw !== null && /^\d+$/.test(orderRaw) ? Number(orderRaw) : null;
  const tRaw = params.get('t');
  const t = tRaw !== null && /^\d+$/.test(tRaw) ? Number(tRaw) : null;
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

export default function WorkspacePrototype() {
  const ws = useAsync((signal) => loadWorkspace(signal), []);
  return (
    <div className="pb-root">
      <a className="pb-skip" href="#pb-main">
        Skip to the workspace
      </a>
      {ws.data ? (
        <Loaded ws={ws.data} />
      ) : (
        <>
          <Header />
          <main id="pb-main" className="pb-state">
            {ws.error ? (
              ws.error instanceof StoryUnavailable ? (
                <div className="pb-empty" role="status">
                  <p>{ws.error.message}</p>
                  <button type="button" onClick={ws.reload}>
                    Check again
                  </button>
                </div>
              ) : (
                <ErrorBox error={ws.error} onRetry={ws.reload} />
              )
            ) : (
              <Spinner label="Loading the stored experiment" />
            )}
          </main>
        </>
      )}
    </div>
  );
}

function Header({ answer, actions }: { answer?: ReactNode; actions?: ReactNode }) {
  return (
    <header className="pb-header">
      <div className="pb-header-top">
        <p className="pb-eyebrow">
          BlackBox latency experiment
          <span className="pb-flag">Synthetic data</span>
          <span className="pb-flag">Simulated historical replay</span>
          <span className="pb-flag">No market impact</span>
        </p>
        {actions ? <div className="pb-header-actions">{actions}</div> : null}
      </div>
      <h1 className="pb-question">{QUESTION}</h1>
      {answer ? <p className="pb-answer">{answer}</p> : null}
    </header>
  );
}

function Loaded({ ws }: { ws: Workspace }) {
  const { story } = ws;
  const view = useViewState();
  const wide = useWide();
  const [layoutGen, setLayoutGen] = useState(0);
  const [announce, setAnnounce] = useState('');
  const stepButtons = useRef<(HTMLButtonElement | null)[]>([]);

  const names = useMemo(() => ({ fast: scenarioName(story.fast), slow: scenarioName(story.slow) }), [story]);
  const summary = useMemo(() => answerSummary(story), [story]);
  const { first } = divergences(story);
  const inst = ws.dataset.instrument;

  // ----- shared time and replay window -----
  const t = view.t ?? first?.decision_ts ?? 0;
  const [win, setWin] = useState<MomentWindow>(() => momentWindow(t));
  const activeWin = t >= win.t0 && t <= win.t1 ? win : momentWindow(t);
  useEffect(() => {
    if (activeWin !== win) setWin(activeWin);
  }, [activeWin, win]);

  const { update } = view;
  const pick = useCallback((ts: number) => update({ t: Math.max(0, Math.round(ts)) }), [update]);

  // ----- selected order -----
  const order = useAsync(
    (signal) => (view.order === null ? Promise.resolve(null) : loadOrderById(story, view.order, signal)),
    [story, view.order],
  );
  const orderData = order.data;
  const tMissing = view.t === null;
  useEffect(() => {
    // An order chosen without a time (typed id): start the replay at its decision.
    if (!tMissing || !orderData) return;
    const d = orderData.fast ?? orderData.slow;
    if (d) {
      setWin(momentWindow(d.order.decision_ts));
      update({ t: d.order.decision_ts });
    }
  }, [orderData, tMissing, update]);

  const selectOrder = (orderId: number, ts: number | null) => {
    if (ts !== null) setWin(momentWindow(ts));
    view.update({ order: orderId, t: ts, step: 3 });
    setAnnounce(`Order ${orderId} selected. Step 3 of 4: ${STEPS[2].title}. The replay moved to its decision time.`);
    if (!wide) document.getElementById('pb-panel-replay')?.scrollIntoView({ block: 'start' });
  };

  // ----- steps -----
  const goStep = useCallback(
    (n: number, focusButton = false) => {
      const s = Math.min(4, Math.max(1, n));
      update({ step: s });
      setAnnounce(`Step ${s} of 4: ${STEPS[s - 1].title}. ${STEPS[s - 1].hint}.`);
      if (focusButton) stepButtons.current[s - 1]?.focus();
    },
    [update],
  );
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const el = e.target as HTMLElement | null;
      if (el && (el.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName))) return;
      const inRail = stepButtons.current.some((b) => b === el);
      if (e.key === '[') goStep(view.step - 1, inRail);
      else if (e.key === ']') goStep(view.step + 1, inRail);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [goStep, view.step]);
  const onRailKey = (e: KeyboardEvent<HTMLOListElement>) => {
    const moves: Record<string, number> = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 };
    if (e.key in moves) goStep(view.step + moves[e.key], true);
    else if (e.key === 'Home') goStep(1, true);
    else if (e.key === 'End') goStep(4, true);
    else return;
    e.preventDefault();
  };

  const resetLayout = () => {
    for (const k of Object.values(LAYOUT_KEYS)) localStorage.removeItem(k);
    setLayoutGen((g) => g + 1);
    setAnnounce('Panel layout reset.');
  };

  const equity = useAsync((signal) => loadEquity(story, signal), [story]);
  const runs = [story.fast.runId, story.slow.runId];
  const emphasized = STEP_PANELS[view.step];

  const panel = (id: string, title: ReactNode, body: ReactNode, actions?: ReactNode) => (
    <section id={`pb-panel-${id}`} className="pb-panel" data-emph={emphasized.includes(id) ? '' : undefined} aria-labelledby={`pb-h-${id}`}>
      <header className="pb-panel-head">
        <h2 id={`pb-h-${id}`}>{title}</h2>
        {actions ? <div className="pb-panel-actions">{actions}</div> : null}
      </header>
      <div className="pb-panel-body">{body}</div>
    </section>
  );

  const guidePanel = panel(
    'guide',
    'Guide',
    <StepGuide
      ws={ws}
      step={view.step}
      summary={summary}
      names={names}
      focus={view.focus}
      orderId={view.order}
      order={order}
      onSelectOrder={selectOrder}
      onPick={pick}
      onStep={(n) => goStep(n)}
    />,
  );
  const equityPanel = panel(
    'equity',
    'Equity over the whole run',
    equity.error ? (
      <ErrorBox error={equity.error} onRetry={equity.reload} />
    ) : equity.data ? (
      <EquityPanel pages={equity.data} names={names} focus={view.focus} t={t} divergenceTs={first?.decision_ts ?? null} onPick={pick} />
    ) : (
      <Spinner label="Loading equity" />
    ),
  );
  const replayPanel = panel(
    'replay',
    'Market replay',
    <ReplayPanel story={story} inst={inst} names={names} win={activeWin} t={t} focus={view.focus} orderId={view.order} onPick={pick} />,
  );
  const lifecyclePanel = panel(
    'lifecycle',
    view.order === null ? 'Order lifecycle' : `Order ${view.order} in both scenarios`,
    view.order === null ? (
      <div className="pb-empty">
        <p>No order selected yet. Start with the first order whose handling differs between the scenarios.</p>
        {first ? (
          <button type="button" className="btn-primary pb-cta" onClick={() => selectOrder(first.order_id, first.decision_ts)}>
            Show the first divergence (order {first.order_id})
          </button>
        ) : (
          <p>The stored comparison has no divergence.</p>
        )}
      </div>
    ) : order.error ? (
      <ErrorBox error={order.error} onRetry={order.reload} />
    ) : !order.data ? (
      <Spinner label={`Loading order ${view.order}`} />
    ) : (
      <LifecyclePanel story={story} orderId={view.order} details={order.data} inst={inst} names={names} focus={view.focus} onPick={pick} />
    ),
  );

  const save = (id: LayoutId) => (layout: Layout, meta: { isUserInteraction: boolean }) => {
    if (meta.isUserInteraction) localStorage.setItem(LAYOUT_KEYS[id], JSON.stringify(layout));
  };

  return (
    <>
      <Header
        answer={summary.oneLine}
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
      <div className="pb-body">
        <aside className="pb-rail" aria-label="Experiment controls">
          <fieldset className="pb-scenarios">
            <legend>Scenario in focus</legend>
            {(['both', 'fast', 'slow'] as const).map((k) => (
              <label key={k} className="pb-radio">
                <input type="radio" name="pb-scenario" value={k} checked={view.focus === k} onChange={() => view.update({ scenario: k === 'both' ? null : k })} />
                <span>
                  {k === 'both' ? 'Compare both' : `${names[k]} latency`}
                  <span className="pb-radio-sub">{k === 'both' ? 'side by side' : k === 'fast' ? story.fast.label : story.slow.label}</span>
                </span>
              </label>
            ))}
          </fieldset>
          <nav aria-label="Guided steps">
            <ol className="pb-steps-rail" onKeyDown={onRailKey}>
              {STEPS.map((s, i) => (
                <li key={s.n}>
                  <button
                    ref={(el) => {
                      stepButtons.current[i] = el;
                    }}
                    type="button"
                    className="pb-step"
                    aria-current={view.step === s.n ? 'step' : undefined}
                    tabIndex={view.step === s.n ? 0 : -1}
                    onClick={() => goStep(s.n)}
                  >
                    <span className="pb-step-n" aria-hidden="true">
                      {s.n}
                    </span>
                    <span>
                      <span className="pb-step-title">{s.title}</span>
                      <span className="pb-step-hint">{s.hint}</span>
                    </span>
                  </button>
                </li>
              ))}
            </ol>
            <p className="pb-kbd-hint">
              Arrow keys move between steps; <kbd>[</kbd> and <kbd>]</kbd> work anywhere.
            </p>
          </nav>
          <div className="pb-rail-links">
            {view.order !== null && order.data
              ? (['fast', 'slow'] as const).map((k) =>
                  order.data?.[k] ? (
                    <Link key={k} to={paths.order(k === 'fast' ? story.fast.runId : story.slow.runId, view.order as number)}>
                      Order {view.order} forensics, {names[k]} &rarr;
                    </Link>
                  ) : null,
                )
              : null}
            {wide ? (
              <button type="button" className="btn-link" onClick={resetLayout}>
                Reset panel layout
              </button>
            ) : null}
          </div>
        </aside>
        <main id="pb-main" className="pb-main" tabIndex={-1}>
          {wide ? (
            <Group key={`rows-${layoutGen}`} id="pb-rows" orientation="vertical" className="pb-group" defaultLayout={savedLayout('rows')} onLayoutChanged={save('rows')}>
              <Panel id="top" defaultSize="45%" minSize="25%">
                <Group key={`top-${layoutGen}`} id="pb-top" orientation="horizontal" className="pb-group" defaultLayout={savedLayout('top')} onLayoutChanged={save('top')}>
                  <Panel id="guide" defaultSize="44%" minSize="25%">
                    {guidePanel}
                  </Panel>
                  <Separator className="pb-sep" aria-label="Resize guide and equity panels" />
                  <Panel id="equity" minSize="25%">
                    {equityPanel}
                  </Panel>
                </Group>
              </Panel>
              <Separator className="pb-sep pb-sep-h" aria-label="Resize top and bottom panel rows" />
              <Panel id="bottom" minSize="25%">
                <Group key={`bottom-${layoutGen}`} id="pb-bottom" orientation="horizontal" className="pb-group" defaultLayout={savedLayout('bottom')} onLayoutChanged={save('bottom')}>
                  <Panel id="replay" defaultSize="54%" minSize="30%">
                    {replayPanel}
                  </Panel>
                  <Separator className="pb-sep" aria-label="Resize replay and order lifecycle panels" />
                  <Panel id="lifecycle" minSize="25%">
                    {lifecyclePanel}
                  </Panel>
                </Group>
              </Panel>
            </Group>
          ) : (
            <div className="pb-stack">
              {guidePanel}
              {equityPanel}
              {replayPanel}
              {lifecyclePanel}
            </div>
          )}
        </main>
      </div>
      <p className="visually-hidden" aria-live="polite">
        {announce}
      </p>
    </>
  );
}
