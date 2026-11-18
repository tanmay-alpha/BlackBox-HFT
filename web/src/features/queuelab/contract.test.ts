import { describe, expect, it } from 'vitest';

import publishedSums from '../../../public/study-results/queue-shadow/study-result.json.sha256?raw';
import publishedText from '../../../public/study-results/queue-shadow/study-result.json?raw';
import { StudyLoadError, sha256Hex } from '../study/contract';
import { parseQueueShadow, verifyQueueShadow } from './contract';

async function failure(p: Promise<unknown>): Promise<StudyLoadError> {
  try {
    await p;
  } catch (e) {
    expect(e).toBeInstanceOf(StudyLoadError);
    return e as StudyLoadError;
  }
  throw new Error('expected a StudyLoadError');
}

/** `text` published with a matching hash file, as the generator would write it. */
async function republish(text: string): Promise<{ bytes: Uint8Array; sums: string }> {
  const bytes = new TextEncoder().encode(text);
  return { bytes, sums: `${await sha256Hex(bytes)}  study-result.json\n` };
}

function edited(edit: (doc: Record<string, any>) => void): string {
  const doc = JSON.parse(publishedText);
  edit(doc);
  return JSON.stringify(doc);
}

describe('the published queue-shadow artefact', () => {
  it('matches its hash file and contract blackbox.queue-shadow version 1', async () => {
    const { study, sha256 } = await verifyQueueShadow(new TextEncoder().encode(publishedText), publishedSums);
    expect(sha256).toBe(publishedSums.slice(0, 64));
    expect(study.study_id).toBe('queue-shadow');
    expect(study.data_class).toBe('synthetic');
    expect(study.results.cells).toHaveLength(
      study.results.symbols.length * study.design.latencies_ns.length * study.design.horizons_ns.length * study.models.length,
    );
  });
});

describe('verifyQueueShadow failure kinds', () => {
  it('hash_mismatch: one changed byte fails before the content is read', async () => {
    const bytes = new TextEncoder().encode(publishedText);
    bytes[bytes.length - 2] ^= 1;
    const e = await failure(verifyQueueShadow(bytes, publishedSums));
    expect(e.kind).toBe('hash_mismatch');
    expect(e.message).toContain(publishedSums.slice(0, 64));
  });

  it('unsupported_contract: a newer version is refused even with a matching hash', async () => {
    const { bytes, sums } = await republish(publishedText.replace('"schema_version":1', '"schema_version":2'));
    const e = await failure(verifyQueueShadow(bytes, sums));
    expect(e.kind).toBe('unsupported_contract');
    expect(e.message).toMatch(/reads blackbox\.queue-shadow version 1; the file declares "blackbox\.queue-shadow" version 2/);
  });

  it('unsupported_contract: another contract name (the execution study) is refused', async () => {
    const { bytes, sums } = await republish(edited((d) => (d.schema = 'blackbox.study-result')));
    expect((await failure(verifyQueueShadow(bytes, sums))).kind).toBe('unsupported_contract');
  });

  it('invalid: a real-data artefact is never shown', async () => {
    const { bytes, sums } = await republish(edited((d) => (d.data_class = 'real_local')));
    const e = await failure(verifyQueueShadow(bytes, sums));
    expect(e.kind).toBe('invalid');
    expect(e.message).toMatch(/data_class is not "synthetic" \(real-data results are never published\)/);
  });

  it('invalid: a hash file that names another file or is not a sha256sum line', async () => {
    const bytes = new TextEncoder().encode(publishedText);
    expect((await failure(verifyQueueShadow(bytes, `${publishedSums.slice(0, 64)}  other.json\n`))).kind).toBe('invalid');
    expect((await failure(verifyQueueShadow(bytes, '<!doctype html>'))).kind).toBe('invalid');
  });
});

describe('parseQueueShadow', () => {
  it('reads implementation and interval metadata without changing contract v1', () => {
    const study = parseQueueShadow(publishedText);
    expect(study.schema_version).toBe(1);
    expect(study.engine?.revision).toBe('inclusive-deadlines-v2');
    expect(study.engine?.deadline_semantics).toEqual(expect.any(String));
    expect(study.engine?.checkpoint_semantics).toEqual(expect.any(String));
    expect(study.interval_note).toEqual(expect.any(String));
    expect(study.caveats.every((c) => typeof c === 'string')).toBe(true);
  });

  it('still accepts older v1 artefacts without additive metadata', () => {
    const study = parseQueueShadow(edited((d) => {
      delete d.engine;
      delete d.interval_note;
    }));
    expect(study.engine).toBeUndefined();
    expect(study.interval_note).toBeUndefined();
  });

  it.each(['revision', 'deadline_semantics', 'checkpoint_semantics'])('validates engine.%s before rendering it', (field) => {
    expect(() => parseQueueShadow(edited((d) => (d.engine[field] = 42)))).toThrow(`engine.${field} is not a string`);
    expect(() => parseQueueShadow(edited((d) => delete d.engine[field]))).toThrow(`engine.${field} is not a string`);
  });

  it('rejects malformed optional metadata and caveats', () => {
    expect(() => parseQueueShadow(edited((d) => (d.engine = null)))).toThrow('engine is not an object');
    expect(() => parseQueueShadow(edited((d) => (d.interval_note = 42)))).toThrow('interval_note is not a string');
    expect(() => parseQueueShadow(edited((d) => (d.interval_note = null)))).toThrow('interval_note is not a string');
    expect(() => parseQueueShadow(edited((d) => (d.caveats = 'not an array')))).toThrow('caveats is not an array');
    expect(() => parseQueueShadow(edited((d) => (d.caveats[0] = 42)))).toThrow('caveats[0] is not a string');
  });

  it('names the first bad field', () => {
    expect(() => parseQueueShadow(edited((d) => (d.results.cells[3].fill_fraction_95.lower.status = 'maybe')))).toThrow(
      /results\.cells\[3\]\.fill_fraction_95\.lower\.status is not "available" or "unavailable"/,
    );
    expect(() => parseQueueShadow(edited((d) => delete d.results.cells[1].vs_l3))).toThrow(/results\.cells\[1\]\.vs_l3 is not an object/);
  });

  it('requires every design combination exactly once', () => {
    expect(() => parseQueueShadow(edited((d) => d.results.cells.pop()))).toThrow(/results\.cells has 89 key combinations, not the 90 of the design/);
    expect(() => parseQueueShadow(edited((d) => (d.results.cells[1] = d.results.cells[0])))).toThrow(/results\.cells\[1\] repeats the key/);
    expect(() => parseQueueShadow(edited((d) => (d.results.checkpoints[0].latency_ns = 7)))).toThrow(/results\.checkpoints\[0\]\.latency_ns is 7, not one of 0, 50000, 500000/);
  });

  it('requires the L3 reference first and only L2 models in the hypotheses', () => {
    expect(() => parseQueueShadow(edited((d) => d.models.reverse()))).toThrow(/models\[0\]\.id is not "l3_shadow"/);
    expect(() => parseQueueShadow(edited((d) => d.hypotheses.cells[0].h0_not_rejected.push('l3_shadow')))).toThrow(/h0_not_rejected\[9\]/);
  });

  it('rejects text that is not JSON as invalid', () => {
    expect(() => parseQueueShadow('<!doctype html>')).toThrow(expect.objectContaining({ kind: 'invalid' }));
  });
});
