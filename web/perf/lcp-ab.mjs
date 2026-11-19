// Same-session cold-cache mobile A/B. Each navigation gets an egress reset and server restart.
// node perf/lcp-ab.mjs --bin <copied exe> --base-dir <origin/main dist> --final-dir <candidate dist>
//   --psql <psql.exe> [--data-dir data/e2e-ui] [--port 18810] [--reps 3] [--label p10]
import { spawn, execFileSync } from 'node:child_process';
import { createWriteStream, mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { once } from 'node:events';

const web = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const root = resolve(web, '..');
const args = new Map();
for (let i = 2; i < process.argv.length; i += 2) args.set(process.argv[i], process.argv[i + 1]);
const required = (name) => {
  const value = args.get(name);
  if (!value) throw new Error(`${name} is required`);
  return resolve(root, value);
};
const bin = required('--bin');
const builds = [['base', required('--base-dir')], ['final', required('--final-dir')]];
const psql = required('--psql');
const data = args.get('--data-dir') ?? 'data/e2e-ui';
const port = Number(args.get('--port') ?? 18810);
const reps = Number(args.get('--reps') ?? 3);
const label = args.get('--label') ?? 'p10';
if (!Number.isInteger(reps) || reps < 3) throw new Error('--reps must be at least 3');
if (!/^[\w.-]+$/.test(label)) throw new Error('Invalid label');
const base = `http://127.0.0.1:${port}`;
const env = {
  ...process.env,
  DATABASE_URL: 'postgres://blackbox:blackbox-local@127.0.0.1:55432/blackbox_e2e_ui',
  BB_CORS_ALLOWED_ORIGINS: base,
  PGPASSWORD: 'blackbox-local',
};
for (const key of ['BLOB_READ_WRITE_TOKEN', 'DATABASE_URL_UNPOOLED', 'BB_API_KEY_PEPPER']) delete env[key];
const out = join(root, 'data/perf', `${label}-session`);
mkdirSync(out, { recursive: true });
const pause = (ms) => new Promise((r) => setTimeout(r, ms));
async function ready(server) {
  for (let i = 0; i < 120; i += 1) {
    if (server.exitCode !== null) throw new Error(`Server exited: ${server.exitCode}`);
    try {
      const response = await fetch(`${base}/api/v1/status`, { signal: AbortSignal.timeout(1000) });
      if (response.ok) return;
    } catch { /* the process may still be binding its port */ }
    await pause(250);
  }
  throw new Error('Server readiness timed out');
}
for (let rep = 1; rep <= reps; rep += 1) {
  for (const [build, dir] of builds) {
    for (const scenario of ['explore-load', 'replay-load', 'shell-palette', 'shell-help']) {
      execFileSync(psql, ['-h', '127.0.0.1', '-p', '55432', '-U', 'blackbox', '-d', 'blackbox_e2e_ui', '-v', 'ON_ERROR_STOP=1', '-c', "delete from bb.egress_usage_daily where day >= (now() at time zone 'utc')::date - 1"], { env, stdio: 'pipe' });
      const name = `${label}-${build}-${rep}-${scenario}`;
      console.log(name);
      const log = createWriteStream(join(out, `${name}.log`));
      const server = spawn(bin, ['--data-dir', data, 'serve', '--addr', `127.0.0.1:${port}`, '--web-dir', dir], { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
      server.stdout.pipe(log);
      server.stderr.pipe(log);
      const exited = once(server, 'exit');
      try {
        await ready(server);
        const run = spawn(process.execPath, ['perf/run.mjs', '--base', base, '--profile', 'mobile', '--reps', '1', '--only', scenario, '--label', name], { cwd: web, env, stdio: 'inherit' });
        const [code] = await once(run, 'exit');
        if (code !== 0) throw new Error(`Harness failed: ${name} (${code})`);
      } finally {
        server.kill();
        await exited;
        log.end();
      }
    }
  }
}
