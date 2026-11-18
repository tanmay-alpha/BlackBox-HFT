// Small pieces shared by the study page and its cell panel: honesty badges, sections, stored-run
// status, the evidence chain and the reproduction block.

import { useState } from 'react';
import type { ReactNode } from 'react';
import { Link } from 'react-router-dom';

import { Badge } from '../../components/ui';
import type { PublicRun } from './contract';
import { runChain, type RunLink } from './links';
import { DATA_CLASS_LABEL, DATA_CLASS_MEANING, NOT_CALIBRATED, type DataClass } from './model';

export function SyntheticBadges() {
  return (
    <span className="row stc-badges">
      <Badge kind="neutral" title="Generated data, not recorded from an exchange">
        Synthetic
      </Badge>
      <Badge kind="modeled" title="Queue position, latency and fills are simulated model outputs">
        Modelled
      </Badge>
      <Badge kind="warn" title={NOT_CALIBRATED}>
        Not calibrated to real markets
      </Badge>
    </span>
  );
}

export function ClassBadge({ dataClass }: { dataClass: DataClass }) {
  return (
    <span className={`stc-class stc-class-${dataClass}`} title={DATA_CLASS_MEANING[dataClass]}>
      {DATA_CLASS_LABEL[dataClass]}
    </span>
  );
}

export function Section({ title, id, children, labels, testId }: { title: string; id: string; children: ReactNode; labels?: ReactNode; testId?: string }) {
  return (
    <section className="stc-section col" aria-labelledby={id} data-testid={testId}>
      <header className="row stc-section-head">
        <h2 id={id} className="stc-h2">
          {title}
        </h2>
        {labels}
      </header>
      {children}
    </section>
  );
}

const NOT_STORED_TEXT: Record<Extract<RunLink, { kind: 'not_stored' }>['reason'], string> = {
  no_stored_run: 'No stored public run has this config hash',
  dataset_mismatch: 'A stored run has this run id but a different dataset hash: not linked',
  unchecked: 'Stored public runs not checked',
};

export function LinkStatus({ link }: { link: RunLink }) {
  if (link.kind === 'stored') {
    return (
      <span className="row">
        <Badge kind="ok" title="The API lists this run id (config SHA-256 prefix) and its stored dataset hash equals the study's">
          Stored: config + dataset hash match
        </Badge>
        {link.resultMatches ? (
          <span className="small dim">result hash identical</span>
        ) : (
          <span className="small dim">result hash differs (other platform class)</span>
        )}
      </span>
    );
  }
  return <span className="small dim">{NOT_STORED_TEXT[link.reason]}</span>;
}

export function Reproduce({ command, open = false, summary = 'Reproduce locally' }: { command: string; open?: boolean; summary?: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <details className="stc-repro" open={open}>
      <summary>{summary}</summary>
      <pre className="mono small stc-pre">{command}</pre>
      <button type="button" className="btn-link" onClick={() => void navigator.clipboard?.writeText(command).then(() => setCopied(true))}>
        {copied ? 'Copied' : 'Copy command'}
      </button>
    </details>
  );
}

export function Chain({ run, link, linkById }: { run: PublicRun; link: RunLink; linkById: (id: string) => RunLink }) {
  const chain = runChain(run, link, linkById);
  if (!chain) return null;
  const fd = run.comparison?.first_divergence;
  return (
    <nav className="row stc-chain small" aria-label={`Evidence for run ${run.run_id}`}>
      <Link to={chain.run}>Run</Link>
      {chain.comparison ? (
        <Link to={chain.comparison}>Comparison</Link>
      ) : run.comparison ? (
        <span className="dim">comparison: reference run not stored</span>
      ) : null}
      {chain.firstDivergence && fd ? <Link to={chain.firstDivergence}>First divergence (order {fd.order_id})</Link> : null}
      {chain.forensics ? <Link to={chain.forensics}>Order forensics</Link> : null}
      <Link to={chain.report}>Report</Link>
    </nav>
  );
}
