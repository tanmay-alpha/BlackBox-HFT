// The workstation shell (UI_DESIGN_SPEC.md §5): top bar, workspace rail, workspace area with an
// optional resizable inspector, status bar, command palette, help, and the global keyboard model
// (§8.1). Browser- and OS-reserved bindings are never taken; Ctrl+K is the only modified chord
// taken over, and single-key shortcuts can be turned off and never fire while typing.

import { Component, useCallback, useEffect, useRef, useState } from 'react';
import type { ErrorInfo, KeyboardEvent as ReactKeyboardEvent, ReactNode } from 'react';
import { Link, NavLink, Outlet, useLocation, useNavigate } from 'react-router-dom';
import { Group, Panel, Separator } from 'react-resizable-panels';
import { ChevronDown, CircleHelp, PanelRightClose, PanelRightOpen, Search } from 'lucide-react';

import { isStaticViewer } from '../api/client';
import { STATIC_VIEWER_BANNER } from '../components/staticViewer';
import { BasisLegend, ErrorBox } from '../components/ui';
import { GLOSSARY } from '../components/glossary';
import { fmtClock } from '../components/fmt';
import { CommandPalette, type PaletteCommand } from './CommandPalette';
import { HelpDialog } from './HelpDialog';
import { HealthProvider } from './health';
import { InspectorProvider, useInspectorContent } from './inspector';
import { paths } from './paths';
import { SettingsProvider, useSettings } from './settings';
import { StatusBar } from './StatusBar';
import { WORKSPACES, workspaceFor } from './workspaces';

const ICON = { size: 16, strokeWidth: 1.5, 'aria-hidden': true } as const;

// ----- error boundary ---------------------------------------------------------------------

/** Keeps a rendering failure in one workspace from blanking the console. */
class WorkspaceErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: unknown): { error: Error } {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error('Workspace failed to render', error, info.componentStack);
  }

  render(): ReactNode {
    if (this.state.error) {
      return (
        <div className="col pad-3">
          <ErrorBox error={this.state.error} />
          <p className="note">
            This workspace failed to render. Nothing was simulated or changed by this failure. Reload, or go to{' '}
            <a href="/">Overview</a>.
          </p>
        </div>
      );
    }
    return this.props.children;
  }
}

// ----- top bar ----------------------------------------------------------------------------

interface Crumb {
  label: string;
  title: string;
  to: string;
}

/** `dataset > run > order > t` for whatever the URL names (§5.2). */
function useBreadcrumb(): Crumb[] {
  const { pathname, search } = useLocation();
  const q = new URLSearchParams(search);
  const seg = pathname.split('/').filter(Boolean).map(decodeURIComponent);
  const crumbs: Crumb[] = [];
  const dataset = (id: string) => crumbs.push({ label: id, title: `Dataset ${id}`, to: paths.dataset(id) });
  const run = (id: string) => crumbs.push({ label: `run ${id.slice(0, 12)}`, title: `Run ${id}`, to: paths.run(id) });
  if (seg[0] === 'data' && seg[1]) dataset(seg[1]);
  if (seg[0] === 'book' && seg[1] === 'data' && seg[2]) dataset(seg[2]);
  if (seg[0] === 'book' && seg[1] === 'runs' && seg[2]) run(seg[2]);
  if (seg[0] === 'forensics' && seg[1]) run(seg[1]);
  if (seg[0] === 'forensics' && seg[2] === 'orders' && seg[3]) {
    crumbs.push({ label: `order ${seg[3]}`, title: `Order ${seg[3]}`, to: paths.order(seg[1], seg[3]) });
  }
  if (seg[0] === 'strategy' && q.get('dataset')) dataset(q.get('dataset')!);
  if (seg[0] === 'strategy' && q.get('from')) run(q.get('from')!);
  const t = q.get('t');
  if (seg[0] === 'book' && t !== null && /^-?\d{1,19}$/.test(t)) {
    crumbs.push({ label: `t ${fmtClock(t)}`, title: `Cursor ${t} ns after dataset origin`, to: `${pathname}${search}` });
  }
  return crumbs;
}

