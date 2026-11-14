// Access mode (UI_DESIGN_SPEC.md §10.2, §10.3; API_CONTRACT.md §3): DEMO (no key, the public demo
// workspace, read-only) or RESEARCH (a key the server accepted). `GET /whoami` is the single
// authenticated read that validates a key and reports workspace, scopes and quotas. The key
// itself is handled only by `api/auth.ts` (sessionStorage of this tab).
//
// Mutating controls stay rendered and focusable with `aria-disabled` and the reason; activating
// one without the needed scope opens the key dialog instead of acting.

import { createContext, useCallback, useContext, useEffect, useId, useMemo, useState } from 'react';
import type { ButtonHTMLAttributes, ReactNode } from 'react';
import { Lock } from 'lucide-react';

import { clearApiKey, getApiKey, setApiKey, API_KEY_PATTERN, subscribeApiKey } from '../api/auth';
import { api, ApiError } from '../api/client';
import { setUnauthorizedHandler } from '../api/http';
import type { Scope, Whoami } from '../api/model';
import { ErrorBox } from '../components/ui';
import { Dialog } from './Dialog';

export const DEMO_REASON = 'Requires an API key \u2014 the public demo is read-only.';

interface AccessState {
  /** `null` until the first whoami answers (or when it failed). */
  whoami: Whoami | null;
  /** The whoami request failed; the mode is then unknown, not demo. */
  error: Error | null;
  loading: boolean;
}

interface AccessApi extends AccessState {
  mode: 'demo' | 'research' | 'unknown';
  hasScope: (scope: Scope) => boolean;
  /** Why an action needing `scope` is unavailable, or `null` when it is allowed. */
  reasonFor: (scope: Scope) => string | null;
  openKeyDialog: () => void;
  signOut: () => void;
  refresh: () => void;
}

const AccessContext = createContext<AccessApi | null>(null);

export function AccessProvider({ children, announce }: { children: ReactNode; announce: (text: string) => void }) {
  const [state, setState] = useState<AccessState>({ whoami: null, error: null, loading: true });
  const [dialogOpen, setDialogOpen] = useState(false);
  const [keyVersion, setKeyVersion] = useState(0);

  useEffect(() => subscribeApiKey(() => setKeyVersion((v) => v + 1)), []);

  useEffect(() => {
    setUnauthorizedHandler(() => {
      announce('API key refused (401). It was removed from this tab.');
      setDialogOpen(true);
    });
    return () => setUnauthorizedHandler(null);
  }, [announce]);

  useEffect(() => {
    const ctl = new AbortController();
    setState((s) => ({ ...s, loading: true }));
    api.whoami({ signal: ctl.signal }).then(
      (whoami) => setState({ whoami, error: null, loading: false }),
      (e: unknown) => {
        if (ctl.signal.aborted) return;
        setState({ whoami: null, error: e instanceof Error ? e : new Error(String(e)), loading: false });
      },
    );
    return () => ctl.abort();
  }, [keyVersion]);

  const signOut = useCallback(() => {
    clearApiKey();
    announce('Signed out. The key was removed from this tab; the console is in DEMO read-only mode.');
  }, [announce]);

  const value = useMemo<AccessApi>(() => {
    const w = state.whoami;
    const scopes = w?.key?.scopes ?? [];
    const mode = w ? w.mode : 'unknown';
    const hasScope = (scope: Scope) => mode === 'research' && (scopes.includes(scope) || scopes.includes('admin'));
    return {
      ...state,
      mode,
      hasScope,
      reasonFor: (scope) => {
        if (hasScope(scope)) return null;
        if (mode === 'research') return `This key lacks the \u201c${scope}\u201d scope (it has ${scopes.join(', ') || 'none'}).`;
        if (mode === 'unknown' && getApiKey()) return 'Checking the API key\u2026';
        return DEMO_REASON;
      },
      openKeyDialog: () => setDialogOpen(true),
      signOut,
      refresh: () => setKeyVersion((v) => v + 1),
    };
  }, [state, signOut]);

  return (
    <AccessContext.Provider value={value}>
      {children}
      <KeyDialog
        open={dialogOpen}
        onClose={() => setDialogOpen(false)}
        onAccepted={(w) => {
          setDialogOpen(false);
          announce(`Research access: workspace ${w.workspace.slug}, scopes ${w.key?.scopes.join(', ') ?? ''}.`);
        }}
      />
    </AccessContext.Provider>
  );
}

