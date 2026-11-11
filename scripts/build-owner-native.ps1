param()
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
& cargo build --locked --release --lib --manifest-path (Join-Path $repo 'Cargo.toml') --target-dir (Join-Path $repo 'target')
if ($LASTEXITCODE -ne 0) { throw "silverhamu native build failed (exit $LASTEXITCODE)." }
