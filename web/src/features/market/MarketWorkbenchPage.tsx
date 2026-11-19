// Market Data Workbench (Phase 4): one L3 (order-level) dataset in five steps, Dataset ->
// Validation -> Order book -> Event timeline -> Research metrics. The URL holds the dataset (path),
// the step (`tab`), the selected event (`event`, shared by the book, timeline and charts), the
// timeline filter (`type`) and the timeline start (`from`). J/K step to the next/previous event.

import { useCallback, useEffect, useRef, useState } from 'react';
import type { FormEvent, ReactNode } from 'react';
import { ChevronLeft, ChevronRight } from 'lucide-react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom';

import { isAbort } from '../../api/client';
import { DataTable, type Column } from '../../components/DataTable';
import { fmtInt } from '../../components/fmt';
import { usePhone } from '../../components/PaneTabs';
import { Tabs } from '../../components/Tabs';
import { ToolbarDivider } from '../../components/Toolbar';
import { BasisLegend, Empty, ErrorBox, HashText, Notice, Spinner, useAsync } from '../../components/ui';
import { useInspector } from '../../shell/inspector';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import { marketApi, type MarketSummary } from './api';
import { DatasetTab } from './DatasetTab';
import { EventCompare } from './EventCompare';
import { EventFacts } from './EventFacts';
import { DataClassBadge } from './labels';
import { MetricsTab } from './MetricsTab';
import { MARKET_TABS, parseExampleParam, parseWorkbenchQuery, stepWithin, type MarketTab, type WorkbenchQuery } from './model';
import { EventTouch, TimelineTab } from './TimelineTab';
import { findNeighbour, PAGE_SIZE, useEventPager, useSelectedEvent } from './useEvents';
import { ValidationTab } from './ValidationTab';
import './market.css';

function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName);
}

const INDEX_COLUMNS: Column<MarketSummary>[] = [
  {
    id: 'id',
    header: 'Dataset',
    rowHeader: true,
    cell: (d) => (
      <Link to={paths.marketDataset(d.dataset_id)} className="mono">
        {d.dataset_id}
      </Link>
    ),
  },
  { id: 'symbol', header: 'Instrument', cell: (d) => <span className="mono">{d.symbol}</span> },
  { id: 'venue', header: 'Venue', cell: (d) => d.venue },
  { id: 'class', header: 'Data class', cell: (d) => <DataClassBadge dataClass={d.data_class} /> },
  { id: 'events', header: 'Events', unit: 'count', numeric: true, cell: (d) => fmtInt(d.events) },
  { id: 'quality', header: 'Quality', cell: (d) => (d.quality.passed ? 'passed' : 'not passed') },
];

function MarketIndex() {
  const list = useAsync((signal) => marketApi.datasets({ signal }), []);
  const navigate = useNavigate();
  return (
    <Workspace ownHeading scroll>
      <div className="col mk-index">
        <h1>Market Data Workbench</h1>
        <p className="prose">
          Inspect an order-level (L3) market data source step by step: what it is, whether its reconstruction is sound, the full order
          book at any event, the events themselves, and the research measures computed from them, including what a price-level (L2)
          view of the same book loses.
        </p>
        <Notice tag="Data">
          The public workbench serves generated (synthetic) data only. Private exchange files stay on the machine that owns them and are
          never served here.
        </Notice>
        {list.error ? (
          <ErrorBox error={list.error} onRetry={list.reload} />
        ) : !list.data ? (
          <Spinner label="Loading workbench datasets" />
        ) : (
          <DataTable
            label="Workbench datasets"
            columns={INDEX_COLUMNS}
            rows={list.data.items}
            rowKey={(d) => d.dataset_id}
            onActivate={(d) => navigate(paths.marketDataset(d.dataset_id))}
            empty="No workbench dataset is available on this server."
          />
        )}
      </div>
    </Workspace>
  );
}

