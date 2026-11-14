// Playwright web server for the v1 E2E stories: from the repo root, apply the migrations, seed the
// public demo workspace, mint a research key (written to `<data-dir>/e2e-key.txt`; the key is
// never logged), then run `blackbox serve` with the built UI on the same origin.
//
// Env: DATABASE_URL (default: the local Postgres database `blackbox_e2e_ui`, which holds only what
// this suite creates: a shared database also lists runs whose bundles live in other data
// directories, and those fail verification here), BLACKBOX_BIN, E2E_PORT.

import { execFileSync, spawn } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const webDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const root = resolve(webDir, '..');
const exe = process.platform === 'win32' ? '.exe' : '';
const bin = resolve(root, process.env.BLACKBOX_BIN ?? `target/debug/blackbox${exe}`);
const dataDir = 'data/e2e-ui';
const port = process.env.E2E_PORT ?? '18797';
const env = {
  ...process.env,
  DATABASE_URL: process.env.DATABASE_URL ?? 'postgres://blackbox:blackbox-local@127.0.0.1:55432/blackbox_e2e_ui',
  BB_CORS_ALLOWED_ORIGINS: `http://127.0.0.1:${port}`,
};

const run = (args) => execFileSync(bin, ['--data-dir', dataDir, ...args], { cwd: root, env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] });

mkdirSync(join(root, dataDir), { recursive: true });
run(['migrate']);
run(['demo', 'seed']);
const minted = run(['keys', 'mint', '--workspace', 'e2e-lab', '--label', 'e2e-ui', '--scopes', 'read,run']);
const key = /bbk_[A-Za-z0-9_-]{43}/.exec(minted)?.[0];
if (!key) throw new Error('keys mint printed no key');
writeFileSync(join(root, dataDir, 'e2e-key.txt'), key);

const server = spawn(bin, ['--data-dir', dataDir, 'serve', '--addr', `127.0.0.1:${port}`, '--web-dir', 'web/dist'], { cwd: root, env, stdio: 'inherit' });
for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => server.kill(sig));
server.on('exit', (code) => process.exit(code ?? 0));
