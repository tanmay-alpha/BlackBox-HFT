// Sweep form parts: the base run's summary and one axis editor. Values are typed in the unit the
// user thinks in (ms, bp) and the exact value sent is printed under the input (gridModel.ts).

import { Link } from "react-router-dom";

import type { RunDetail } from "../../api/model";
import { baseEntryNs, baseResponseNs, type RunConfig } from "../../api/types";
import { exchangeModelLabel, fractionToBps, latencyLabel, queueModelLabel, strategyName } from "../../components/describe";
import { fmtDurationExact } from "../../components/fmt";
import { Term } from "../../components/Tooltip";
import { BasisBadge, HashText } from "../../components/ui";
import { paths } from "../../shell/paths";
import { AXIS_PARAMS, QUEUE_VARIANTS, axisInfo, isPowerVariant } from "./gridModel";
import type { AxisDraft, AxisParam, ParsedAxis } from "./gridModel";

export function BaseSummary({ run }: { run: RunDetail }) {
  const { config } = run;
  const latency = config.execution.order_latency;
  return (
    <dl className="kv">
      <dt>Run</dt>
      <dd>
        <Link to={paths.run(run.run_id)}>
          <HashText hash={run.run_id} />
        </Link>
      </dd>
      <dt>Strategy</dt>
      <dd>
        {run.strategy_kind === null ? 'strategy not reported' : strategyName(run.strategy_kind)}, every {run.interval_ns === null ? 'interval not reported' : fmtDurationExact(run.interval_ns)}
      </dd>
      <dt>Dataset</dt>
      <dd>
        {run.dataset.id} v{run.dataset.version}
      </dd>
      <dt>Exchange model</dt>
      <dd>{exchangeModelLabel(config.execution.exchange_model)}</dd>
      <dt>
        <Term term="latency">Order latency</Term>
      </dt>
      <dd>
        {latency.type === "constant" || latency.type === "jitter"
          ? `entry ${fmtDurationExact(latency.entry_ns)}, response ${fmtDurationExact(latency.response_ns)}${
              latency.type === "jitter" ? `, jitter up to ${fmtDurationExact(latency.jitter_ns)}` : ""
            }`
          : `${latency.type}: ${latencyLabel(latency)}`}
      </dd>
      <dt>Seed</dt>
      <dd>{config.seed}</dd>
    </dl>
  );
}

/** The base run's own value of a parameter: what each grid value replaces. */
function baseValueText(param: AxisParam, config: RunConfig): string {
  const execution = config.execution;
  switch (param) {
    case "order_latency_ns":
      return `entry ${fmtDurationExact(baseEntryNs(execution.order_latency))}, response ${fmtDurationExact(baseResponseNs(execution.order_latency))}`;
    case "entry_latency_ns":
      return fmtDurationExact(baseEntryNs(execution.order_latency));
    case "feed_latency_offset_ns":
      return fmtDurationExact(execution.feed_latency_offset_ns);
    case "queue_model":
      return queueModelLabel(execution.queue_model);
    case "maker_fee":
      return `${fractionToBps(execution.fees.maker)} bp`;
    case "taker_fee":
      return `${fractionToBps(execution.fees.taker)} bp`;
    default:
      return "";
  }
}

export function AxisEditor({
  name,
  draft,
  parsed,
  base,
  onChange,
}: {
  name: "X" | "Y";
  draft: AxisDraft;
  parsed: ParsedAxis;
  base: RunConfig | null;
  onChange: (draft: AxisDraft) => void;
}) {
  const info = axisInfo(draft.param);
  const id = `grd-${name.toLowerCase()}`;
  return (
    <fieldset className="grd-fieldset">
      <legend>
        {name} axis ({name === "X" ? "columns" : "rows"})
      </legend>
      <label className="field" htmlFor={`${id}-param`}>
        <span className="field-label">Parameter</span>
        <span className="field-control">
          <select
            id={`${id}-param`}
            value={draft.param}
            onChange={(e) => {
              const next = AXIS_PARAMS.find((p) => p.param === e.target.value);
              // Units differ between parameters, so typed values are not carried across.
              if (next) onChange({ ...draft, param: next.param, text: "" });
            }}
          >
            {AXIS_PARAMS.map((p) => (
              <option key={p.param} value={p.param}>
                {p.label}
              </option>
            ))}
          </select>
        </span>
      </label>
      <p className="grd-help">
        <BasisBadge basis="modeled" /> {info.help}
        {base ? (
          <>
            {" "}
            In the base run: <span className="mono">{baseValueText(draft.param, base)}</span>.
          </>
        ) : null}
      </p>

      {info.input === "queue" ? (
        <div className="grd-queue" role="group" aria-label={`${name} axis queue models`}>
          {QUEUE_VARIANTS.map((variant) => (
            <div key={variant} className="grd-queue-row">
              <label className="grd-check">
                <input
                  type="checkbox"
                  checked={draft.queue[variant]}
                  onChange={(e) => onChange({ ...draft, queue: { ...draft.queue, [variant]: e.target.checked } })}
                />
                <span className="mono">{variant}</span>
              </label>
              {isPowerVariant(variant) ? (
                <label className="grd-exponent">
                  <span className="dim">exponent n</span>
                  <input
                    type="text"
                    inputMode="decimal"
                    className="mono"
                    value={draft.exponents[variant]}
                    disabled={!draft.queue[variant]}
                    placeholder="e.g. 2, 3"
                    aria-label={`${variant} exponent n, comma separated for several`}
                    onChange={(e) =>
                      onChange({
                        ...draft,
                        exponents: { ...draft.exponents, [variant]: e.target.value },
                      })
                    }
                  />
                </label>
              ) : null}
            </div>
          ))}
          <p className="grd-help">Several exponents for one variant give one model, and one row or column, per exponent.</p>
        </div>
      ) : (
        <label className="field" htmlFor={`${id}-values`}>
          <span className="field-label">
            {info.input === "ms" ? "Values in milliseconds, comma separated" : "Values in basis points, comma separated"}
          </span>
          <span className="field-control">
            <input
              id={`${id}-values`}
              type="text"
              inputMode="decimal"
              className="mono"
              value={draft.text}
              placeholder={info.input === "ms" ? "e.g. 0, 0.5, 2, 10" : "e.g. -1, 0, 2"}
              onChange={(e) => onChange({ ...draft, text: e.target.value })}
            />
            <span className="field-unit">{info.input}</span>
          </span>
        </label>
      )}

      <div className="grd-converted">
        <span className="dim">
          {info.input === "ms"
            ? "Sent as nanoseconds (1 ms = 1,000,000 ns):"
            : info.input === "bp"
              ? "Sent as a fraction of traded value (1 bp = 0.0001):"
              : "Sent as these queue models:"}
        </span>
        {parsed.conversions.length === 0 ? (
          <span className="dim"> no value yet</span>
        ) : (
          <ul>
            {parsed.conversions.map((line, i) => (
              <li key={i} className="mono">
                {line}
              </li>
            ))}
          </ul>
        )}
      </div>
    </fieldset>
  );
}
