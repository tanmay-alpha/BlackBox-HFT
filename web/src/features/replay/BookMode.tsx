// The `Market only | With run` segmented control of the Order Book Lab toolbar (§11.3). Market
// mode offers the stored runs on this dataset; run mode links back to the dataset's market view.

import { Link, useNavigate } from 'react-router-dom';

import type { Collected } from '../../api/client';
import type { Run } from '../../api/model';
import { paths } from '../../shell/paths';
import { runLabelText, useDemoTitles } from '../../components/labels';
import type { AsyncResult } from '../../components/ui';

export type BookMarketQuery = { t0?: number; len?: number; levels?: number };

function runReason(runs: AsyncResult<Collected<Run>>, matching: Run[]): string | null {
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
  /** Stored runs, ideally already filtered to `datasetId` by the server. */
  runs: AsyncResult<Collected<Run>>;
  /** Exchange-clock cursor handed to the run view, or `null`. */
  cursor: number | null;
}) {
  const navigate = useNavigate();
  const demoTitles = useDemoTitles();
  const matching = (runs.data?.items ?? []).filter((r) => r.dataset.id === datasetId);
  const shown = `${matching.length} stored run${matching.length === 1 ? '' : 's'}${runs.data?.incomplete ? ' (list incomplete: first runs shown)' : ''}`;
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
          <option value="">{reason ?? shown}</option>
          {matching.map((r) => (
            <option key={r.run_id} value={r.run_id}>
              {r.label ? `${runLabelText(r.label, demoTitles)} \u00b7 ${r.run_id}` : r.run_id}
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
