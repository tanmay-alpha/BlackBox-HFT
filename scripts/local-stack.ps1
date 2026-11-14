# Local v1 stack without Docker, in one command (Windows PowerShell, from the repository root):
#   $env:DATABASE_URL = 'postgres://user:password@127.0.0.1:5432/blackbox'; scripts/local-stack.ps1
# Builds the binary and the console, applies the migrations, seeds the demo workspace (demo runs
# computed in this process, curated comparisons and saved reports; idempotent) and serves the
# console and /api/v1 on http://127.0.0.1:8787. The database must exist; it is not created here.
# DATA_DIR (default `data`) holds the datasets and run artefacts.
$ErrorActionPreference = 'Stop'

if (-not $env:DATABASE_URL) {
    throw 'set DATABASE_URL to an existing Postgres database, e.g. postgres://user:password@127.0.0.1:5432/blackbox'
}
$data = if ($env:DATA_DIR) { $env:DATA_DIR } else { 'data' }
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$bin = Join-Path $target 'release/blackbox'

function Invoke-Step {
    param([string]$File, [string[]]$Arguments)
    & $File @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$File $($Arguments -join ' ') failed with exit code $LASTEXITCODE" }
}

Invoke-Step cargo @('build', '--release', '--locked', '-p', 'bb-app', '--bin', 'blackbox')
Push-Location web
try {
    Invoke-Step npm @('ci')
    Invoke-Step npm @('run', 'build')
} finally {
    Pop-Location
}
Invoke-Step $bin @('--data-dir', $data, 'migrate')
Invoke-Step $bin @('--data-dir', $data, 'demo', 'seed', '--wait')
Invoke-Step $bin @('--data-dir', $data, 'serve', '--web-dir', 'web/dist')
