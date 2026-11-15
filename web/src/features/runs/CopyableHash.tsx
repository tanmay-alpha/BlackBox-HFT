// A full digest or id with a copy button. Hashes live in a run's metadata, not in its display name,
// so where they are shown they are complete and copyable.

import { useState } from 'react';

export function CopyableHash({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <span className="runs-copy">
      <span className="mono wrap-anywhere">{value}</span>
      <button type="button" className="btn-link" onClick={() => void navigator.clipboard?.writeText(value).then(() => setCopied(true))} aria-label={`Copy ${label}`}>
        {copied ? 'Copied' : 'Copy'}
      </button>
    </span>
  );
}
