// The `Market only | With run` segmented control of the Order Book Lab toolbar (§11.3). Market
// mode offers the stored runs on this dataset; run mode links back to the dataset's market view.

import { Link, useNavigate } from 'react-router-dom';

import type { RunSummary } from '../../api/types';
import { paths } from '../../shell/paths';
import type { AsyncResult } from '../../components/ui';

export type BookMarketQuery = { t0?: number; len?: number; levels?: number };

function runReason(runs: AsyncResult<RunSummary[]>, matching: RunSummary[]): string | null {
  if (runs.error) return `Stored runs could not be loaded: ${runs.error instanceof Error ? runs.error.message : String(runs.error)}`;
  if (!runs.data) return 'Loading the stored runs';
  if (matching.length === 0) return 'No stored run uses this dataset';
  return null;
}

/** Market mode: "Market only" is current; "With run" picks a stored run on the dataset. */
export function BookModeMarket({
  datasetId,
  runs,
  cursor,
}: {
  datasetId: string;
  runs: AsyncResult<RunSummary[]>;
  /** Exchange-clock cursor handed to the run view, or `null`. */
  cursor: number | null;
}) {
  const navigate = useNavigate();
  const matching = (runs.data ?? []).filter((r) => r.manifest.dataset.id === datasetId);
  const reason = runReason(runs, matching);
  return (
    <div className="book-mode" role="group" aria-label="Book view">
      <span className="book-mode-current" aria-current="page">
        Market only
      </span>
      <label title={reason ?? 'Replay a stored run on this dataset at the same time'}>
        With run
        <select
          aria-label="Open a stored run on this dataset"
          value=""
          disabled={reason !== null}
          onChange={(e) => {
            if (e.target.value) navigate(paths.bookRun(e.target.value, { t: cursor === null ? null : Math.round(cursor) }));
          }}
        >
          <option value="">{reason ?? `${matching.length} stored run${matching.length === 1 ? '' : 's'}`}</option>
          {matching.map((r) => (
            <option key={r.manifest.run_id} value={r.manifest.run_id}>
              {r.label ? `${r.label} \u00b7 ${r.manifest.run_id}` : r.manifest.run_id}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}

/** Run mode: "With run" is current; "Market only" opens the same window without the run. */
export function BookModeRun({ datasetId, query }: { datasetId: string; query: BookMarketQuery | null }) {
  return (
    <div className="book-mode" role="group" aria-label="Book view">
      <Link to={paths.bookDataset(datasetId, query ?? undefined)} title="The same dataset window without the run's simulated orders">
        Market only
      </Link>
      <span className="book-mode-current" aria-current="page">
        With run
      </span>
    </div>
  );
}
