param([string]$Directory = '.kb/sources')
$ErrorActionPreference = 'Stop'
& node (Join-Path $PSScriptRoot 'kb/fetch.mjs') $Directory
exit $LASTEXITCODE
