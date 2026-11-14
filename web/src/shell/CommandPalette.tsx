// Command palette (UI_DESIGN_SPEC.md §6.12, Ctrl+K): one combobox over workspaces, datasets, runs
// and commands. Matching is a client-side subsequence match over lists the API already returned.
// Every single-key shortcut also exists here, so turning single keys off loses no function.

import { useEffect, useId, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent } from 'react';
import { useNavigate } from 'react-router-dom';

import { api } from '../api/client';
import type { DatasetManifest, RunSummary } from '../api/types';
import { Dialog } from './Dialog';
import { paths } from './paths';
import { WORKSPACES } from './workspaces';

export interface PaletteCommand {
  id: string;
  label: string;
  hint?: string;
  run: () => void;
}

interface Item {
  id: string;
  group: string;
  label: string;
  hint?: string;
  run: () => void;
}

function matches(text: string, query: string): boolean {
  const t = text.toLowerCase();
  let i = 0;
  for (const ch of query.toLowerCase()) {
    if (ch === ' ') continue;
    i = t.indexOf(ch, i);
    if (i < 0) return false;
    i += 1;
  }
  return true;
}

export function CommandPalette({ open, onClose, commands }: { open: boolean; onClose: () => void; commands: PaletteCommand[] }) {
  const navigate = useNavigate();
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const [datasets, setDatasets] = useState<DatasetManifest[]>([]);
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const listId = useId();
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setQuery('');
    setActive(0);
    let live = true;
    // Lists are loaded when the palette opens; a failure leaves them out (the status bar reports the API).
    api.datasets().then((d) => live && setDatasets(d), () => undefined);
    api.runs().then((r) => live && setRuns(r), () => undefined);
    requestAnimationFrame(() => inputRef.current?.focus());
    return () => {
      live = false;
    };
  }, [open]);

  const go = (to: string) => () => navigate(to);

  const items = useMemo<Item[]>(() => {
    const all: Item[] = [
      ...WORKSPACES.map((w) => ({ id: `ws-${w.id}`, group: 'Workspaces', label: w.name, hint: `g ${w.chord}`, run: go(w.path) })),
      ...datasets.map((d) => ({ id: `ds-${d.id}`, group: 'Datasets', label: d.id, hint: d.kind, run: go(paths.dataset(d.id)) })),
      ...runs.map((r) => ({
        id: `run-${r.manifest.run_id}`,
        group: 'Runs',
        label: `${r.manifest.run_id.slice(0, 12)} ${r.label}`,
        hint: r.manifest.config.strategy.kind.type,
        run: go(paths.run(r.manifest.run_id)),
      })),
      ...commands.map((c) => ({ id: `cmd-${c.id}`, group: 'Commands', label: c.label, hint: c.hint, run: c.run })),
    ];
    return query.trim() === '' ? all : all.filter((it) => matches(`${it.label} ${it.hint ?? ''}`, query));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [datasets, runs, commands, query]);

  const choose = (it: Item | undefined) => {
    if (!it) return;
    onClose();
    it.run();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown') setActive((a) => Math.min(items.length - 1, a + 1));
    else if (e.key === 'ArrowUp') setActive((a) => Math.max(0, a - 1));
    else if (e.key === 'Enter') choose(items[active]);
    else return;
    e.preventDefault();
  };

  useEffect(() => {
    document.getElementById(`${listId}-${active}`)?.scrollIntoView({ block: 'nearest' });
  }, [active, listId]);

  let lastGroup = '';
  return (
    <Dialog open={open} onClose={onClose} title="Command palette" className="palette">
      <input
        ref={inputRef}
        type="text"
        role="combobox"
        aria-expanded="true"
        aria-controls={listId}
        aria-autocomplete="list"
        aria-activedescendant={items[active] ? `${listId}-${active}` : undefined}
        aria-label="Search or run a command"
        placeholder="Search or run a command"
        className="palette-input"
        value={query}
        onChange={(e) => {
          setQuery(e.target.value);
          setActive(0);
        }}
        onKeyDown={onKeyDown}
      />
      <ul id={listId} role="listbox" aria-label="Results" className="palette-list">
        {items.length === 0 ? <li className="dim palette-empty">No match.</li> : null}
        {items.map((it, i) => {
          const header = it.group !== lastGroup ? it.group : null;
          lastGroup = it.group;
          return (
            <li key={it.id} role="presentation">
              {header ? (
                <div className="palette-group" aria-hidden="true">
                  {header}
                </div>
              ) : null}
              <div
                id={`${listId}-${i}`}
                role="option"
                aria-selected={i === active}
                className="palette-option"
                onPointerMove={() => setActive(i)}
                onClick={() => choose(it)}
              >
                <span className="palette-label">{it.label}</span>
                {it.hint ? <span className="palette-hint mono">{it.hint}</span> : null}
              </div>
            </li>
          );
        })}
      </ul>
    </Dialog>
  );
}