function WorkspaceSwitcher() {
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const current = workspaceFor(pathname);
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLUListElement>(null);

  useEffect(() => {
    if (!open) return;
    menu.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    const onDown = (e: PointerEvent) => {
      if (!menu.current?.contains(e.target as Node) && e.target !== button.current) setOpen(false);
    };
    document.addEventListener('pointerdown', onDown);
    return () => document.removeEventListener('pointerdown', onDown);
  }, [open]);

  const close = () => {
    setOpen(false);
    button.current?.focus();
  };

  const onMenuKey = (e: ReactKeyboardEvent<HTMLUListElement>) => {
    const items = Array.from(menu.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []);
    const i = items.indexOf(document.activeElement as HTMLElement);
    if (e.key === 'ArrowDown') items[(i + 1) % items.length]?.focus();
    else if (e.key === 'ArrowUp') items[(i - 1 + items.length) % items.length]?.focus();
    else if (e.key === 'Home') items[0]?.focus();
    else if (e.key === 'End') items[items.length - 1]?.focus();
    else if (e.key === 'Escape' || e.key === 'Tab') close();
    else return;
    e.preventDefault();
  };

  return (
    <div className="switcher">
      <button
        ref={button}
        type="button"
        className="switcher-button"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        {current?.name ?? 'Workspaces'}
        <ChevronDown {...ICON} />
      </button>
      {open ? (
        <ul ref={menu} role="menu" aria-label="Workspaces" className="menu" onKeyDown={onMenuKey}>
          {WORKSPACES.map((w) => (
            <li key={w.id} role="none">
              <button
                type="button"
                role="menuitem"
                tabIndex={-1}
                className="menu-item"
                aria-current={w === current ? 'page' : undefined}
                onClick={() => {
                  setOpen(false);
                  navigate(w.path);
                }}
              >
                <span className="menu-mnemonic mono">{w.mnemonic}</span>
                <span>{w.name}</span>
                <kbd>g {w.chord}</kbd>
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

function TopBar({ onPalette, onHelp }: { onPalette: () => void; onHelp: () => void }) {
  const crumbs = useBreadcrumb();
  const { settings, update } = useSettings();
  const staticViewer = isStaticViewer();
  return (
    <header className="topbar">
      <Link to="/" className="brand" aria-label="BlackBox HFT, Overview">
        <span className="brand-mark" aria-hidden="true" />
        <span className="brand-name">BlackBox HFT</span>
      </Link>
      <WorkspaceSwitcher />
      {crumbs.length > 0 ? (
        <nav aria-label="Context" className="crumbs">
          <ol>
            {crumbs.map((c, i) => (
              <li key={`${c.to}-${i}`}>
                <Link to={c.to} title={c.title} className="mono">
                  {c.label}
                </Link>
              </li>
            ))}
          </ol>
        </nav>
      ) : null}
      <span className="spacer" />
      <button type="button" className="palette-trigger" onClick={onPalette} aria-keyshortcuts="Control+K">
        <Search {...ICON} />
        <span>Search or run a command</span>
        <kbd>Ctrl K</kbd>
      </button>
      <span
        className="badge badge-neutral"
        title={staticViewer ? STATIC_VIEWER_BANNER : 'Local server: no authentication; every action is available.'}
      >
        {staticViewer ? 'precomputed bundle' : 'local'}
      </span>
      <button
        type="button"
        className="btn-icon"
        aria-pressed={settings.inspectorOpen}
        aria-label="Inspector"
        title="Show or hide the inspector (i)"
        onClick={() => update({ inspectorOpen: !settings.inspectorOpen })}
      >
        {settings.inspectorOpen ? <PanelRightClose {...ICON} /> : <PanelRightOpen {...ICON} />}
      </button>
      <button type="button" className="btn-icon" aria-label="Help, shortcuts and settings" title="Help, shortcuts and settings (?)" onClick={onHelp}>
        <CircleHelp {...ICON} />
      </button>
    </header>
  );
}

// ----- rail -------------------------------------------------------------------------------

function Rail() {
  const { settings } = useSettings();
  return (
    <nav aria-label="Workspaces" className={settings.railOpen ? 'rail rail-open' : 'rail'}>
      <ul>
        {WORKSPACES.map((w) => {
          const Icon = w.icon;
          return (
            <li key={w.id}>
              <NavLink
                to={w.path}
                end={w.id === 'overview'}
                className={({ isActive }) => (isActive ? 'rail-item active' : 'rail-item')}
                title={`${w.name} (g ${w.chord})`}
                aria-label={w.name}
              >
                <Icon {...ICON} />
                <span className="rail-mnemonic">{w.mnemonic}</span>
                {settings.railOpen ? <span className="rail-name">{w.name}</span> : null}
              </NavLink>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}

// ----- inspector --------------------------------------------------------------------------

function Inspector({ onClose }: { onClose: () => void }) {
  const content = useInspectorContent();
  return (
    <aside className="inspector" aria-label="Inspector" tabIndex={-1}>
      <header className="panel-head">
        <h2 className="panel-title">Inspector</h2>
        <span className="inspector-title">{content?.title ?? 'Reference'}</span>
        <div className="panel-actions">
          <button type="button" className="btn-icon" aria-label="Close inspector" title="Close inspector (i)" onClick={onClose}>
            <PanelRightClose {...ICON} />
          </button>
        </div>
      </header>
      <div className="inspector-body">
        {content ? (
          content.body
        ) : (
          <div className="col">
            <p className="note">Select a row or a chart marker to see its details here.</p>
            <h3 className="inspector-heading">Evidence bases</h3>
            <BasisLegend />
            <h3 className="inspector-heading">Glossary</h3>
            <dl className="glossary">
              {Object.values(GLOSSARY).map((g) => (
                <div key={g.term} className="glossary-entry">
                  <dt>{g.term}</dt>
                  <dd>{g.definition}</dd>
                </div>
              ))}
            </dl>
          </div>
        )}
      </div>
    </aside>
  );
}

const INSPECTOR_LAYOUT_KEY = 'bb.layout.shell.v1';

function savedShellLayout(): Record<string, number> | undefined {
  try {
    const raw = localStorage.getItem(INSPECTOR_LAYOUT_KEY);
    const v = raw ? (JSON.parse(raw) as Record<string, number>) : undefined;
    return v && typeof v.workspace === 'number' && typeof v.inspector === 'number' ? v : undefined;
  } catch {
    return undefined;
  }
}

// ----- keyboard ---------------------------------------------------------------------------

const REGIONS = ['.topbar', '.rail', '.ws-toolbar', '#main', '.inspector', '.statusbar'];

function isTyping(el: EventTarget | null): boolean {
  if (!(el instanceof HTMLElement)) return false;
  if (el.isContentEditable || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement) return true;
  return el instanceof HTMLInputElement && !['checkbox', 'radio', 'button', 'submit', 'range'].includes(el.type);
}

function focusRegion(step: 1 | -1) {
  const regions = REGIONS.map((s) => document.querySelector<HTMLElement>(s)).filter((el): el is HTMLElement => el !== null);
  if (regions.length === 0) return;
  const current = regions.findIndex((r) => r.contains(document.activeElement));
  const next = regions[(current + step + regions.length) % regions.length];
  const target =
    next.querySelector<HTMLElement>('[tabindex="0"], button:not([disabled]), a[href], input, select, textarea') ?? next;
  if (target === next && !next.hasAttribute('tabindex')) next.tabIndex = -1;
  target.focus();
}

// ----- shell ------------------------------------------------------------------------------

function ShellLayout() {
  const location = useLocation();
  const navigate = useNavigate();
  const { settings, update } = useSettings();
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const [message, setMessage] = useState('');
  const chord = useRef(0);

  const announce = useCallback((text: string) => {
    // Re-announce identical text by clearing first.
    setMessage('');
    window.setTimeout(() => setMessage(text), 50);
  }, []);

  const copyLink = useCallback(() => {
    void navigator.clipboard?.writeText(window.location.href).then(() => announce('Link to this view copied'));
  }, [announce]);

  const commands: PaletteCommand[] = [
    { id: 'inspector', label: 'Toggle inspector', hint: 'i', run: () => update({ inspectorOpen: !settings.inspectorOpen }) },
    { id: 'rail', label: 'Toggle rail', hint: '\\', run: () => update({ railOpen: !settings.railOpen }) },
    {
      id: 'cvd',
      label: settings.sidePalette === 'cvd' ? 'Side colours: green/red' : 'Side colours: blue/orange (CVD palette)',
      run: () => update({ sidePalette: settings.sidePalette === 'cvd' ? 'default' : 'cvd' }),
    },
    {
      id: 'single',
      label: settings.singleKeys ? 'Turn single-key shortcuts off' : 'Turn single-key shortcuts on',
      run: () => update({ singleKeys: !settings.singleKeys }),
    },
    { id: 'link', label: 'Copy link to this view', hint: 'y', run: copyLink },
    { id: 'help', label: 'Keyboard shortcuts, glossary and settings', hint: '?', run: () => setHelpOpen(true) },
  ];

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return;
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        setPaletteOpen(true);
        return;
      }
      if (e.key === 'F6' && !e.ctrlKey && !e.altKey && !e.metaKey) {
        e.preventDefault();
        focusRegion(e.shiftKey ? -1 : 1);
        return;
      }
      if (!settings.singleKeys || e.ctrlKey || e.metaKey || e.altKey || isTyping(e.target)) return;
      if (document.querySelector('dialog[open]')) return;
      if (Date.now() - chord.current < 1500) {
        chord.current = 0;
        const w = WORKSPACES.find((x) => x.chord === e.key);
        if (w) {
          e.preventDefault();
          navigate(w.path);
        }
        return;
      }
      switch (e.key) {
        case 'g':
          chord.current = Date.now();
          break;
        case '?':
          setHelpOpen(true);
          break;
        case '\\':
          update({ railOpen: !settings.railOpen });
          break;
        case 'i':
          update({ inspectorOpen: !settings.inspectorOpen });
          break;
        case 'y':
          copyLink();
          break;
        default:
          return;
      }
      e.preventDefault();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [settings, update, navigate, copyLink]);

  const [shellLayout] = useState(savedShellLayout);

  return (
    <div className={settings.railOpen ? 'shell shell-rail-open' : 'shell'}>
      <a className="skip-link" href="#main">
        Skip to workspace
      </a>
      <TopBar onPalette={() => setPaletteOpen(true)} onHelp={() => setHelpOpen(true)} />
      <Rail />
      <Group
        className="shell-area"
        orientation="horizontal"
        id="shell"
        defaultLayout={settings.inspectorOpen ? shellLayout : undefined}
        onLayoutChanged={(layout, meta) => {
          if (meta.isUserInteraction) localStorage.setItem(INSPECTOR_LAYOUT_KEY, JSON.stringify(meta.requestedLayout ?? layout));
        }}
      >
        <Panel id="workspace" minSize="40%">
          <main id="main" tabIndex={-1} className="shell-main">
            <WorkspaceErrorBoundary key={location.pathname}>
              <Outlet />
            </WorkspaceErrorBoundary>
          </main>
        </Panel>
        {settings.inspectorOpen ? (
          <>
            <Separator className="split-separator" aria-label="Resize workspace and inspector" />
            <Panel id="inspector" defaultSize="320px" minSize="240px" maxSize="560px">
              <Inspector onClose={() => update({ inspectorOpen: false })} />
            </Panel>
          </>
        ) : null}
      </Group>
      <StatusBar announce={announce} />
      <div className="visually-hidden" role="status" aria-live="polite">
        {message}
      </div>
      <CommandPalette open={paletteOpen} onClose={() => setPaletteOpen(false)} commands={commands} />
      <HelpDialog open={helpOpen} onClose={() => setHelpOpen(false)} />
    </div>
  );
}

export function Shell() {
  return (
    <SettingsProvider>
      <HealthProvider>
        <InspectorProvider>
          <ShellLayout />
        </InspectorProvider>
      </HealthProvider>
    </SettingsProvider>
  );
}
