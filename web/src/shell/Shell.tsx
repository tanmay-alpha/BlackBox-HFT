// The workstation shell (UI_DESIGN_SPEC.md §5): top bar, workspace rail, workspace area with an
// optional resizable inspector, status bar, command palette, help, and the global keyboard model
// (§8.1). Browser- and OS-reserved bindings are never taken; Ctrl+K is the only modified chord
// taken over, and single-key shortcuts can be turned off and never fire while typing.

import { Component, Suspense, useCallback, useEffect, useRef, useState } from 'react';
import type { ErrorInfo, KeyboardEvent as ReactKeyboardEvent, ReactNode } from 'react';
import { Link, NavLink, Outlet, useLocation, useNavigate } from 'react-router-dom';
import { Group, Panel, Separator } from 'react-resizable-panels';
import { ChevronDown, CircleHelp, Menu, PanelRightClose, PanelRightOpen, Search } from 'lucide-react';

import { BasisLegend, ErrorBox, Spinner } from '../components/ui';
import { GLOSSARY } from '../components/glossary';
import { fmtClock } from '../components/fmt';
import { AccessProvider, useAccess } from './access';
import { AnnounceContext } from './announce';
import { Dialog } from './Dialog';
import { CommandPalette, type PaletteCommand } from './CommandPalette';
import { HelpDialog } from './HelpDialog';
import { HealthProvider } from './health';
import { InspectorProvider, useInspectorContent } from './inspector';
import { paths } from './paths';
import { SettingsProvider, useSettings } from './settings';
import { StatusBar } from './StatusBar';
import { SECTIONS, WORKSPACES, workspaceFor } from './workspaces';

const ICON = { size: 16, strokeWidth: 1.5, 'aria-hidden': true } as const;

/** Below this width the rail becomes a menu and the inspector a full-screen sheet (shell.css). */
const NARROW_QUERY = '(max-width: 959px)';

function useNarrow(): boolean {
  const [narrow, setNarrow] = useState(() => window.matchMedia(NARROW_QUERY).matches);
  useEffect(() => {
    const mq = window.matchMedia(NARROW_QUERY);
    const on = () => setNarrow(mq.matches);
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  }, []);
  return narrow;
}

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
            <a href="/">the latency experiment</a>.
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

function AccessBadge() {
  const access = useAccess();
  const w = access.whoami;
  if (access.mode === 'research' && w) {
    return (
      <span className="row access-badge">
        <button
          type="button"
          className="badge badge-ok"
          data-testid="access-badge"
          title={`Research access: workspace ${w.workspace.name} (${w.workspace.slug}), scopes ${w.key?.scopes.join(', ') ?? ''}. Activate to change the key.`}
          onClick={access.openKeyDialog}
        >
          RESEARCH {'\u00b7'} key {w.key?.display_prefix}
        </button>
        <button type="button" className="btn-link small" onClick={access.signOut}>
          Sign out
        </button>
      </span>
    );
  }
  return (
    <button
      type="button"
      className="badge badge-neutral"
      data-testid="access-badge"
      title={access.mode === 'demo' ? 'Public demo workspace, read-only. Activate to enter an API key.' : 'Access not known yet. Activate to enter an API key.'}
      onClick={access.openKeyDialog}
    >
      {access.mode === 'demo' ? 'DEMO \u00b7 read-only' : 'ACCESS \u2026'}
    </button>
  );
}

function TopBar({
  onPalette,
  onHelp,
  inspectorOpen,
  onInspector,
}: {
  onPalette: () => void;
  onHelp: () => void;
  inspectorOpen: boolean;
  onInspector: () => void;
}) {
  const crumbs = useBreadcrumb();
  return (
    <header className="topbar">
      <NavMenu />
      <Link to={paths.explore()} className="brand" aria-label="BlackBox HFT, latency experiment">
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
      <AccessBadge />
      <button
        type="button"
        className="btn-icon"
        aria-pressed={inspectorOpen}
        aria-label="Inspector"
        title="Show or hide the inspector (i)"
        onClick={onInspector}
      >
        {inspectorOpen ? <PanelRightClose {...ICON} /> : <PanelRightOpen {...ICON} />}
      </button>
      <button type="button" className="btn-icon" aria-label="Help, shortcuts and settings" title="Help, shortcuts and settings (?)" onClick={onHelp}>
        <CircleHelp {...ICON} />
      </button>
    </header>
  );
}

/** Narrow viewports: the rail's sections as a labelled list in a modal sheet (focus trapped, Esc closes). */
function NavMenu() {
  const { pathname } = useLocation();
  const current = workspaceFor(pathname);
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" className="nav-menu-button" aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(true)}>
        <Menu {...ICON} />
        <span>{current?.name ?? 'Workspaces'}</span>
      </button>
      <Dialog open={open} onClose={() => setOpen(false)} title="Workspaces" className="sheet nav-sheet">
        <nav aria-label="Workspaces">
          {SECTIONS.map((s) => (
            <section key={s.id} className="nav-sheet-section" aria-labelledby={`nav-sheet-${s.id}`}>
              <h3 id={`nav-sheet-${s.id}`} className="rail-section-name">
                {s.name}
              </h3>
              <ul>
                {WORKSPACES.filter((w) => w.section === s.id).map((w) => {
                  const Icon = w.icon;
                  return (
                    <li key={w.id}>
                      <NavLink to={w.path} end={w.id === 'explore'} className="nav-sheet-item" onClick={() => setOpen(false)}>
                        <Icon {...ICON} />
                        <span>{w.name}</span>
                      </NavLink>
                    </li>
                  );
                })}
              </ul>
            </section>
          ))}
        </nav>
      </Dialog>
    </>
  );
}

