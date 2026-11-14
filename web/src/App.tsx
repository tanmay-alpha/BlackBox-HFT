import { BrowserRouter, Link, Navigate, Route, Routes, useLocation, useParams } from 'react-router-dom';

import { Empty } from './components/ui';
import BookIndexPage from './features/book/BookIndexPage';
import DatasetsPage from './features/datasets/DatasetsPage';
import PairWorkspace from './features/experiments/PairWorkspace';
import OrderForensicsPage from './features/forensics/OrderForensicsPage';
import SweepFormPage from './features/grid/SweepFormPage';
import SweepPage from './features/grid/SweepPage';
import SweepsPage from './features/grid/SweepsPage';
import LabPage from './features/lab/LabPage';
import LatencyLabPage from './features/latency/LatencyLabPage';
import MissionControlPage from './features/mission/MissionControlPage';
import ReplayPage from './features/replay/ReplayPage';
import ReportsPage from './features/reports/ReportsPage';
import RiskPage from './features/risk/RiskPage';
import RunOverviewPage from './features/runs/RunOverviewPage';
import RunsPage from './features/runs/RunsPage';
import WorkbenchPage from './features/sim/WorkbenchPage';
import SystemPage from './features/system/SystemPage';
import ReplayTerminalPage from './features/terminal/ReplayTerminalPage';
import { paths } from './shell/paths';
import { Shell } from './shell/Shell';
import { Workspace } from './shell/Workspace';

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
          No view at <span className="mono">{location.pathname}</span>. <Link to={paths.overview()}>Go to Overview</Link>
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
      <Routes>
        <Route element={<Shell />}>
          <Route index element={<MissionControlPage />} />
          <Route path="data" element={<DatasetsPage />} />
          <Route path="data/:datasetId" element={<DatasetsPage />} />
          <Route path="book" element={<BookIndexPage />} />
          <Route path="book/data/:datasetId" element={<ReplayPage />} />
          <Route path="book/runs/:runId" element={<ReplayTerminalPage />} />
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
    </BrowserRouter>
  );
}
