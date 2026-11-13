import { Component } from 'react';
import type { ErrorInfo, ReactNode } from 'react';
import { BrowserRouter, Link, NavLink, Outlet, Route, Routes, useLocation } from 'react-router-dom';

import { api } from './api/client';
import { CONTRACT_VERSION } from './api/types';
import { usePageTitle } from './components/page';
import { Badge, BasisLegend, Empty, ErrorBox, useAsync } from './components/ui';
import ComparePage from './features/compare/ComparePage';
import DatasetsPage from './features/datasets/DatasetsPage';
import OrderForensicsPage from './features/forensics/OrderForensicsPage';
import GridPage from './features/grid/GridPage';
import LabPage from './features/lab/LabPage';
import ReplayPage from './features/replay/ReplayPage';
import RunOverviewPage from './features/runs/RunOverviewPage';
import RunsPage from './features/runs/RunsPage';

/** Shown on every page. The wording is fixed by the product's honesty rules. */
const RESEARCH_BANNER =
  'Simulated on historical replay \u2014 research only. Simulated orders do not affect historical prices.';

interface NavItem {
  to: string;
  label: string;
  /** Other path prefixes that belong to this section. */
  also?: string[];
  end?: boolean;
}

const NAV: NavItem[] = [
  { to: '/', label: 'Datasets', end: true, also: ['/replay'] },
  { to: '/lab', label: 'Strategy Lab' },
  { to: '/runs', label: 'Runs' },
  { to: '/compare', label: 'Compare' },
  { to: '/grid', label: 'Sensitivity Grid' },
];

/** Keeps a rendering failure in one page from blanking the whole console. */
class PageErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: unknown): { error: Error } {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error('Page failed to render', error, info.componentStack);
  }

  render(): ReactNode {
    if (this.state.error) {
      return (
        <div className="col">
          <h1>This page failed to render</h1>
          <ErrorBox error={this.state.error} />
          <p className="note">
            Nothing was simulated or changed by this failure. Reload the page or go back to{' '}
            <a href="/">Datasets</a>.
          </p>
        </div>
      );
    }
    return this.props.children;
  }
}

function ServerStatus() {
  const health = useAsync(() => api.health(), []);
  if (health.loading && !health.data) {
    return <span className="dim">Contacting server</span>;
  }
  if (health.error || !health.data) {
    return (
      <span className="row shell-status">
        <Badge kind="error">server unreachable</Badge>
        <span className="dim">{health.error ? health.error.message : 'No response.'}</span>
        <button type="button" onClick={health.reload}>
          Retry
        </button>
      </span>
    );
  }
  const h = health.data;
  const contractOk = h.contract_version === CONTRACT_VERSION;
  return (
    <span className="row shell-status">
      <Badge kind={h.status === 'ok' ? 'ok' : 'warn'}>server {h.status}</Badge>
      <span className="mono dim">
        blackbox {h.blackbox_version} · engine {h.engine.name} {h.engine.version} · contract v
        {h.contract_version}
      </span>
      {contractOk ? null : (
        <Badge
          kind="warn"
          title={`This console was built for contract v${CONTRACT_VERSION}; the server speaks v${h.contract_version}. Fields may be missing or misread.`}
        >
          contract mismatch
        </Badge>
      )}
      <span className="dim">{h.disclaimer}</span>
    </span>
  );
}

function Shell() {
  const location = useLocation();
  return (
    <div className="shell">
      <a className="skip-link" href="#main">
        Skip to content
      </a>
      <header className="shell-header">
        <Link to="/" className="brand" aria-label="BlackBox HFT, datasets">
          <span className="brand-mark" aria-hidden="true" />
          <span className="brand-name">BlackBox HFT</span>
          <span className="brand-sub">execution forensics</span>
        </Link>
        <nav className="shell-nav" aria-label="Primary">
          {NAV.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.end}
              className={({ isActive }) =>
                isActive || (item.also ?? []).some((p) => location.pathname.startsWith(p))
                  ? 'nav-link active'
                  : 'nav-link'
              }
            >
              {item.label}
            </NavLink>
          ))}
        </nav>
      </header>
      <div className="shell-banner" role="note">
        <Badge kind="modeled">simulated</Badge>
        <span>{RESEARCH_BANNER}</span>
      </div>
      <main id="main" className="shell-main" tabIndex={-1}>
        <PageErrorBoundary key={location.pathname}>
          <Outlet />
        </PageErrorBoundary>
      </main>
      <footer className="shell-footer">
        <BasisLegend />
        <ServerStatus />
        <p className="dim">
          Charts on run pages use TradingView Lightweight Charts{'\u2122'}, Copyright (c) 2025
          TradingView, Inc.{' '}
          <a href="https://www.tradingview.com/" target="_blank" rel="noreferrer">
            https://www.tradingview.com/
          </a>
        </p>
      </footer>
    </div>
  );
}

function NotFoundPage() {
  usePageTitle('Not found');
  const location = useLocation();
  return (
    <Empty>
      <p>
        No page at <span className="mono">{location.pathname}</span>.
      </p>
      <p>
        <Link to="/">Go to Datasets</Link>
      </p>
    </Empty>
  );
}

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<Shell />}>
          <Route index element={<DatasetsPage />} />
          <Route path="replay/:datasetId" element={<ReplayPage />} />
          <Route path="lab" element={<LabPage />} />
          <Route path="runs" element={<RunsPage />} />
          <Route path="runs/:runId" element={<RunOverviewPage />} />
          <Route path="runs/:runId/orders/:orderId" element={<OrderForensicsPage />} />
          <Route path="compare" element={<ComparePage />} />
          <Route path="grid" element={<GridPage />} />
          <Route path="*" element={<NotFoundPage />} />
        </Route>
      </Routes>
    </BrowserRouter>
  );
}
