// Job presentation shared by the Simulation Workbench and Overview (UI_DESIGN_SPEC.md §10.4,
// §11.5): state pill, progress, heartbeat, and the full job detail with its actions.

import { useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';

import { api } from '../../api/client';
import type { Job } from '../../api/model';
import { DASH, fmtBytes, fmtInt } from '../../components/fmt';
import { fmtNsUtc } from '../../components/ns';
import { ErrorBox, HashText, Notice, Pill } from '../../components/ui';
import { valueLabel } from '../../components/labels';
import { GatedButton } from '../../shell/access';
import { useAnnounce } from '../../shell/announce';
import { paths } from '../../shell/paths';
import { ageSeconds, heartbeatStale, isTerminal, STATE_NOTE, STATE_TONE } from './jobs';

import './sim.css';

/** `47 s`, `1 min 12 s`, `2 h 3 min`. */
export function fmtSeconds(s: number | null): string {
  if (s === null) return DASH;
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.floor(s / 60)} min ${s % 60} s`;
  return `${Math.floor(s / 3600)} h ${Math.floor((s % 3600) / 60)} min`;
}

export const utc = (ns: string | null) => (ns === null ? DASH : fmtNsUtc(ns, 3));

export function JobStatePill({ job }: { job: Job }) {
  return (
    <Pill tone={STATE_TONE[job.state]} title={STATE_NOTE[job.state]}>
      {job.state.toUpperCase()}
    </Pill>
  );
}

/** Queue position as the server reports it; never estimated. */
export function queueText(job: Job): string {
  if (job.state !== 'queued') return DASH;
  if (job.queue_position === null) return 'position unknown';
  return job.queue_depth !== null ? `#${job.queue_position} of ${job.queue_depth}` : `#${job.queue_position}`;
}

/** `ticks 600 / 1,200 · 50%`, or the indeterminate wording when nothing is reported. */
export function progressText(job: Job): string {
  const p = job.progress;
  if (!p) return job.state === 'running' ? 'running \u2014 no progress reported' : DASH;
  const pct = p.total > 0 ? ` \u00b7 ${Math.floor((p.done / p.total) * 100)}%` : '';
  return `${p.unit} ${fmtInt(p.done)} / ${fmtInt(p.total)}${pct}`;
}

export function JobProgress({ job }: { job: Job }) {
  const p = job.progress;
  const text = progressText(job);
  if (!p || p.total <= 0) {
    return <span className="job-progress-text">{text}</span>;
  }
  return (
    <span className="job-progress">
      <span
        className="job-progress-bar"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={p.total}
        aria-valuenow={p.done}
        aria-valuetext={text}
      >
        <span className="job-progress-fill" style={{ width: `${Math.min(100, (p.done / p.total) * 100)}%` }} />
      </span>
      <span className="job-progress-text mono">{text}</span>
    </span>
  );
}

export function heartbeatText(job: Job, nowMs: number): string {
  if (job.state !== 'running' && job.state !== 'cancelling') return DASH;
  const age = ageSeconds(job.last_heartbeat_at_ns, nowMs);
  if (age === null) return 'no heartbeat yet';
  const every = job.heartbeat_interval_ms !== null ? ` (every ${Math.round(job.heartbeat_interval_ms / 1000)} s)` : '';
  return `${fmtSeconds(age)} ago${every}`;
}

/** Re-submits the configuration of the job's experiment (same content address). */
async function resubmit(job: Job) {
  const page = await api.experimentsPage({ run_id: job.experiment_fingerprint.slice(0, 16), limit: 1 });
  const exp = page.items[0];
  if (!exp) throw new Error(`No experiment of this workspace has fingerprint ${job.experiment_fingerprint}; nothing to re-submit.`);
  const full = await api.experiment(exp.id);
  if (!full.config) throw new Error(`Experiment ${exp.id} was returned without its configuration.`);
  return api.submitExperiment(full.config, exp.label === '' ? null : exp.label);
}

