// Experiment Comparison -> Pair. The comparison view itself (features/compare) is mounted as is;
// this frame gives it the workspace toolbar and title.

import { useSearchParams } from 'react-router-dom';

import { Workspace } from '../../shell/Workspace';
import ComparePage from '../compare/ComparePage';
import { CompareTabs } from './CompareTabs';

export default function PairWorkspace() {
  const [params] = useSearchParams();
  const a = params.get('a');
  const b = params.get('b');
  return (
    <Workspace toolbar={<CompareTabs />} context={a && b ? `${a.slice(0, 12)} vs ${b.slice(0, 12)}` : null} scroll ownHeading>
      <div className="pad-3">
        <ComparePage />
      </div>
    </Workspace>
  );
}
