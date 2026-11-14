// The workspaces in the navigation (UI_DESIGN_SPEC.md §5.3, §8.1, §12). Only workspaces backed by
// an existing endpoint are listed. Simulation Workbench (SW), Risk & Performance (RP), Research
// Reports (RR) and System/Admin (SA) need the v1 API (jobs, health detail, auth) and are added
// with it; until then their content lives where §12 maps it today (run metrics and the report
// download under Execution Forensics, health in the status bar and Overview).

import { BookOpen, Database, GitCompareArrows, LayoutDashboard, Microscope, SlidersHorizontal } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

import { paths } from './paths';

export interface WorkspaceDef {
  id: 'overview' | 'data' | 'book' | 'strategy' | 'forensics' | 'compare';
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
  { id: 'forensics', name: 'Execution Forensics', mnemonic: 'EF', chord: 'f', path: paths.runs(), icon: Microscope, owns: under('/forensics') },
  { id: 'compare', name: 'Experiment Comparison', mnemonic: 'EC', chord: 'c', path: paths.compare(), icon: GitCompareArrows, owns: under('/compare') },
];

export function workspaceFor(pathname: string): WorkspaceDef | undefined {
  return WORKSPACES.find((w) => w.owns(pathname));
}