export function useAccess(): AccessApi {
  const a = useContext(AccessContext);
  if (!a) throw new Error('useAccess outside AccessProvider');
  return a;
}

// ----- key dialog ------------------------------------------------------------------------------

function KeyDialog({ open, onClose, onAccepted }: { open: boolean; onClose: () => void; onAccepted: (w: Whoami) => void }) {
  const [value, setValue] = useState('');
  const [show, setShow] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [pending, setPending] = useState(false);
  const inputId = useId();
  const hintId = useId();

  useEffect(() => {
    if (!open) {
      setValue('');
      setShow(false);
      setError(null);
      setPending(false);
    }
  }, [open]);

  const submit = async () => {
    const key = value.trim();
    if (!API_KEY_PATTERN.test(key)) {
      setError(new ApiError('invalid_request', 'An API key is bbk_ followed by 43 characters (A-Z, a-z, 0-9, _ or -).', 0));
      return;
    }
    setPending(true);
    setError(null);
    try {
      const w = await api.whoami({ key });
      if (w.mode !== 'research' || !w.key) {
        throw new ApiError('unauthorized', 'The server did not accept this key as a research key.', 401);
      }
      setApiKey(key);
      setValue('');
      onAccepted(w);
    } catch (e) {
      setError(e);
    } finally {
      setPending(false);
    }
  };

  return (
    <Dialog open={open} onClose={onClose} title="Research access" className="key-dialog">
      <form
        className="col"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <p id={hintId}>
          Paste an API key to create experiments, run sweeps and cancel jobs. The key is kept in this browser tab only
          (sessionStorage) and is sent to the BlackBox API with each request. It is never put in a URL or saved to disk by
          this page.
        </p>
        <label htmlFor={inputId}>API key</label>
        <div className="row">
          <input
            id={inputId}
            className="mono key-input"
            type={show ? 'text' : 'password'}
            autoComplete="off"
            spellCheck={false}
            aria-describedby={hintId}
            value={value}
            onChange={(e) => setValue(e.target.value)}
            placeholder="bbk_…"
          />
          <button type="button" onClick={() => setShow((s) => !s)} aria-pressed={show}>
            {show ? 'Hide' : 'Show'}
          </button>
        </div>
        {error ? <ErrorBox error={error} /> : null}
        <div className="row dialog-actions">
          <button type="button" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="btn-primary" disabled={pending || value.trim() === ''}>
            {pending ? 'Checking\u2026' : 'Use key'}
          </button>
        </div>
      </form>
    </Dialog>
  );
}

// ----- gated controls --------------------------------------------------------------------------

export interface GatedButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'onClick'> {
  /** Minimum scope the action needs (API_CONTRACT §3). */
  scope?: Scope;
  onClick: () => void;
  /** A further, non-access reason the action is unavailable (`null` = none). */
  blockedReason?: string | null;
  children: ReactNode;
}

/**
 * A mutating control (§10.3). Without the scope it stays rendered and focusable with
 * `aria-disabled`, a lock icon and the reason; activating it opens the key dialog.
 */
export function GatedButton({ scope = 'run', onClick, blockedReason = null, children, className, title, ...rest }: GatedButtonProps) {
  const access = useAccess();
  const accessReason = access.reasonFor(scope);
  const reason = accessReason ?? blockedReason;
  const locked = accessReason !== null;
  return (
    <button
      type="button"
      {...rest}
      className={[className, locked ? 'btn-locked' : null].filter(Boolean).join(' ') || undefined}
      aria-disabled={reason !== null || rest.disabled ? true : undefined}
      disabled={locked ? undefined : rest.disabled}
      title={reason ?? title}
      data-locked={locked ? 'true' : undefined}
      onClick={() => {
        if (locked) {
          if (access.mode !== 'research') access.openKeyDialog();
          return;
        }
        if (reason !== null) return;
        onClick();
      }}
    >
      {locked ? <Lock size={12} strokeWidth={1.5} aria-hidden="true" /> : null}
      {children}
      {locked ? <span className="visually-hidden">{` (${accessReason})`}</span> : null}
    </button>
  );
}