// ----- rail -------------------------------------------------------------------------------

function Rail() {
  const { settings } = useSettings();
  return (
    <nav aria-label="Workspaces" className={settings.railOpen ? 'rail rail-open' : 'rail'}>
      {SECTIONS.map((s) => (
        <div key={s.id} className="rail-section" role="group" aria-label={s.name}>
          {settings.railOpen ? <h2 className="rail-section-name">{s.name}</h2> : null}
          <ul>
            {WORKSPACES.filter((w) => w.section === s.id).map((w) => {
              const Icon = w.icon;
              return (
                <li key={w.id}>
                  <NavLink
                    to={w.path}
                    end={w.id === 'explore'}
                    className={({ isActive }) => (isActive ? 'rail-item active' : 'rail-item')}
                    title={`${s.name}: ${w.name} (g ${w.chord})`}
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
        </div>
      ))}
    </nav>
  );
}

// ----- inspector --------------------------------------------------------------------------

function InspectorBody() {
  const content = useInspectorContent();
  return content ? (
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
  );
}

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
        <InspectorBody />
      </div>
    </aside>
  );
}

/** Narrow viewports: the inspector as a full-screen modal sheet. */
function InspectorSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const content = useInspectorContent();
  return (
    <Dialog open={open} onClose={onClose} title={`Inspector \u00b7 ${content?.title ?? 'Reference'}`} className="sheet inspector-sheet">
      <InspectorBody />
    </Dialog>
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
  const regions = REGIONS.map((s) => document.querySelector<HTMLElement>(s)).filter(
    (el): el is HTMLElement => el !== null && el.getClientRects().length > 0,
  );
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
  const [message, setMessage] = useState('');
  const announce = useCallback((text: string) => {
    // Re-announce identical text by clearing first.
    setMessage('');
    window.setTimeout(() => setMessage(text), 50);
  }, []);
  return (
    <AnnounceContext.Provider value={announce}>
      <AccessProvider announce={announce}>
        <ShellFrame announce={announce} />
        <div className="visually-hidden" role="status" aria-live="polite">
          {message}
        </div>
      </AccessProvider>
    </AnnounceContext.Provider>
  );
}

function ShellFrame({ announce }: { announce: (text: string) => void }) {
  const location = useLocation();
  const navigate = useNavigate();
  const { settings, update } = useSettings();
  const access = useAccess();
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const chord = useRef(0);
  const narrow = useNarrow();
  const [sheetOpen, setSheetOpen] = useState(false);
  const inspectorOpen = narrow ? sheetOpen : settings.inspectorOpen;
  const toggleInspector = useCallback(() => {
    if (narrow) setSheetOpen((o) => !o);
    else update({ inspectorOpen: !settings.inspectorOpen });
  }, [narrow, settings.inspectorOpen, update]);
  useEffect(() => {
    if (!narrow) setSheetOpen(false);
  }, [narrow]);

  const copyLink = useCallback(() => {
    void navigator.clipboard?.writeText(window.location.href).then(() => announce('Link to this view copied'));
  }, [announce]);

  const commands: PaletteCommand[] = [
    { id: 'inspector', label: 'Toggle inspector', hint: 'i', run: toggleInspector },
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
    access.mode === 'research'
      ? { id: 'signout', label: 'Sign out (remove the API key from this tab)', run: access.signOut }
      : { id: 'key', label: 'Enter API key (research access)', run: access.openKeyDialog },
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
          toggleInspector();
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
  }, [settings, update, navigate, copyLink, toggleInspector]);

  const [shellLayout] = useState(savedShellLayout);

  return (
    <div className={settings.railOpen ? 'shell shell-rail-open' : 'shell'}>
      <a className="skip-link" href="#main">
        Skip to workspace
      </a>
      <TopBar
        onPalette={() => setPaletteOpen(true)}
        onHelp={() => setHelpOpen(true)}
        inspectorOpen={inspectorOpen}
        onInspector={toggleInspector}
      />
      <Rail />
      <Group
        className="shell-area"
        orientation="horizontal"
        id="shell"
        defaultLayout={settings.inspectorOpen && !narrow ? shellLayout : undefined}
        onLayoutChanged={(layout, meta) => {
          if (meta.isUserInteraction) localStorage.setItem(INSPECTOR_LAYOUT_KEY, JSON.stringify(meta.requestedLayout ?? layout));
        }}
      >
        <Panel id="workspace" minSize="40%">
          <main id="main" tabIndex={-1} className="shell-main">
            <WorkspaceErrorBoundary key={location.pathname}>
              <Suspense fallback={<Spinner label="Loading workspace" />}>
                <Outlet />
              </Suspense>
            </WorkspaceErrorBoundary>
          </main>
        </Panel>
        {settings.inspectorOpen && !narrow ? (
          <>
            <Separator className="split-separator" aria-label="Resize workspace and inspector" />
            <Panel id="inspector" defaultSize="320px" minSize="240px" maxSize="560px">
              <Inspector onClose={() => update({ inspectorOpen: false })} />
            </Panel>
          </>
        ) : null}
      </Group>
      <StatusBar announce={announce} />
      {narrow ? <InspectorSheet open={sheetOpen} onClose={() => setSheetOpen(false)} /> : null}
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
