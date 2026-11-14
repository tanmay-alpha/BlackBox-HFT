// Workspace-level tabs of Experiment Comparison (UI_DESIGN_SPEC.md §6.5, §11.8): each is a route.
// Pair compares two runs, Set compares a baseline with N runs (the former Latency Lab), Sweeps
// lists the workspace's sweeps and opens each one's result matrix (sweeps are submitted from the
// Simulation Workbench).

import { RouteTabs } from '../../components/Tabs';
import { paths } from '../../shell/paths';

export function CompareTabs() {
  return (
    <RouteTabs
      label="Comparison mode"
      items={[
        { to: paths.comparePair(), label: 'Pair' },
        { to: paths.compareSet(), label: 'Set' },
        { to: paths.sweeps(), label: 'Sweeps' },
      ]}
    />
  );
}