export function JobDetail({ job, nowMs, onChanged }: { job: Job; nowMs: number; onChanged: (job: Job | null) => void }) {
  const navigate = useNavigate();
  const announce = useAnnounce();
  const [error, setError] = useState<unknown>(null);
  const [pending, setPending] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const stale = heartbeatStale(job, nowMs);
  const started = ageSeconds(job.started_at_ns, nowMs);
  const runIds = job.run_ids.length > 0 ? job.run_ids : job.run_id ? [job.run_id] : [];
  const cancellable = !isTerminal(job.state) && job.state !== 'cancelling';
  const resubmittable = job.state === 'failed' || job.state === 'cancelled' || job.state === 'expired';

  const cancel = async () => {
    setPending(true);
    setError(null);
    try {
      const updated = await api.cancelJob(job.id);
      const msg = updated.detached
        ? `Your experiment was detached from job ${job.id.slice(0, 12)}; it continues for another workspace.`
        : `Cancel requested for job ${job.id.slice(0, 12)}: ${updated.state}.`;
      setNote(msg);
      announce(msg);
      onChanged(updated);
    } catch (e) {
      setError(e);
    } finally {
      setPending(false);
    }
  };

  const again = async () => {
    setPending(true);
    setError(null);
    try {
      const r = await resubmit(job);
      if (r.job) {
        announce(`Re-submitted: job ${r.job.id.slice(0, 12)} ${r.job.state}${r.created ? '' : ' (existing live job returned)'}.`);
        navigate(paths.job(r.job.id));
      } else if (r.run) {
        announce(`Already simulated \u2014 stored run ${r.run.run_id} returned.`);
        navigate(paths.run(r.run.run_id));
      }
    } catch (e) {
      setError(e);
    } finally {
      setPending(false);
    }
  };

  return (
    <div className="col job-detail" data-testid="job-detail" data-job-state={job.state}>
      <div className="row">
        <JobStatePill job={job} />
        <span className="note">{STATE_NOTE[job.state]}</span>
      </div>
      {stale ? (
        <Notice tag="STALE HEARTBEAT">
          The worker has not reported for {fmtSeconds(ageSeconds(job.last_heartbeat_at_ns, nowMs))}. The job may be stuck or the host may
          be restarting.
        </Notice>
      ) : null}
      {job.error ? (
        <div className="error-box" role="alert" data-error-state={job.error.category}>
          <div className="error-box-head">
            FAILED {job.error.category} {'\u00b7'} {job.error.code}
          </div>
          <div className="error-box-message">{job.error.message}</div>
        </div>
      ) : null}
      {job.state === 'expired' ? (
        <Notice tag="EXPIRED">{job.expired_reason ?? 'The server gave no reason.'}</Notice>
      ) : null}
      <dl className="kv">
        <dt>Job</dt>
        <dd className="mono wrap-anywhere">{job.id}</dd>
        <dt>Kind</dt>
        <dd title={job.kind}>{valueLabel(job.kind)}</dd>
        <dt>Experiment</dt>
        <dd>
          <HashText hash={job.experiment_fingerprint} chars={16} />
        </dd>
        <dt>Platform</dt>
        <dd className="mono">{job.platform}</dd>
        <dt>Queue</dt>
        <dd className="mono">{queueText(job)}</dd>
        <dt>Progress</dt>
        <dd>
          <JobProgress job={job} />
          {job.progress?.tape_events !== null && job.progress?.tape_events !== undefined && !(job.state === 'completed' && job.progress.tape_events === 0) ? (
            <span className="note"> tape events {fmtInt(job.progress.tape_events)}</span>
          ) : null}
        </dd>
        <dt>Heartbeat</dt>
        <dd className="mono">{heartbeatText(job, nowMs)}</dd>
        <dt>Attempt</dt>
        <dd className="mono">
          {job.attempt} of {job.max_attempts}
          {job.lease_losses > 0 ? ` \u00b7 ${job.lease_losses} lease loss${job.lease_losses === 1 ? '' : 'es'} (re-queued)` : ''}
        </dd>
        <dt>Created</dt>
        <dd className="mono">{utc(job.created_at_ns)}</dd>
        <dt>Queued</dt>
        <dd className="mono">{utc(job.queued_at_ns)}</dd>
        <dt>Started</dt>
        <dd className="mono">
          {utc(job.started_at_ns)}
          {job.state === 'running' && started !== null ? ` \u00b7 elapsed ${fmtSeconds(started)}` : ''}
        </dd>
        <dt>Finished</dt>
        <dd className="mono">{utc(job.finished_at_ns)}</dd>
        {job.cancel_requested_at_ns !== null ? (
          <>
            <dt>Cancel requested</dt>
            <dd className="mono">{utc(job.cancel_requested_at_ns)}</dd>
          </>
        ) : null}
        <dt>Run</dt>
        <dd>
          {runIds.length === 0
            ? DASH
            : runIds.map((id) => (
                <span key={id} className="row job-run-links">
                  <Link className="mono" to={paths.run(id)}>
                    {id}
                  </Link>
                  <Link to={paths.riskRun(id)}>Risk</Link>
                  <Link to={paths.report(id)}>Report</Link>
                  <Link to={paths.comparePair(id)}>Compare</Link>
                </span>
              ))}
        </dd>
      </dl>
      {note ? <p className="note">{note}</p> : null}
      <ErrorBox error={error} />
      <div className="row">
        {cancellable ? (
          <GatedButton onClick={() => void cancel()} blockedReason={pending ? 'Working\u2026' : null} data-testid="cancel-job">
            Cancel job
          </GatedButton>
        ) : null}
        {resubmittable ? (
          <GatedButton onClick={() => void again()} blockedReason={pending ? 'Working\u2026' : null} data-testid="resubmit-job">
            Re-submit
          </GatedButton>
        ) : null}
        {job.error ? (
          <button
            type="button"
            onClick={() => void navigator.clipboard?.writeText(`job ${job.id}\n${job.error?.category} ${job.error?.code}\n${job.error?.message}`)}
          >
            Copy details
          </button>
        ) : null}
      </div>
    </div>
  );
}
