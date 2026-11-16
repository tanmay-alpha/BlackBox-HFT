// What the Lab says about its own progress: the four steps' states in the step bar, and the
// submitted job's phase after "Run simulation". Pure functions so the wording rules are tested:
// a queued job is never described as already simulated (PHASE3_REVIEW.md D1), and the Run step
// never reads "ready" while the configuration is invalid (D2).

import type { SubmitExperimentResult } from '../../api/model';

export type Job = NonNullable<SubmitExperimentResult['job']>;

/** Where a submission stands. `stored`: no job, the server returned an earlier identical run. */
export type JobPhase = 'queued' | 'running' | 'completed' | 'failed' | 'stored' | 'none';

export interface JobView {
  job: Job | null;
  phase: JobPhase;
  /** The run's views exist (a stored run, or the job completed). */
  ready: boolean;
  /** One or two words for the state, e.g. "Queued", "Cancelled". */
  headline: string;
  /** One honest sentence about what has and has not happened. */
  detail: string;
}

/**
 * The job to show: the submit response names it, the poll only refreshes it. A polled job with
 * another id (an earlier submission's, or not fetched yet) is ignored, so the response's own job
 * is shown until the first poll for it lands.
 */
export function currentJob(submitted: Job | null, polled: Job | null): Job | null {
  return submitted !== null && polled?.id === submitted.id ? polled : submitted;
}

/** The phase and wording for a submission and the latest poll of its job. */
export function jobView(result: SubmitExperimentResult, polled: Job | null): JobView {
  const job = currentJob(result.job, polled);
  const runId = result.experiment.run_id;
  if (job === null) {
    if (result.run !== null) {
      return {
        job,
        phase: 'stored',
        ready: true,
        headline: 'Already simulated',
        detail: `This exact configuration was simulated before: stored run ${result.run.run_id} was returned and no new job was queued.`,
      };
    }
    return {
      job,
      phase: 'none',
      ready: false,
      headline: 'No job',
      detail: `No job was returned; experiment ${result.experiment.id} is ${result.experiment.status}.`,
    };
  }
  const shared = result.created ? '' : 'The same experiment was already submitted, so its existing job is followed (the first label is kept). ';
  switch (job.state) {
    case 'created':
    case 'queued': {
      const pos =
        job.queue_position !== null
          ? ` (position ${job.queue_position}${job.queue_depth !== null && job.queue_depth !== undefined ? ` of ${job.queue_depth}` : ''})`
          : '';
      return {
        job,
        phase: 'queued',
        ready: false,
        headline: 'Queued',
        detail: `${shared}Waiting for a worker${pos}. Nothing has been simulated yet.`,
      };
    }
    case 'running':
    case 'cancelling': {
      const p = job.progress;
      const done = p && p.total > 0 ? ` ${p.done.toLocaleString('en-US')} of ${p.total.toLocaleString('en-US')} ${p.unit} done.` : '';
      return {
        job,
        phase: 'running',
        ready: false,
        headline: job.state === 'cancelling' ? 'Cancelling' : 'Running',
        detail:
          job.state === 'cancelling'
            ? `${shared}A cancel was requested; the worker stops at its next check and stores no run.`
            : `${shared}A worker is simulating this configuration now.${done}`,
      };
    }
    case 'completed':
      return {
        job,
        phase: 'completed',
        ready: true,
        headline: 'Completed',
        detail: `${shared}The simulation finished and run ${job.run_id ?? runId} is stored.`,
      };
    default: {
      const why = job.error?.message ?? job.expired_reason ?? null;
      return {
        job,
        phase: 'failed',
        ready: false,
        headline: job.state === 'cancelled' ? 'Cancelled' : job.state === 'expired' ? 'Expired' : 'Failed',
        detail: `${shared}The job ${job.state} and produced no run.${why ? ` ${why}` : ''}`,
      };
    }
  }
}

export type StageState = 'done' | 'current' | 'todo' | 'error' | 'skipped';

/** Queued → Running → Completed (the last renamed to the end state when the job did not complete). */
export function jobStages(view: JobView): { label: string; state: StageState }[] {
  const started = view.job?.started_at_ns !== null && view.job?.started_at_ns !== undefined;
  switch (view.phase) {
    case 'queued':
      return [
        { label: 'Queued', state: 'current' },
        { label: 'Running', state: 'todo' },
        { label: 'Completed', state: 'todo' },
      ];
    case 'running':
      return [
        { label: 'Queued', state: 'done' },
        { label: view.headline, state: 'current' },
        { label: 'Completed', state: 'todo' },
      ];
    case 'completed':
      return [
        { label: 'Queued', state: 'done' },
        { label: 'Running', state: 'done' },
        { label: 'Completed', state: 'done' },
      ];
    case 'failed':
      return [
        { label: 'Queued', state: 'done' },
        { label: 'Running', state: started ? 'done' : 'skipped' },
        { label: view.headline, state: 'error' },
      ];
    default:
      return [];
  }
}

export type StepId = 'lab-step-choose' | 'lab-step-configure' | 'lab-step-review' | 'lab-step-run';
export type StepState = 'done' | 'open' | 'error';

export interface StepInput {
  origin: 'defaults' | 'preset' | 'run';
  /** Settings that differ from the starting point. */
  edits: number;
  /** Blocking problems (one per field plus uncovered rules). */
  problems: number;
  /** Phase of the last submission, when the form still holds what was submitted; else null. */
  submitted: JobPhase | null;
}

/** Each step's short status and state for the step bar. */
export function stepStates(s: StepInput): Record<StepId, { status: string; state: StepState }> {
  const blocked = s.problems > 0;
  let run: { status: string; state: StepState };
  if (blocked) run = { status: 'blocked', state: 'open' };
  else if (s.submitted === null) run = { status: 'ready', state: 'open' };
  else if (s.submitted === 'completed' || s.submitted === 'stored') run = { status: s.submitted === 'stored' ? 'stored run' : 'completed', state: 'done' };
  else if (s.submitted === 'failed') run = { status: 'failed', state: 'error' };
  else run = { status: s.submitted === 'none' ? 'submitted' : s.submitted, state: 'open' };
  return {
    'lab-step-choose': { status: s.origin === 'preset' ? 'preset' : s.origin === 'run' ? 'from run' : 'defaults', state: 'done' },
    'lab-step-configure': { status: `${s.edits} edit${s.edits === 1 ? '' : 's'}`, state: blocked ? 'error' : 'done' },
    'lab-step-review': blocked ? { status: `${s.problems} to fix`, state: 'error' } : { status: 'valid', state: 'done' },
    'lab-step-run': run,
  };
}
