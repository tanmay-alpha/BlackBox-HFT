// The workspaces in the navigation (UI_DESIGN_SPEC.md §5.3, §8.1, §12), each backed by `/api/v1`
// endpoints (API_CONTRACT.md §8): Simulation Workbench by jobs, Risk & Performance by run metrics
// and equity, Research Reports by run reports and saved reports, System by status, readiness and
// the admin reads.

import {
  BookOpen,
  Cpu,
  Database,
  FileText,
  GitCompareArrows,
  LayoutDashboard,
  LineChart,
  Microscope,
  Server,
  SlidersHorizontal,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

import { paths } from './paths';

export interface WorkspaceDef {
  id: 'overview' | 'data' | 'book' | 'strategy' | 'sim' | 'forensics' | 'risk' | 'compare' | 'reports' | 'system';
  name: string;
  /** Two-letter rail mnemonic. */
  mnemonic: string;
  /** Second key of the `g` chord. */
  chord: string;
  path: string;
  icon: LucideIcon;
  /** Whether a pathname belongs to this workspace. */
  owns: (pathname: string) => boolean;
}

const under = (prefix: string) => (p: string) => p === prefix || p.startsWith(`${prefix}/`);

export const WORKSPACES: WorkspaceDef[] = [
  { id: 'overview', name: 'Overview', mnemonic: 'OV', chord: 'o', path: paths.overview(), icon: LayoutDashboard, owns: (p) => p === '/' },
  { id: 'data', name: 'Market Data Explorer', mnemonic: 'MD', chord: 'm', path: paths.datasets(), icon: Database, owns: under('/data') },
  { id: 'book', name: 'Order Book Lab', mnemonic: 'OB', chord: 'b', path: paths.book(), icon: BookOpen, owns: under('/book') },
  { id: 'strategy', name: 'Strategy Lab', mnemonic: 'SL', chord: 'l', path: paths.strategy(), icon: SlidersHorizontal, owns: under('/strategy') },
  { id: 'sim', name: 'Simulation Workbench', mnemonic: 'SW', chord: 'w', path: paths.sim(), icon: Cpu, owns: under('/sim') },
  { id: 'forensics', name: 'Execution Forensics', mnemonic: 'EF', chord: 'f', path: paths.runs(), icon: Microscope, owns: under('/forensics') },
  { id: 'risk', name: 'Risk & Performance', mnemonic: 'RP', chord: 'r', path: paths.risk(), icon: LineChart, owns: under('/risk') },
  { id: 'compare', name: 'Experiment Comparison', mnemonic: 'EC', chord: 'c', path: paths.compare(), icon: GitCompareArrows, owns: under('/compare') },
  { id: 'reports', name: 'Research Reports', mnemonic: 'RR', chord: 'p', path: paths.reports(), icon: FileText, owns: under('/reports') },
  { id: 'system', name: 'System', mnemonic: 'SA', chord: 'a', path: paths.system(), icon: Server, owns: under('/system') },
];

export function workspaceFor(pathname: string): WorkspaceDef | undefined {
  return WORKSPACES.find((w) => w.owns(pathname));
}
