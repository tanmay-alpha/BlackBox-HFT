// Failures in plain language for the public experiment page. The server's exact error (status,
// code, message, request id) stays available in a collapsed technical disclosure.

import type { ReactNode } from 'react';

import { ApiError } from '../../api/client';
import { fmtBytes } from '../../components/fmt';
import { ErrorBox } from '../../components/ui';

interface Plain {
  title: string;
  body: ReactNode;
  /** Trying again right away can help. */
  retry: boolean;
}

/** When the daily allowance resets: the server sends the seconds until 00:00 UTC. */
function resetText(retryAfterS: number | null): string {
  if (retryAfterS === null) return 'It resets at midnight UTC.';
  const at = new Date(Date.now() + retryAfterS * 1000);
  const local = at.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  const hours = retryAfterS / 3600;
  const inText = hours >= 1 ? `in about ${Math.round(hours)} hour${Math.round(hours) === 1 ? '' : 's'}` : `in about ${Math.max(1, Math.round(retryAfterS / 60))} minutes`;
  return `It resets at midnight UTC (${local} your time, ${inText}).`;
}

export function plainError(error: unknown): Plain {
  if (!(error instanceof ApiError)) {
    return { title: 'Something went wrong while showing this part of the experiment.', body: 'Try again; if it keeps failing, the technical details below say what happened.', retry: true };
  }
  const d = error.details ?? {};
  switch (error.code) {
    case 'unavailable':
      if (d.budget === 'egress') {
        return {
          title: 'The public demo’s data allowance for today is used up.',
          body: `${resetText(error.retryAfterS)} Everything already on screen stays available; views that need new data load again after the reset, or right away with an API key.`,
          retry: false,
        };
      }
      return { title: 'The data server is busy or briefly unavailable.', body: 'Try again in a moment.', retry: true };
    case 'response_too_large':
      return {
        title: 'This view needs more data than the public demo sends in one response.',
        body: `${typeof d.limit_bytes === 'number' ? `Visitors without an API key get at most ${fmtBytes(d.limit_bytes)} per response. ` : ''}Choose a shorter time window, or use the research console with an API key.`,
        retry: false,
      };
    case 'waking':
      return {
        title: 'The data server is starting up.',
        body: 'The free host sleeps when idle; the first response can take about a minute. Try again shortly.',
        retry: true,
      };
    case 'network':
      return { title: 'The data server could not be reached.', body: 'Check your connection and try again.', retry: true };
    case 'rate_limited':
      return { title: 'Too many requests in a short time.', body: 'Wait a few seconds and try again.', retry: true };
    case 'not_found':
      return { title: 'This part of the stored experiment was not found.', body: 'It may have been removed from the public demo.', retry: false };
    default:
      return { title: 'The server could not return this part of the experiment.', body: 'Try again; the technical details below say what happened.', retry: true };
  }
}

export function PlainError({ error, onRetry, what }: { error: unknown; onRetry?: () => void; what?: string }) {
  const p = plainError(error);
  return (
    <div className="ex-error" role="alert">
      <p>
        {what ? <span className="ex-error-what">{what}: </span> : null}
        <strong>{p.title}</strong> {p.body}
      </p>
      {onRetry && p.retry ? (
        <button type="button" onClick={onRetry}>
          Try again
        </button>
      ) : null}
      <details className="ex-details">
        <summary>Technical details</summary>
        <ErrorBox error={error} />
      </details>
    </div>
  );
}
