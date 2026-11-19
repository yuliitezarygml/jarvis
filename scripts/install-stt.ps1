[CmdletBinding()]
param([string]$Directory = $(if ($env:JARVIS_STT_DIR) {$env:JARVIS_STT_DIR} else {Join-Path $env:APPDATA 'com.jarvis.desktop/stt'}))
$ErrorActionPreference = 'Stop'
$Directory = [IO.Path]::GetFullPath($Directory)
[void][IO.Directory]::CreateDirectory($Directory)
$guard = [IO.File]::Open((Join-Path $Directory '.install.lock'),'OpenOrCreate','ReadWrite','None')
$stage = Join-Path $Directory ([guid]::NewGuid().ToString())
[void][IO.Directory]::CreateDirectory($stage)
function Install-Model([string]$Archive,[string]$Manifest,[string]$Destination) {
    $digests = [ordered]@{}
    foreach ($line in [IO.File]::ReadAllLines((Join-Path $PSScriptRoot $Manifest))) {
        if ($line -notmatch '^([0-9a-f]{64})  ([a-zA-Z0-9_.-]+)$') { throw 'Invalid STT checksum manifest' }
        $digests.Add($Matches[2],$Matches[1])
    }
    $complete = $true
    foreach ($name in $digests.Keys) {
        $file = Join-Path $Destination $name
        if (!(Test-Path -LiteralPath $file -PathType Leaf) -or (Get-FileHash -LiteralPath $file).Hash -ne $digests[$name]) { $complete = $false; break }
    }
    if ($complete) { Write-Output "$Archive verified"; return }
    $download = Join-Path $stage ($Archive+'.tar.bz2')
    & curl.exe --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --retry 2 --max-time 600 --output $download "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/$Archive.tar.bz2"
    if ($LASTEXITCODE -ne 0) { throw "Download failed for $Archive" }
    $members = @($digests.Keys | ForEach-Object { "$Archive/$_" })
    & tar.exe -xjf $download -C $stage @members
    if ($LASTEXITCODE -ne 0) { throw "Extraction failed for $Archive" }
    foreach ($name in $digests.Keys) {
        $file = Join-Path (Join-Path $stage $Archive) $name
        if ((Get-FileHash -LiteralPath $file).Hash -ne $digests[$name]) { throw "Checksum mismatch for $name" }
    }
    [void][IO.Directory]::CreateDirectory($Destination)
    foreach ($name in $digests.Keys) {
        $source = Join-Path (Join-Path $stage $Archive) $name
        $target = Join-Path $Destination $name
        if ([IO.File]::Exists($target)) { [IO.File]::Replace($source,$target,[System.Management.Automation.Language.NullString]::Value) }
        else { [IO.File]::Move($source,$target) }
    }
    Write-Output "$Archive installed and verified"
}
try {
    Install-Model 'sherpa-onnx-zipformer-ru-int8-2025-04-20' 'stt.sha256' $Directory
    Install-Model 'sherpa-onnx-streaming-t-one-russian-2025-09-08' 'stt-streaming.sha256' (Join-Path $Directory 'streaming')
} finally {
    $clean = [IO.Path]::GetFullPath($stage)
    if ([IO.Path]::GetDirectoryName($clean) -eq $Directory -and [IO.Path]::GetFileName($clean) -match '^[a-f0-9-]{36}$') {
        Remove-Item -LiteralPath $clean -Recurse -Force
    }
    $guard.Dispose()
}
