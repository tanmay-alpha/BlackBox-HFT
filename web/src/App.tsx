import { lazy, Suspense } from 'react';
import { BrowserRouter, Link, Navigate, Route, Routes, useLocation, useParams } from 'react-router-dom';

import { Empty } from './components/ui';
import { paths } from './shell/paths';
import { Shell } from './shell/Shell';
import { Workspace } from './shell/Workspace';

// Every workspace is its own chunk (PRODUCT_BASELINE D7); the shell suspends while one loads.
const BookIndexPage = lazy(() => import('./features/book/BookIndexPage'));
const DatasetsPage = lazy(() => import('./features/datasets/DatasetsPage'));
const PairWorkspace = lazy(() => import('./features/experiments/PairWorkspace'));
const MarketWorkbenchPage = lazy(() => import('./features/market/MarketWorkbenchPage'));
const OrderForensicsPage = lazy(() => import('./features/forensics/OrderForensicsPage'));
const SweepFormPage = lazy(() => import('./features/grid/SweepFormPage'));
const SweepPage = lazy(() => import('./features/grid/SweepPage'));
const SweepsPage = lazy(() => import('./features/grid/SweepsPage'));
const LabPage = lazy(() => import('./features/lab/LabPage'));
const LatencyLabPage = lazy(() => import('./features/latency/LatencyLabPage'));
const MissionControlPage = lazy(() => import('./features/mission/MissionControlPage'));
const ReplayPage = lazy(() => import('./features/replay/ReplayPage'));
const ReportsPage = lazy(() => import('./features/reports/ReportsPage'));
const RiskPage = lazy(() => import('./features/risk/RiskPage'));
const RunOverviewPage = lazy(() => import('./features/runs/RunOverviewPage'));
const RunsPage = lazy(() => import('./features/runs/RunsPage'));
const WorkbenchPage = lazy(() => import('./features/sim/WorkbenchPage'));
const SystemPage = lazy(() => import('./features/system/SystemPage'));
const ReplayTerminalPage = lazy(() => import('./features/terminal/ReplayTerminalPage'));
const ExplorePage = lazy(() => import('./features/explore/ExplorePage'));

/**
 * Redirect that keeps the query string (UI_DESIGN_SPEC.md §9.2). `to` maps the matched route
 * parameters to the new path.
 */
function Redirect({ to }: { to: (params: Record<string, string | undefined>) => string }) {
  const params = useParams();
  const { search, hash } = useLocation();
  const target = to(params);
  const join = target.includes('?') ? '&' : '?';
  return <Navigate replace to={`${target}${search ? `${join}${search.slice(1)}` : ''}${hash}`} />;
}

function NotFoundPage() {
  const location = useLocation();
  return (
    <Workspace>
      <Empty>
        <p>
          No view at <span className="mono">{location.pathname}</span>. <Link to={paths.explore()}>Go to the latency experiment</Link> or{' '}
          <Link to={paths.overview()}>all experiments</Link>.
        </p>
      </Empty>
    </Workspace>
  );
}

const r = (id: string | undefined) => id ?? '';

const LEGACY: [string, (p: Record<string, string | undefined>) => string][] = [
  ['datasets', () => paths.datasets()],
  ['replay/:datasetId', (p) => paths.bookDataset(r(p.datasetId))],
  ['lab', () => paths.strategy()],
  ['runs', () => paths.runs()],
  ['runs/:runId', (p) => paths.run(r(p.runId))],
  ['runs/:runId/replay', (p) => paths.bookRun(r(p.runId))],
  ['runs/:runId/orders/:orderId', (p) => paths.order(r(p.runId), r(p.orderId))],
  ['latency', () => paths.compareSet()],
  ['grid', () => paths.sweepForm()],
  ['compare/sweep', () => paths.sweeps()],
  ['compare', () => paths.comparePair()],
  ['sim/runs', () => paths.runs()],
  ['sim/runs/:runId', (p) => paths.run(r(p.runId))],
];

export default function App() {
  return (
    <BrowserRouter>
      <Suspense fallback={null}>
      <Routes>
        <Route element={<Shell />}>
          <Route index element={<ExplorePage />} />
          <Route path="experiments" element={<MissionControlPage />} />
          <Route path="data" element={<DatasetsPage />} />
          <Route path="data/:datasetId" element={<DatasetsPage />} />
          <Route path="book" element={<BookIndexPage />} />
          <Route path="book/data/:datasetId" element={<ReplayPage />} />
          <Route path="book/runs/:runId" element={<ReplayTerminalPage />} />
          <Route path="market" element={<MarketWorkbenchPage />} />
          <Route path="market/:datasetId" element={<MarketWorkbenchPage />} />
          <Route path="strategy" element={<LabPage />} />
          <Route path="forensics" element={<RunsPage />} />
          <Route path="forensics/:runId" element={<RunOverviewPage />} />
          <Route path="forensics/:runId/orders/:orderId" element={<OrderForensicsPage />} />
          <Route path="sim" element={<WorkbenchPage />} />
          <Route path="sim/jobs/:jobId" element={<WorkbenchPage />} />
          <Route path="sim/sweep" element={<SweepFormPage />} />
          <Route path="risk" element={<RiskPage />} />
          <Route path="risk/:runId" element={<RiskPage />} />
          <Route path="reports" element={<ReportsPage />} />
          <Route path="reports/saved/:reportId" element={<ReportsPage />} />
          <Route path="reports/:runId" element={<ReportsPage />} />
          <Route path="system" element={<SystemPage />} />
          <Route path="compare/pair" element={<PairWorkspace />} />
          <Route path="compare/set" element={<LatencyLabPage />} />
          <Route path="compare/sweeps" element={<SweepsPage />} />
          <Route path="compare/sweeps/:sweepId" element={<SweepPage />} />
          {LEGACY.map(([path, to]) => (
            <Route key={path} path={path} element={<Redirect to={to} />} />
          ))}
          <Route path="*" element={<NotFoundPage />} />
        </Route>
      </Routes>
      </Suspense>
    </BrowserRouter>
  );
}
