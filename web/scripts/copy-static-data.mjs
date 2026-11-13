// Copies the bundle written by `blackbox export-static --out data/static-data` into the static
// build (`dist/static-data/`), where the hosted read-only viewer reads it. The bundle is generated
// locally and git-ignored; it holds synthetic data only (the exporter refuses anything else).

import { cpSync, existsSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const web = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = join(web, '..', 'data', 'static-data');
const target = join(web, 'dist', 'static-data');

if (!existsSync(join(source, 'blackbox-static.json'))) {
  console.error(
    `No exported bundle at ${source}. From the repository root run:\n` +
      '  cargo build -p bb-app --bin blackbox\n' +
      '  target/debug/blackbox export-static --out data/static-data',
  );
  process.exit(1);
}
rmSync(target, { recursive: true, force: true });
cpSync(source, target, { recursive: true });
console.log(`Copied ${source} to ${target}.`);
