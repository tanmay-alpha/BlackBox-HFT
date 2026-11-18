import { describe, expect, it } from 'vitest';

import { candidateRunIds, listedDemoRuns, matchRun, reproduceCommand, runChain, type StoredRunFacts } from './links';
import { ANCHOR_IDS, BASE_ID, makeStudy } from './study.fixtures';

const study = makeStudy();
const [reference, heavy] = study.demo_anchors.runs;

const demos = [
  {
    id: 'execution_validation',
    runs: [
      { label: 'Normal-liquidity baseline', run_id: ANCHOR_IDS[0] },
      { label: 'Heavy-tail order latency', run_id: ANCHOR_IDS[1] },
      { label: 'Not simulated yet', run_id: null },
    ],
  },
];

const facts = (runId: string, datasetSha: string, resultSha: string): StoredRunFacts => ({
  run_id: runId,
  result_sha256: resultSha,
  dataset: { id: 'synthetic-demo', events_sha256: datasetSha },
});

const storedBoth = new Map([
  [ANCHOR_IDS[0], facts(ANCHOR_IDS[0], reference.dataset.sha256, reference.result_sha256)],
  [ANCHOR_IDS[1], facts(ANCHOR_IDS[1], heavy.dataset.sha256, '0'.repeat(64))],
]);

describe('listedDemoRuns and candidateRunIds', () => {
  it('keeps simulated demo runs and fetches only study runs a demo lists', () => {
    const listed = listedDemoRuns(demos);
    expect(listed.map((l) => l.runId)).toEqual(ANCHOR_IDS);
    expect(candidateRunIds([...study.cells, ...study.demo_anchors.runs], listed)).toEqual(ANCHOR_IDS);
  });
});

describe('matchRun', () => {
  const listed = listedDemoRuns(demos);

  it('links when the run id (config hash prefix) and the dataset hash both match', () => {
    expect(matchRun(reference, listed, storedBoth)).toEqual({
      kind: 'stored',
      runId: ANCHOR_IDS[0],
      demoId: 'execution_validation',
      demoLabel: 'Normal-liquidity baseline',
      resultMatches: true,
    });
    // Same inputs, other platform class: still the same experiment, flagged.
    expect(matchRun(heavy, listed, storedBoth)).toMatchObject({ kind: 'stored', resultMatches: false });
  });

  it('never links on a dataset hash mismatch', () => {
    const stored = new Map([[ANCHOR_IDS[0], facts(ANCHOR_IDS[0], '9'.repeat(64), reference.result_sha256)]]);
    expect(matchRun(reference, listed, stored)).toEqual({ kind: 'not_stored', reason: 'dataset_mismatch' });
  });

  it('never guesses from labels or dataset ids', () => {
    // A study cell on its own dataset with the same label text as a demo run is not stored.
    const cell = { ...study.cells[0], label: 'Normal-liquidity baseline' };
    expect(matchRun(cell, listed, storedBoth)).toEqual({ kind: 'not_stored', reason: 'no_stored_run' });
    // A listed id whose stored facts were not read stays unlinked.
    expect(matchRun(reference, listed, new Map())).toEqual({ kind: 'not_stored', reason: 'unchecked' });
    // A run id that is not the config hash prefix is not a match.
    expect(matchRun({ ...reference, config_sha256: 'c'.repeat(64) }, listed, storedBoth)).toEqual({ kind: 'not_stored', reason: 'no_stored_run' });
  });

  it('reports unchecked when the demo list could not be read', () => {
    expect(matchRun(reference, null, storedBoth)).toEqual({ kind: 'not_stored', reason: 'unchecked' });
  });
});

describe('runChain', () => {
  const listed = listedDemoRuns(demos);
  const linkOf = (id: string) => {
    const run = study.demo_anchors.runs.find((r) => r.run_id === id);
    return run ? matchRun(run, listed, storedBoth) : ({ kind: 'not_stored', reason: 'no_stored_run' } as const);
  };

  it('links run, comparison, first divergence, order forensics and report', () => {
    expect(runChain(heavy, linkOf(heavy.run_id), linkOf)).toEqual({
      run: `/forensics/${ANCHOR_IDS[1]}`,
      comparison: `/compare/pair?a=${ANCHOR_IDS[0]}&b=${ANCHOR_IDS[1]}`,
      firstDivergence: `/book/runs/${ANCHOR_IDS[1]}?t=900000000&order=42`,
      forensics: `/forensics/${ANCHOR_IDS[1]}/orders/42`,
      report: `/reports/${ANCHOR_IDS[1]}`,
    });
  });

  it('omits the comparison when the reference run is not stored', () => {
    const onlyHeavy = new Map([[ANCHOR_IDS[1], storedBoth.get(ANCHOR_IDS[1])!]]);
    const link = (id: string) => {
      const run = study.demo_anchors.runs.find((r) => r.run_id === id)!;
      return matchRun(run, listed, onlyHeavy);
    };
    expect(runChain(heavy, link(heavy.run_id), link)).toMatchObject({ comparison: null, firstDivergence: null, forensics: null });
  });

  it('has no chain for a run that is not stored', () => {
    expect(runChain(study.cells[0], { kind: 'not_stored', reason: 'no_stored_run' }, linkOf)).toBeNull();
  });
});

describe('reproduceCommand', () => {
  it('names the study command, the output file and the hashes to expect', () => {
    const cell = study.cells.find((c) => c.run_id === BASE_ID)!;
    const text = reproduceCommand(study, cell, { detail: cell.detail });
    expect(text.split('\n')[0]).toBe('blackbox study exec-validation --out data/studies/exec-validation-quick --quick');
    expect(text).toContain(`data/studies/exec-validation-quick/cells/${BASE_ID}.json`);
    expect(text).toContain(cell.config_sha256);
    expect(reproduceCommand(study, heavy, { anchorIndex: 1 })).toContain('demo_anchors.runs[1] in data/studies/exec-validation-quick/public/study-result.json');
  });
});
