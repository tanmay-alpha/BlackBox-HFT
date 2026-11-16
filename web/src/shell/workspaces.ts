// The workspaces in the navigation (UI_DESIGN_SPEC.md §5.3, §8.1, §12), each backed by `/api/v1`
// endpoints (API_CONTRACT.md §8): Simulation Workbench by jobs, Risk & Performance by run metrics
// and equity, Research Reports by run reports and saved reports, System by status, readiness and
// the admin reads.

import {
  BookOpen,
  Cpu,
  Database,
  FileText,
  FlaskConical,
  GitCompareArrows,
  LayoutDashboard,
  Layers,
  LineChart,
  Microscope,
  Server,
  SlidersHorizontal,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

import { paths } from './paths';

/** The five navigation sections (docs/rebuild/TARGET_PRODUCT_ARCHITECTURE.md §2). */
export type SectionId = 'explore' | 'studio' | 'market' | 'execution' | 'system';

export const SECTIONS: { id: SectionId; name: string }[] = [
  { id: 'explore', name: 'Explore' },
  { id: 'studio', name: 'Research Studio' },
  { id: 'market', name: 'Market Lab' },
  { id: 'execution', name: 'Execution Analysis' },
  { id: 'system', name: 'System' },
];

export interface WorkspaceDef {
  id: 'explore' | 'overview' | 'data' | 'book' | 'market' | 'strategy' | 'sim' | 'forensics' | 'risk' | 'compare' | 'reports' | 'system';
  name: string;
  section: SectionId;
  /** Two-letter mnemonic, shown only in the collapsed rail. */
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
  { id: 'explore', name: 'Latency experiment', section: 'explore', mnemonic: 'LX', chord: 'e', path: paths.explore(), icon: FlaskConical, owns: (p) => p === '/' },
  { id: 'overview', name: 'All experiments', section: 'explore', mnemonic: 'EX', chord: 'o', path: paths.overview(), icon: LayoutDashboard, owns: under('/experiments') },
  { id: 'strategy', name: 'Strategy Lab', section: 'studio', mnemonic: 'SL', chord: 'l', path: paths.strategy(), icon: SlidersHorizontal, owns: under('/strategy') },
  { id: 'sim', name: 'Jobs & Sweeps', section: 'studio', mnemonic: 'JS', chord: 'w', path: paths.sim(), icon: Cpu, owns: under('/sim') },
  { id: 'reports', name: 'Reports', section: 'studio', mnemonic: 'RE', chord: 'p', path: paths.reports(), icon: FileText, owns: under('/reports') },
  { id: 'data', name: 'Datasets', section: 'market', mnemonic: 'DS', chord: 'm', path: paths.datasets(), icon: Database, owns: under('/data') },
  { id: 'book', name: 'Order Book Replay', section: 'market', mnemonic: 'OB', chord: 'b', path: paths.book(), icon: BookOpen, owns: under('/book') },
  { id: 'market', name: 'Market Data Workbench', section: 'market', mnemonic: 'MW', chord: 'k', path: paths.market(), icon: Layers, owns: under('/market') },
  { id: 'forensics', name: 'Runs & Forensics', section: 'execution', mnemonic: 'RF', chord: 'f', path: paths.runs(), icon: Microscope, owns: under('/forensics') },
  { id: 'compare', name: 'Comparisons', section: 'execution', mnemonic: 'CO', chord: 'c', path: paths.compare(), icon: GitCompareArrows, owns: under('/compare') },
  { id: 'risk', name: 'Risk & P&L', section: 'execution', mnemonic: 'RP', chord: 'r', path: paths.risk(), icon: LineChart, owns: under('/risk') },
  { id: 'system', name: 'System status', section: 'system', mnemonic: 'SY', chord: 'a', path: paths.system(), icon: Server, owns: under('/system') },
];

export function workspaceFor(pathname: string): WorkspaceDef | undefined {
  return WORKSPACES.find((w) => w.owns(pathname));
}
