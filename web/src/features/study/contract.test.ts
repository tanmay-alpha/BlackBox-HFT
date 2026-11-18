import { describe, expect, it } from 'vitest';

import publishedSums from '../../../public/study-results/exec-validation/study-result.json.sha256?raw';
import publishedText from '../../../public/study-results/exec-validation/study-result.json?raw';
import { StudyLoadError, parseShaFile, parseStudyResult, sha256Hex, verifyStudy } from './contract';
import { makeStudy, published } from './study.fixtures';

async function failure(p: Promise<unknown>): Promise<StudyLoadError> {
  try {
    await p;
  } catch (e) {
    expect(e).toBeInstanceOf(StudyLoadError);
    return e as StudyLoadError;
  }
  throw new Error('expected a StudyLoadError');
}

describe('sha256Hex', () => {
  it('hashes with WebCrypto to lowercase hex', async () => {
    expect(await sha256Hex(new TextEncoder().encode('abc'))).toBe('ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
  });
});

describe('parseShaFile', () => {
  const digest = 'a'.repeat(64);
  it('reads a sha256sum line, with LF or CRLF', () => {
    expect(parseShaFile(`${digest}  study-result.json\n`)).toBe(digest);
    expect(parseShaFile(`${digest}  study-result.json\r\n`)).toBe(digest);
  });
  it('rejects another file name, several lines or a malformed digest', () => {
    expect(() => parseShaFile(`${digest}  other.json\n`)).toThrow(/names other.json/);
    expect(() => parseShaFile(`${digest}  study-result.json\n${digest}  study-result.json\n`)).toThrow(StudyLoadError);
    expect(() => parseShaFile(`${'A'.repeat(64)}  study-result.json`)).toThrow(StudyLoadError);
    expect(() => parseShaFile('<!doctype html><html></html>')).toThrow(StudyLoadError);
  });
});

describe('verifyStudy', () => {
  it('accepts bytes that match the published hash and the contract', async () => {
    const { bytes, sums } = await published(makeStudy(), sha256Hex);
    const verified = await verifyStudy(bytes, sums);
    expect(verified.sha256).toBe(sums.slice(0, 64));
    expect(verified.study.cells).toHaveLength(3);
  });

  it('fails visibly on a single changed byte, before reading the content', async () => {
    const { bytes, sums } = await published(makeStudy(), sha256Hex);
    const tampered = bytes.slice();
    tampered[tampered.length - 2] ^= 1;
    const e = await failure(verifyStudy(tampered, sums));
    expect(e.kind).toBe('hash_mismatch');
    expect(e.message).toContain(sums.slice(0, 64));
  });

  it('rejects another contract version even when the hash matches', async () => {
    const study = { ...makeStudy(), schema_version: 2 };
    const { bytes, sums } = await published(study, sha256Hex);
    const e = await failure(verifyStudy(bytes, sums));
    expect(e.kind).toBe('unsupported_contract');
    expect(e.message).toMatch(/reads blackbox\.study-result version 1; the file declares "blackbox\.study-result" version 2/);
  });

  it('rejects another contract name', () => {
    expect(() => parseStudyResult(JSON.stringify({ ...makeStudy(), schema: 'something-else' }))).toThrow(
      expect.objectContaining({ kind: 'unsupported_contract' }),
    );
  });
});

describe('parseStudyResult', () => {
  it('names the first field that breaks the contract', () => {
    const study = makeStudy() as unknown as { cells: Record<string, unknown>[] };
    delete study.cells[1].accounting;
    expect(() => parseStudyResult(JSON.stringify(study))).toThrow(/cells\[1\]\.accounting is not an object/);
  });

  it('requires the run id to be the config hash prefix', () => {
    const study = makeStudy();
    study.cells[0].run_id = 'ffffffffffffffff';
    expect(() => parseStudyResult(JSON.stringify(study))).toThrow(/cells\[0\]\.run_id is not the prefix of config_sha256/);
  });

  it('requires seed-variation uncertainty', () => {
    const study = { ...makeStudy(), uncertainty: { ...makeStudy().uncertainty, source: 'market' } };
    expect(() => parseStudyResult(JSON.stringify(study))).toThrow(/uncertainty\.source/);
  });

  it('rejects text that is not JSON', () => {
    expect(() => parseStudyResult('<!doctype html>')).toThrow(expect.objectContaining({ kind: 'invalid' }));
  });
});

describe('the published exec-validation artefact', () => {
  it('matches its hash file and contract version 1', async () => {
    const verified = await verifyStudy(new TextEncoder().encode(publishedText), publishedSums);
    expect(verified.study.study_id).toBe('exec-validation');
    expect(verified.study.data_class).toBe('synthetic');
    expect(verified.study.cells.length).toBeGreaterThan(0);
    // Bounded: per-cell summaries only, no tapes.
    expect(publishedText.length).toBeLessThan(1_000_000);
    expect(publishedText).not.toMatch(/wall_time_ms|created_unix_ms/);
  });
});