function GoToEvent({ selected, max, onGo }: { selected: number | null; max: number | null; onGo: (index: number) => void }) {
  const [text, setText] = useState('');
  const value = text.trim() === '' ? null : Number(text);
  const invalid = value !== null && (!Number.isSafeInteger(value) || value < 0 || (max !== null && value > max));
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (value === null || invalid) return;
    onGo(value);
    setText('');
  };
  return (
    <form className="row nowrap mk-inline-form" onSubmit={submit}>
      <label htmlFor="mk-goto" className="mk-goto-label">
        Event
      </label>
      <input
        id="mk-goto"
        className="mk-index-input"
        inputMode="numeric"
        placeholder={selected === null ? 'index' : String(selected)}
        value={text}
        aria-invalid={invalid || undefined}
        title={max === null ? 'Event index' : `Event index from 0 to ${fmtInt(max)}`}
        onChange={(e) => setText(e.target.value)}
      />
      <button type="submit" disabled={value === null || invalid}>
        Go
      </button>
    </form>
  );
}

function Workbench({ datasetId }: { datasetId: string }) {
  const [sp] = useSearchParams();
  const navigate = useNavigate();
  const q = parseWorkbenchQuery(sp);
  const phone = usePhone();
  const summary = useAsync((signal) => marketApi.dataset(datasetId, { signal }), [datasetId]);
  const s = summary.data;
  const maxIndex = s ? Math.max(0, s.events - 1) : null;
  const pager = useEventPager(datasetId, q.type, q.from, q.tab === 'events');
  // The book step reads its event from the event detail; only the timeline needs this lookup.
  const selectedEvent = useSelectedEvent(datasetId, q.tab === 'book' ? null : q.event);
  const examples = useAsync((signal) => marketApi.examples(datasetId, { signal }), [datasetId]);
  const exampleId = parseExampleParam(sp);
  const [navNote, setNavNote] = useState<string | null>(null);
  const [stepping, setStepping] = useState(false);
  const stepCtl = useRef<AbortController | null>(null);
  // The latest selection, ahead of the URL: key repeats arrive before the navigation re-renders.
  const selRef = useRef<number | null>(q.event);
  useEffect(() => {
    selRef.current = q.event;
  }, [q.event]);

  const go = useCallback(
    (patch: Partial<WorkbenchQuery>, replace: boolean) => {
      const next = { ...q, ...patch };
      navigate(paths.marketDataset(datasetId, { tab: next.tab, event: next.event, type: next.type, from: next.from }), { replace });
    },
    [navigate, datasetId, q],
  );

  // A book step without an event (or a `?example=` link) opens on a curated example.
  useEffect(() => {
    if (q.tab !== 'book' || q.event !== null || !examples.data) return;
    const items = examples.data.items;
    const pick = items.find((x) => x.id === exampleId) ?? items[0];
    if (pick) go({ event: pick.event_index }, true);
  }, [q.tab, q.event, examples.data, exampleId, go]);

  const select = useCallback(
    (index: number) => {
      stepCtl.current?.abort();
      stepCtl.current = null;
      setStepping(false);
      setNavNote(null);
      selRef.current = index;
      go({ event: index }, true);
    },
    [go],
  );

  const step = useCallback(
    (dir: 1 | -1) => {
      // One server search at a time; presses during it are dropped rather than queued.
      if (stepCtl.current !== null) return;
      const current = selRef.current;
      if (current === null && dir === -1) {
        setNavNote('Select an event first.');
        return;
      }
      const from = current ?? -1;
      const page = q.tab === 'events' ? pager.page.data : null;
      if (page) {
        const local = stepWithin(page.items, from, dir, { from: pager.start, complete: page.next_cursor === null });
        if (local.kind === 'select') {
          select(local.index);
          return;
        }
      }
      const ctl = new AbortController();
      stepCtl.current = ctl;
      setStepping(true);
      setNavNote(null);
      const type = q.tab === 'events' ? q.type : null;
      findNeighbour(datasetId, type, from, dir, ctl.signal).then(
        (index) => {
          if (ctl.signal.aborted) return;
          stepCtl.current = null;
          setStepping(false);
          if (index === null) {
            setNavNote(dir === 1 ? 'No later event.' : 'No earlier event.');
            return;
          }
          // Keep the timeline page on the new selection.
          const onPage = page?.items.some((e) => e.event_index === index) ?? false;
          const patch: Partial<WorkbenchQuery> = { event: index };
          if (q.tab === 'events' && !onPage) patch.from = dir === 1 ? index : Math.max(0, index - PAGE_SIZE + 1);
          selRef.current = index;
          go(patch, true);
        },
        (e: unknown) => {
          if (ctl.signal.aborted || isAbort(e)) return;
          stepCtl.current = null;
          setStepping(false);
          setNavNote(e instanceof Error ? e.message : String(e));
        },
      );
    },
    [q.tab, q.type, pager.page.data, pager.start, datasetId, select, go],
  );

  useEffect(() => () => stepCtl.current?.abort(), []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey || isTyping(e.target)) return;
      if (document.querySelector('dialog[open]')) return;
      if (e.key === 'j') step(1);
      else if (e.key === 'k') step(-1);
      else return;
      e.preventDefault();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [step]);

  useInspector(
    'Market Data Workbench',
    <div className="col prose">
      <p>
        The book is rebuilt from the order-level message stream by the ITCH 5.0 decoder and L3 reconstruction. Event indices are the
        source frame sequence; times are nanoseconds since the session{'\u2019'}s midnight.
      </p>
      <p>
        Keys: <kbd>J</kbd> next event, <kbd>K</kbd> previous event (in the timeline they follow its type filter).
      </p>
      <BasisLegend />
    </div>,
    [],
  );

  const eventOutOfRange = q.event !== null && maxIndex !== null && q.event > maxIndex;
  const failure = selectedEvent.error ? selectedEvent.error.message : null;
  const eventFacts =
    q.event === null ? null : (
      <EventFacts index={q.event} event={selectedEvent.loading ? undefined : selectedEvent.data} failure={failure} />
    );

  const toolbar = (
    <>
      {phone ? null : (
        <>
          <Link className="btn" to={paths.market()}>
            All workbench datasets
          </Link>
          <ToolbarDivider />
        </>
      )}
      {/* Busy, not disabled: disabling the focused button would drop keyboard focus to the page on
          every press; `step` already ignores presses while a search runs. */}
      <button
        type="button"
        className={phone ? 'btn-icon' : undefined}
        onClick={() => step(-1)}
        aria-disabled={stepping || undefined}
        aria-keyshortcuts="k"
        aria-label={phone ? 'Previous event' : undefined}
        title="Previous event (K)"
      >
        {phone ? <ChevronLeft size={18} aria-hidden="true" /> : 'Previous'}
      </button>
      <GoToEvent selected={q.event} max={maxIndex} onGo={(i) => go({ event: i, from: q.tab === 'events' ? i : q.from }, false)} />
      <button
        type="button"
        className={phone ? 'btn-icon' : undefined}
        onClick={() => step(1)}
        aria-disabled={stepping || undefined}
        aria-keyshortcuts="j"
        aria-label={phone ? 'Next event' : undefined}
        title="Next event (J)"
      >
        {phone ? <ChevronRight size={18} aria-hidden="true" /> : 'Next'}
      </button>
      <span className="mk-nav-note" role="status" aria-live="polite">
        {stepping ? 'Finding event\u2026' : (navNote ?? '')}
      </span>
    </>
  );

  let content: ReactNode;
  if (summary.error) content = <ErrorBox error={summary.error} onRetry={summary.reload} />;
  else if (!s) content = <Spinner label="Loading the dataset summary" />;
  else if (q.tab === 'dataset') content = <DatasetTab summary={s} />;
  else if (q.tab === 'validation') content = <ValidationTab summary={s} />;
  else if (q.tab === 'metrics') content = <MetricsTab datasetId={datasetId} selected={q.event} onSelect={select} />;
  else if (q.tab === 'events')
    content = (
      <TimelineTab
        summary={s}
        pager={pager}
        type={q.type}
        from={q.from}
        selected={q.event}
        selectedEvent={
          <>
            {eventFacts}
            {selectedEvent.data ? <EventTouch event={selectedEvent.data} /> : null}
          </>
        }
        onSelect={select}
        onType={(type) => go({ type }, false)}
        onFrom={(from) => go({ from }, false)}
        phone={phone}
        bookLink={
          q.event !== null && !eventOutOfRange ? (
            <Link className="btn" to={paths.marketDataset(datasetId, { tab: 'book', event: q.event, type: q.type, from: q.from })}>
              Open the order book at this event
            </Link>
          ) : null
        }
      />
    );
  else if (q.event === null && !examples.error && (examples.data === null || examples.data.items.length > 0))
    // The book step opens on a curated example (the effect above selects it), never an empty early book.
    content = <Spinner label="Opening a curated example event" />;
  else if (q.event === null || eventOutOfRange)
    content = (
      <div className="mk-scroll">
        <div className="col mk-stack">
          {eventOutOfRange ? (
            <Notice tag="Out of range" tone="error">
              Event #{fmtInt(q.event)} is beyond this dataset, whose event indices run from 0 to {fmtInt(maxIndex)}.
            </Notice>
          ) : null}
          <Empty>
            <p>Choose the event whose book to show: select one in the event timeline, enter an index above, or start at either end.</p>
            <div className="row">
              <button type="button" onClick={() => go({ event: 0 }, false)}>
                Book after event #0
              </button>
              <button type="button" onClick={() => go({ event: maxIndex }, false)}>
                Book after the last event (#{fmtInt(maxIndex)})
              </button>
            </div>
          </Empty>
        </div>
      </div>
    );
  else
    content = <EventCompare datasetId={datasetId} eventIndex={q.event} examples={examples.data?.items ?? []} phone={phone} onSelect={select} />;

  return (
    <Workspace context={s ? `${s.symbol} \u00b7 ${MARKET_TABS.find((t) => t.id === q.tab)?.label ?? ''}` : datasetId} layout toolbar={toolbar} ownHeading>
      <div className="mk-page">
        <header className="mk-head">
          <h1 className="mk-title">
            {s ? (
              <>
                <span className="mono">{s.symbol}</span> <span className="mk-title-sub">{s.venue}</span>
              </>
            ) : (
              <span className="mono">{datasetId}</span>
            )}
          </h1>
          {/* The line is always present, on its own row and one line high (ellipsized; the full text
              is its title), so the tabs below never move when the summary arrives. */}
          <p
            className="mk-meta"
            title={s ? `${s.data_class} \u00b7 ${s.session} \u00b7 version ${s.dataset_version} \u00b7 ${fmtInt(s.events)} events` : undefined}
          >
            {s ? (
              <>
                <DataClassBadge dataClass={s.data_class} />{' '}
                <span className="mk-meta-extra">
                  {s.session} {'\u00b7'} version <HashText hash={s.dataset_version} chars={10} /> {'\u00b7'}{' '}
                </span>
                {fmtInt(s.events)} events
              </>
            ) : (
              '\u00a0'
            )}
          </p>
        </header>
        <Tabs<MarketTab>
          label="Workbench steps"
          className="mk-tabs"
          active={q.tab}
          onChange={(tab) => go({ tab }, false)}
          tabs={phone ? MARKET_TABS.map((t) => ({ id: t.id, label: t.short })) : MARKET_TABS}
        >
          {content}
        </Tabs>
      </div>
    </Workspace>
  );
}

export default function MarketWorkbenchPage() {
  const { datasetId } = useParams();
  return datasetId ? <Workbench datasetId={datasetId} /> : <MarketIndex />;
}
