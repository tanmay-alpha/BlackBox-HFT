// Same-session runner for one or more UI builds against a real local API.
//
//   node perf/session.mjs --bin <exe> --db <database> --psql <psql.exe> --build <name>=<web dir> [--build ...]
//     [--profiles mobile,desktop] [--reps 3] [--only <scenario,...>] [--data-dir data/e2e] [--port 18810]
//     [--label p11]
//
// For every repetition, profile and build (alternating builds inside a repetition), it deletes today's and
// yesterday's `bb.egress_usage_daily` rows, starts a new `blackbox serve` with that build's `--web-dir`, waits for
// `/api/v1/status`, runs `perf/run.mjs --reps 1` on the scenarios, and stops the server. Output:
// `data/perf/<label>-<build>-<rep>/<profile>.{json,md}`; server logs in `data/perf/<label>-session/`.
// Paths are relative to the repository root. Production credentials are removed from the server environment.
import { execFileSync, spawn } from 'node:child_process';
import { createWriteStream, mkdirSync } from 'node:fs';
import { once } from 'node:events';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const web = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const root = resolve(web, '..');
const opts = { builds: [], profiles: ['mobile', 'desktop'], reps: 3, only: null, data: 'data/e2e', port: 18810, label: 'session' };
for (let i = 2; i < process.argv.length; i += 2) {
  const [key, value] = [process.argv[i], process.argv[i + 1]];
  if (value === undefined) throw new Error(`${key} needs a value`);
  if (key === '--bin') opts.bin = resolve(root, value);
  else if (key === '--psql') opts.psql = resolve(root, value);
  else if (key === '--db') opts.db = value;
  else if (key === '--build') {
    const at = value.indexOf('=');
    if (at < 1) throw new Error('--build expects <name>=<web dir>');
    opts.builds.push([value.slice(0, at), resolve(root, value.slice(at + 1))]);
  } else if (key === '--profiles') opts.profiles = value.split(',');
  else if (key === '--reps') opts.reps = Number(value);
  else if (key === '--only') opts.only = value;
  else if (key === '--data-dir') opts.data = value;
  else if (key === '--port') opts.port = Number(value);
  else if (key === '--label') opts.label = value;
  else throw new Error(`unknown argument ${key}`);
}
for (const k of ['bin', 'psql', 'db']) if (!opts[k]) throw new Error(`--${k} is required`);
if (!opts.builds.length) throw new Error('at least one --build is required');
if (!Number.isInteger(opts.reps) || opts.reps < 3) throw new Error('--reps must be at least 3');
if (![opts.label, opts.db, ...opts.builds.map(([n]) => n)].every((s) => /^[\w.-]+$/.test(s))) throw new Error('invalid label, database or build name');

const base = `http://127.0.0.1:${opts.port}`;
const env = {
  ...process.env,
  DATABASE_URL: `postgres://blackbox:blackbox-local@127.0.0.1:55432/${opts.db}`,
  BB_CORS_ALLOWED_ORIGINS: base,
  PGPASSWORD: 'blackbox-local',
};
for (const key of ['BLOB_READ_WRITE_TOKEN', 'DATABASE_URL_UNPOOLED', 'BB_API_KEY_PEPPER']) delete env[key];
const logs = join(root, 'data/perf', `${opts.label}-session`);
mkdirSync(logs, { recursive: true });
const pause = (ms) => new Promise((r) => setTimeout(r, ms));

async function ready(server) {
  for (let i = 0; i < 120; i += 1) {
    if (server.exitCode !== null) throw new Error(`Server exited: ${server.exitCode}`);
    try {
      const response = await fetch(`${base}/api/v1/status`, { signal: AbortSignal.timeout(1000) });
      if (response.ok) return;
    } catch {
      // The process may still be binding its port.
    }
    await pause(250);
  }
  throw new Error('Server readiness timed out');
}

for (let rep = 1; rep <= opts.reps; rep += 1) {
  for (const profile of opts.profiles) {
    for (const [build, dir] of opts.builds) {
      execFileSync(
        opts.psql,
        ['-h', '127.0.0.1', '-p', '55432', '-U', 'blackbox', '-d', opts.db, '-v', 'ON_ERROR_STOP=1', '-c', "delete from bb.egress_usage_daily where day >= (now() at time zone 'utc')::date - 1"],
        { env, stdio: 'pipe' },
      );
      const name = `${opts.label}-${build}-${rep}`;
      console.log(`${name} ${profile}`);
      const log = createWriteStream(join(logs, `${name}-${profile}.log`));
      const server = spawn(opts.bin, ['--data-dir', opts.data, 'serve', '--addr', `127.0.0.1:${opts.port}`, '--web-dir', dir], { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
      server.stdout.pipe(log);
      server.stderr.pipe(log);
      const exited = once(server, 'exit');
      try {
        await ready(server);
        const args = ['perf/run.mjs', '--base', base, '--profile', profile, '--reps', '1', '--label', name, ...(opts.only ? ['--only', opts.only] : [])];
        const run = spawn(process.execPath, args, { cwd: web, env, stdio: ['ignore', 'ignore', 'inherit'] });
        const [code] = await once(run, 'exit');
        if (code !== 0) throw new Error(`Harness failed: ${name} ${profile} (${code})`);
      } finally {
        server.kill();
        await exited;
        log.end();
      }
    }
  }
}
