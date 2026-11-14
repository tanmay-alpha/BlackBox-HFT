// Workspace-level tabs of Experiment Comparison (UI_DESIGN_SPEC.md §6.5, §11.8): each is a route.
// Pair compares two runs, Set compares a baseline with N runs (the former Latency Lab), Sweep
// runs and reads a two-axis parameter grid (the former Sensitivity Grid).

import { RouteTabs } from '../../components/Tabs';
import { paths } from '../../shell/paths';

export function CompareTabs() {
  return (
    <RouteTabs
      label="Comparison mode"
      items={[
        { to: '/compare/pair', label: 'Pair' },
        { to: '/compare/set', label: 'Set' },
        { to: paths.sweep(), label: 'Sweep' },
      ]}
    />
  );
}
