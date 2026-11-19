# Install a locally built snapshot, or select a retained version for rollback.
[CmdletBinding()]
param([string]$BuildDirectory = '.kb/local',
      [string]$Version,
      [string]$AppDirectory = (Join-Path $env:APPDATA 'com.jarvis.desktop'))
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $BuildDirectory).Path
if (!$Version) { $Version = (Get-Content -Raw -LiteralPath (Join-Path $root 'current.json') | ConvertFrom-Json).version }
if ($version -notmatch '^\d{4}-\d\d-\d\dT[\dZ-]+-[0-9a-f]{8}$') { throw 'Invalid local version' }
$snapshot = Join-Path $root $version
$manifest = Get-Content -Raw -LiteralPath (Join-Path $snapshot 'manifest.json') | ConvertFrom-Json
if ($manifest.version -ne $version -or $manifest.signature -ne 'local_unsigned' -or $manifest.sha256 -notmatch '^[a-f0-9]{64}$') { throw 'Invalid local manifest' }
[void][IO.Directory]::CreateDirectory($AppDirectory)
$guard = [IO.File]::Open((Join-Path $AppDirectory 'kb-install.lock'), 'OpenOrCreate','ReadWrite','None')
$temp = Join-Path $AppDirectory ([guid]::NewGuid().ToString()+'.sqlite3.tmp')
$envTemp = Join-Path $AppDirectory ([guid]::NewGuid().ToString()+'.env.tmp')
try {
    $db = Join-Path $AppDirectory 'windows_kb.sqlite3'
    $envFile = Join-Path $AppDirectory 'backend.env'
    $previous = Join-Path $AppDirectory 'windows_kb.previous.sqlite3'
    [IO.File]::Copy((Join-Path $snapshot 'windows_kb.sqlite3'), $temp)
    if ((Get-Item -LiteralPath $temp).Length -ne $manifest.size -or (Get-FileHash -Algorithm SHA256 -LiteralPath $temp).Hash -ne $manifest.sha256) { throw 'Snapshot checksum mismatch' }
    $settings = if ([IO.File]::Exists($envFile)) { [IO.File]::ReadAllText($envFile) } else { '' }
    $entry = 'JARVIS_KB_LOCAL_SHA256='+$manifest.sha256
    if ($settings -match '(?m)^JARVIS_KB_LOCAL_SHA256=') {
        $settings = [regex]::Replace($settings,'(?m)^JARVIS_KB_LOCAL_SHA256=[^\r\n]*',$entry)
    } else { $settings += [Environment]::NewLine+$entry+[Environment]::NewLine }
    [IO.File]::WriteAllText($envTemp,'')
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $acl = [Security.AccessControl.FileSecurity]::new()
    $acl.SetOwner($sid)
    $acl.SetAccessRuleProtection($true,$false)
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($sid,'FullControl','Allow'))
    Set-Acl -LiteralPath $envTemp -AclObject $acl
    [IO.File]::WriteAllText($envTemp,$settings,[Text.UTF8Encoding]::new($false))
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ($true) {
        try {
            if ([IO.File]::Exists($db)) { [IO.File]::Replace($temp,$db,$previous) }
            else { [IO.File]::Move($temp,$db) }
            break
        } catch {
            $cause = $_.Exception.GetBaseException()
            if (($cause.HResult -band 65535) -notin 32,33 -or [DateTime]::UtcNow -ge $deadline) { throw }
            Start-Sleep -Milliseconds 100
        }
    }
    if ([IO.File]::Exists($envFile)) { [IO.File]::Replace($envTemp,$envFile,[System.Management.Automation.Language.NullString]::Value) }
    else { [IO.File]::Move($envTemp,$envFile) }
    Write-Output "Installed local snapshot $version ($($manifest.documents) documents)"
} finally {
    [IO.File]::Delete($temp)
    [IO.File]::Delete($envTemp)
    $guard.Dispose()
}
