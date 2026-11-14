// Help (`?`): every keyboard binding, the arrangement settings, the glossary, and About (versions,
// the server's disclaimer, third-party attribution).

import { BasisLegend } from '../components/ui';
import { fmtBytes, fmtInt } from '../components/fmt';
import { GLOSSARY } from '../components/glossary';
import { Dialog } from './Dialog';
import { useHealth } from './health';
import { useSettings } from './settings';
import { WORKSPACES } from './workspaces';

const GLOBAL_KEYS: [string, string][] = [
  ['Ctrl+K', 'Command palette'],
  ['?', 'This help (single key)'],
  ['g then a letter', 'Go to a workspace (single keys; letters below)'],
  ['F6 / Shift+F6', 'Next / previous region: top bar, rail, toolbar, workspace, inspector, status bar'],
  ['\\', 'Expand or collapse the rail (single key)'],
  ['i', 'Show or hide the inspector (single key)'],
  ['y', 'Copy a link to the current view (single key)'],
  ['Esc', 'Close the open dialog, menu or tooltip'],
];

const TABLE_KEYS: [string, string][] = [
  ['Up / Down', 'Previous / next row'],
  ['PageUp / PageDown', 'One page up / down'],
  ['Home / End', 'First / last row'],
  ['Enter', "Open the row (its primary link)"],
  ['Space', 'Select the row'],
  ['Ctrl+C', 'Copy the focused row as tab-separated raw values'],
  ['Enter on a header', 'Sort by the column; Shift+click adds a secondary key'],
];

const CHART_KEYS: [string, string][] = [
  ['Space', 'Play / pause replay (Order Book Lab)'],
  ['Left / Right', 'Previous / next event; Shift: x10'],
  ['+ / -', 'Zoom in / out around the cursor'],
  ['0', 'Reset the view'],
  ['j / o', 'Focus "jump to time" / "jump to order"'],
  ['[ / ]', 'Previous / next order of the run'],
  ['Separators', 'Arrow keys resize panes, Home/End snap, Enter collapses, double click resets'],
];

function KeyTable({ caption, rows }: { caption: string; rows: [string, string][] }) {
  return (
    <table className="table help-keys">
      <caption>{caption}</caption>
      <tbody>
        {rows.map(([k, v]) => (
          <tr key={k}>
            <th scope="row" className="mono">
              {k}
            </th>
            <td className="wrap">{v}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function HelpDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { settings, update } = useSettings();
  const { status } = useHealth();
  return (
    <Dialog open={open} onClose={onClose} title="Help, shortcuts and settings" className="help">
      <section aria-labelledby="help-settings" className="help-section">
        <h3 id="help-settings">Settings</h3>
        <label className="row">
          <input type="checkbox" checked={settings.singleKeys} onChange={(e) => update({ singleKeys: e.target.checked })} />
          Single-key shortcuts (?, g-chords, \, i, y). Never active while typing in a field.
        </label>
        <label className="row">
          <input
            type="checkbox"
            checked={settings.sidePalette === 'cvd'}
            onChange={(e) => update({ sidePalette: e.target.checked ? 'cvd' : 'default' })}
          />
          Side colours: blue/orange (colour-vision-deficiency palette). Asks stay above bids and every row names its side.
        </label>
      </section>
      <section aria-labelledby="help-keys" className="help-section">
        <h3 id="help-keys">Keyboard</h3>
        <KeyTable caption="Global" rows={GLOBAL_KEYS} />
        <KeyTable caption="Workspace chords (g then)" rows={WORKSPACES.map((w) => [`g ${w.chord}`, `${w.name} (${w.mnemonic})`])} />
        <KeyTable caption="Tables" rows={TABLE_KEYS} />
        <KeyTable caption="Time-axis views, while the chart has focus" rows={CHART_KEYS} />
      </section>
      <section aria-labelledby="help-evidence" className="help-section">
        <h3 id="help-evidence">Evidence bases</h3>
        <BasisLegend />
      </section>
      <section aria-labelledby="help-glossary" className="help-section">
        <h3 id="help-glossary">Glossary</h3>
        <dl className="kv glossary">
          {Object.values(GLOSSARY).map((g) => (
            <div key={g.term} className="glossary-entry">
              <dt>{g.term}</dt>
              <dd className="prose">{g.definition}</dd>
            </div>
          ))}
        </dl>
      </section>
      <section aria-labelledby="help-about" className="help-section">
        <h3 id="help-about">About</h3>
        <p className="prose">BlackBox HFT {'\u2014'} Market Microstructure &amp; Execution Research Laboratory.</p>
        {status ? (
          <>
            <p className="mono">
              blackbox {status.blackbox_version} ({status.git_sha.slice(0, 12)}) {'\u00b7'} API {status.api_version} {'\u00b7'} contract v
              {status.contract_version} {'\u00b7'} {status.engine.name} {status.engine.version}
            </p>
            <p className="mono">
              limits ({status.limits.profile}): {fmtInt(status.limits.max_ticks)} ticks {'\u00b7'} {fmtInt(status.limits.max_tape_events)} tape events{' '}
              {'\u00b7'} {fmtInt(status.limits.max_sweep_cells)} sweep cells {'\u00b7'} {fmtInt(status.limits.deadline_ms)} ms deadline {'\u00b7'}{' '}
              {fmtBytes(status.limits.max_output_bytes)} output
            </p>
            <p className="prose">{status.disclaimer}</p>
          </>
        ) : null}
        <p className="prose">
          Charts on run pages use TradingView Lightweight Charts{'\u2122'}, Copyright (c) 2025 TradingView, Inc.{' '}
          <a href="https://www.tradingview.com/" target="_blank" rel="noreferrer">
            https://www.tradingview.com/
          </a>
        </p>
        <p className="prose">Fonts: Inter and JetBrains Mono (SIL Open Font License 1.1), served from this site.</p>
      </section>
    </Dialog>
  );
}
