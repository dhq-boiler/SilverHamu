$ErrorActionPreference = 'Stop'
& python (Join-Path $PSScriptRoot 'assemble.py')
if ($LASTEXITCODE -ne 0) { throw 'GIF assembly failed' }
