// Renders form controls from the field descriptors in `fields.ts`.

import { useEffect, useState } from 'react';

import { Badge } from '../../components/ui';
import { fieldProblem } from './fields';
import type { Field, NumberField, OptionalNumberField } from './fields';

/** Blank or unparseable text is NaN, which validation reports; it is never coerced to 0. */
function parseNumber(text: string): number {
  const t = text.trim();
  return t === '' ? Number.NaN : Number(t);
}

function numberText(v: number | null): string {
  return v !== null && Number.isFinite(v) ? String(v) : '';
}

export function fieldDomId(scope: string, fieldId: string): string {
  return `${scope}-${fieldId.replace(/[^a-zA-Z0-9]+/g, '-')}`;
}

interface ControlProps<T, F extends Field<T>> {
  field: F;
  obj: T;
  onChange: (next: T) => void;
  domId: string;
  invalid: boolean;
  /** Used instead of a visible <label> (table cells). */
  ariaLabel?: string;
  describedBy?: string;
}

function NumberControl<T>({
  field,
  obj,
  onChange,
  domId,
  invalid,
  ariaLabel,
  describedBy,
}: ControlProps<T, NumberField<T>>) {
  const stored = field.get(obj);
  const toDisplay = (s: number) => (field.convert ? field.convert.toDisplay(s) : s);
  const toStored = (d: number) => (field.convert ? field.convert.toStored(d) : d);
  const [text, setText] = useState(() => numberText(toDisplay(stored)));

  // The stored value changed from outside this input (preset applied, option switched): show it.
  // While the user types, the stored value is exactly what the text converts to, so this is idle.
  useEffect(() => {
    if (!Object.is(toStored(parseNumber(text)), stored)) {
      setText(numberText(toDisplay(stored)));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stored]);

  return (
    <input
      id={domId}
      type="text"
      inputMode="decimal"
      autoComplete="off"
      spellCheck={false}
      value={text}
      aria-label={ariaLabel}
      aria-invalid={invalid}
      aria-describedby={describedBy}
      onChange={(e) => {
        setText(e.target.value);
        onChange(field.set(obj, toStored(parseNumber(e.target.value))));
      }}
    />
  );
}

function OptionalNumberControl<T>({
  field,
  obj,
  onChange,
  domId,
  invalid,
  ariaLabel,
  describedBy,
}: ControlProps<T, OptionalNumberField<T>>) {
  const stored = field.get(obj);
  const [text, setText] = useState(() => numberText(stored));

  useEffect(() => {
    const typed = stored === null ? null : parseNumber(text);
    if (!Object.is(typed, stored)) setText(numberText(stored));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stored]);

  const enabled = stored !== null;
  return (
    <>
      <label className="row optional-toggle">
        <input
          type="checkbox"
          checked={enabled}
          onChange={(e) => {
            setText('');
            // Switching on leaves the value blank: the user chooses it, the form does not.
            onChange(field.set(obj, e.target.checked ? Number.NaN : null));
          }}
        />
        <span>{field.enableLabel}</span>
      </label>
      <input
        id={domId}
        type="text"
        inputMode="decimal"
        autoComplete="off"
        spellCheck={false}
        value={enabled ? text : ''}
        disabled={!enabled}
        aria-label={ariaLabel ?? field.label}
        aria-invalid={invalid}
        aria-describedby={describedBy}
        onChange={(e) => {
          setText(e.target.value);
          onChange(field.set(obj, parseNumber(e.target.value)));
        }}
      />
    </>
  );
}

export interface FieldControlProps<T> {
  field: Field<T>;
  obj: T;
  onChange: (next: T) => void;
  domId: string;
  ariaLabel?: string;
  describedBy?: string;
}

/** The bare control for one field, without label or help. */
export function FieldControl<T>({
  field,
  obj,
  onChange,
  domId,
  ariaLabel,
  describedBy,
}: FieldControlProps<T>) {
  const invalid = fieldProblem(field, obj) !== null;
  switch (field.kind) {
    case 'number':
      return (
        <NumberControl
          field={field}
          obj={obj}
          onChange={onChange}
          domId={domId}
          invalid={invalid}
          ariaLabel={ariaLabel}
          describedBy={describedBy}
        />
      );
    case 'optional_number':
      return (
        <OptionalNumberControl
          field={field}
          obj={obj}
          onChange={onChange}
          domId={domId}
          invalid={invalid}
          ariaLabel={ariaLabel}
          describedBy={describedBy}
        />
      );
    case 'select':
      return (
        <select
          id={domId}
          value={field.get(obj)}
          aria-label={ariaLabel}
          aria-describedby={describedBy}
          onChange={(e) => onChange(field.set(obj, e.target.value))}
        >
          {field.options.map((o) => (
            <option key={o.value} value={o.value} title={o.help} disabled={o.disabled}>
              {o.label}
            </option>
          ))}
        </select>
      );
    case 'text':
      return (
        <input
          id={domId}
          type="text"
          autoComplete="off"
          value={field.get(obj)}
          aria-label={ariaLabel}
          aria-describedby={describedBy}
          onChange={(e) => onChange(field.set(obj, e.target.value))}
        />
      );
  }
}

export interface FieldRowProps<T> {
  field: Field<T>;
  obj: T;
  onChange: (next: T) => void;
  /** Prefix that keeps DOM ids unique on the page. */
  scope: string;
  /** Marks the field as an execution assumption. */
  modeled?: boolean;
}

/** A labelled field with unit, explicit unit conversion, help and its type-level problem. */
export function FieldRow<T>({ field, obj, onChange, scope, modeled }: FieldRowProps<T>) {
  const domId = fieldDomId(scope, field.id);
  const helpId = `${domId}-help`;
  const problem = fieldProblem(field, obj);
  const unit = field.kind === 'number' || field.kind === 'optional_number' ? field.unit : undefined;

  let conversion: string | null = null;
  if (field.kind === 'number' && field.convert) {
    const stored = field.get(obj);
    if (Number.isFinite(stored)) {
      conversion = `= ${stored.toLocaleString('en-US', { maximumFractionDigits: 12 })} ${field.convert.storedUnit} (stored and sent)`;
    }
  }

  return (
    <div className="field">
      <label className="field-label" htmlFor={domId}>
        <span>{field.label}</span>
        {unit ? <span className="field-unit">{unit}</span> : null}
        {modeled ? (
          <Badge kind="modeled" title="An execution assumption: what it produces is an estimate.">
            modeled
          </Badge>
        ) : null}
      </label>
      <div className="field-control">
        <FieldControl
          field={field}
          obj={obj}
          onChange={onChange}
          domId={domId}
          describedBy={helpId}
        />
      </div>
      {conversion ? <span className="field-help mono">{conversion}</span> : null}
      <span className="field-help" id={helpId}>
        {field.help}
      </span>
      {problem ? (
        <span className="field-error" role="alert">
          {problem}
        </span>
      ) : null}
    </div>
  );
}
