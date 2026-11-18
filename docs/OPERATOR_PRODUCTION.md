# Operating production safely

Who this is for: the operator who holds the production credentials of the hosted deployment (Render
service, Neon Postgres, private Vercel Blob store). It covers where credentials may live, how to run
a maintenance command against production without leaking them into a developer shell, seeding,
backup and recovery, and what the logs may contain. The deployment contract itself (variables,
Render service, release) is `docs/engineering/DEPLOYMENT.md`.

## 1. Where production credentials live

| Credential | Lives in | Never in |
|---|---|---|
| `DATABASE_URL`, `DATABASE_URL_UNPOOLED` (Neon, `sslmode=verify-full`) | Render service environment; operator env file under `data/secrets/` | shell profile, `.env` in the repository root, CI, chat, issues |
| `BLOB_READ_WRITE_TOKEN` (private Blob store) | Render service environment; operator env file | same |
| `BB_API_KEY_PEPPER` (key check tag) | Render service environment; operator env file (needed by `keys mint`) | same |
| Plaintext `bbk_` keys | the person who received it | anywhere stored: only the hash is in Postgres |

`data/secrets/` is git-ignored. Keep each env file owner-readable only (`chmod 600` on Linux/macOS;
on Windows keep it under your user profile's ACL). `scripts/ci/secret-scan.sh` runs in CI against
the tracked tree.

## 2. Credential isolation

The two profiles are kept apart by the binary itself and by the scripts:

- **The local profile refuses production credentials.** `blackbox` with `BB_PROFILE` unset or
  `local` exits before connecting anything when `BLOB_READ_WRITE_TOKEN` is set or when
  `DATABASE_URL`/`DATABASE_URL_UNPOOLED` names a non-local host. Local means a Unix socket,
  `localhost`, a loopback, private (RFC 1918) or link-local address, or a single-label host such as
  the Docker Compose service `db`. The error names the variable and never prints its value:

  ```
  error: invalid configuration: BLOB_READ_WRITE_TOKEN holds a production cloud credential but
  BB_PROFILE is local; unset it (scripts/local-stack.* clear it) or run the one operator command
  through scripts/ops/prod-run.sh (docs/OPERATOR_PRODUCTION.md)
  ```

- **The explicit operator flag.** `BB_ALLOW_CLOUD_CREDENTIALS=true` admits them for the local
  profile. The process then logs a `cloud_credentials_override` warning at startup. Set it only
  through `scripts/ops/prod-run.sh`, never in a shell profile.
- **The hosted profile is unchanged.** `BB_PROFILE=hosted` (Render, `render.yaml`, the `hosted`
  Docker target) requires the real credentials and needs no flag.
- **Local stacks clear inherited credentials.** `scripts/local-stack.sh` unsets
  `BLOB_READ_WRITE_TOKEN`, `DATABASE_URL_UNPOOLED`, `BB_API_KEY_PEPPER` and
  `BB_ALLOW_CLOUD_CREDENTIALS` and forces `BB_PROFILE=local`. `scripts/local-stack.ps1` hides the
  same variables from its child processes and restores the caller's values when it ends. Docker
  Compose (`docker-compose.yml`) passes only its own `db` URL.

## 3. Running one production command: `scripts/ops/prod-run.sh`

```
scripts/ops/prod-run.sh <env-file> <command> [args...]
```

The script reads `<env-file>` in a subshell, exports its variables to `<command>` only and execs
it. Nothing is exported to the calling shell, no value is printed (only the variable names), and
production variables already in the caller's environment are dropped first so the file is the only
source. The file format is `KEY=VALUE` per line. Blank lines and `#` comments are ignored, and an
optional `export ` prefix and one layer of matching quotes are stripped. There is no shell
expansion and nothing in the file is executed. On Linux/macOS the script refuses a file readable by
group or others.

A minimal operator file (`data/secrets/prod-ops.env`; values come from the Neon and Vercel
consoles, never from a chat or ticket):

```
BB_PROFILE=local
DATABASE_URL=<Neon direct (unpooled) connection string, ending in ?sslmode=verify-full>
DATABASE_URL_UNPOOLED=<the same direct connection string>
BB_API_KEY_PEPPER=<the Render service's pepper>
# Only for the curation pass of `demo seed` (it reads run bundles):
# BLOB_READ_WRITE_TOKEN=<the Render service's token>
```

Use the direct (unpooled) endpoint for `DATABASE_URL` too: migrations and the CLI need session
semantics. If the local resolver refuses `*.neon.tech`, add `hostaddr=<resolved IP>` to both URLs
(DEPLOYMENT.md §8). Use a scratch `--data-dir` (for example `data/ops`) so production runs never
mix with local development data.

| Task | Command |
|---|---|
| Apply migrations by hand | `scripts/ops/prod-run.sh data/secrets/prod-ops.env target/release/blackbox --data-dir data/ops migrate` |
| Mint an admin key (printed once) | `… target/release/blackbox --data-dir data/ops keys mint --workspace lab --scopes read,run,admin --label lead` |
| List / revoke keys | `… blackbox --data-dir data/ops keys list` / `… keys revoke <key-id>` |
| Queue the demo runs | `… blackbox --data-dir data/ops demo seed --platform <result_platform from /api/v1/status>` |
| Store curated comparisons and reports (second pass) | same command again, with `BLOB_READ_WRITE_TOKEN` enabled in the file |
| Backup | `… sh -c 'pg_dump --format=custom --schema=bb --file "data/secrets/backups/bb-$(date -u +%F).dump" "$DATABASE_URL_UNPOOLED"'` |

The single quotes in the backup line matter: `$DATABASE_URL_UNPOOLED` must be expanded by the child
shell, which has it, not by your shell, which does not.

## 4. Secure seeding

1. Deploy first. The hosted worker computes the demo runs on the canonical platform class; read
   `result_platform` from `GET /api/v1/status`.
2. Run `demo seed --platform <class>` through `prod-run.sh` without the Blob token. It registers
   the synthetic dataset and queues the demo runs, and it is idempotent.
3. Wait until `/api/v1/status` shows an empty queue, then enable `BLOB_READ_WRITE_TOKEN` in the env
   file and run `demo seed` again. It stores curated comparisons and saved reports and prints
   anything still pending. Comment the token out again afterwards.
4. Mint research keys with `keys mint` (the plaintext is shown once). Hand it over through a
   channel that does not keep history, and record only the key id.
5. For the very first admin key without database access from the operator machine, use the
   bootstrap digest (DEPLOYMENT.md §8 step 7) and clear `BB_ADMIN_BOOTSTRAP_KEY_SHA256` once real
   keys exist.

## 5. Recovery

| Situation | Action |
|---|---|
| Bad data within 6 h | Neon Console → restore the root branch to a timestamp (DEPLOYMENT.md §11) |
| Older damage | `pg_restore` the latest `data/secrets/backups/*.dump` into a fresh Neon branch, check it, then switch the Render `DATABASE_URL*` to it |
| Lost Blob objects | Not backed up. Bundles are reproducible from their stored configurations and are verified against the hashes in Postgres (DEPLOYMENT.md §11) |
| Leaked database password | Neon Console → reset the role password; update the Render environment and the operator file; redeploy |
| Leaked Blob token | Vercel → rotate the store token; update Render and the operator file; redeploy |
| Leaked pepper | Set a new `BB_API_KEY_PEPPER` on Render and in the operator file. Every existing key stops verifying, so re-mint the keys that are still needed (§3) and revoke the old ids |
| Leaked `bbk_` key | `keys revoke <key-id>` through `prod-run.sh` |
| Lost all admin keys | Bootstrap digest (§4 step 5) |

After any rotation, delete superseded operator env files instead of keeping old values around.

## 6. What the logs may contain (SEC-18)

Production logs are JSON lines on stdout (Render log stream). Each failure event carries safe
fields: `request_id` (HTTP), `job_id` (worker), `component` (`startup`, `http`, `db`, `worker`,
`storage`, `panic`), `error_class`, `reason` (a stable code such as `db_unreachable`,
`upload_failed`, `simulation_failed`) and `elapsed_ms` where a duration exists. Raw dependency,
engine or panic text appears only in the `error`/`panic` field, after redaction. The redactor
removes every configured secret value (database URLs and their passwords, the Blob token, the
pepper, in plain, percent-decoded and JSON-escaped spellings), any `scheme://user:password@`
credentials, and Blob- or `bbk_`-shaped tokens. As a second guard every formatted log line passes
through the same redactor before it is written, and panics are logged as one redacted
`thread panicked` event instead of the default raw stderr message. Command-line errors printed by
`blackbox` are redacted the same way.

HTTP error bodies never contain the diagnostic: internal failures answer with a generic message and
the request id (SEC-17). Persisted job errors use a stable category/code and a generic or bounded
message ("internal worker error", "the simulation failed; the details are in the server log").

Evidence: `crates/bb-app/tests/secret_logging.rs` and `crates/bb-app/tests/worker_faults.rs`
capture real log output with sentinel secrets configured and assert that the sentinels never
appear while the safe fields do.

## 7. Fault injection is test-only

The worker's `FaultInjector` seam (SEC-17) exists so integration tests can fail individual job
steps. It has no HTTP endpoint, environment variable or command-line switch. Only in-process code
that holds the `WorkerHub` can install it, and no production code path does.
