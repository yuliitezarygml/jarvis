# Забирает токен Rust TTS с сервера Xeon по SSH и сохраняет его в backend.env Jarvis.
# Пароль или ключ SSH вводит пользователь. Токен не выводится на экран.
[CmdletBinding()]
param([string]$Server = '192.168.50.38',
      [string]$User = 'root',
      [string]$RemoteEnv = '/etc/jarvis-tts-rs.env',
      [string]$AppDirectory = (Join-Path $env:APPDATA 'com.jarvis.desktop'))
$ErrorActionPreference = 'Stop'
if (!(Get-Command ssh -ErrorAction SilentlyContinue)) {
    throw 'Команда ssh не найдена. Установите «Клиент OpenSSH»: Параметры, Приложения, Дополнительные компоненты.'
}
Write-Output "Подключение к $User@$Server. Введите пароль, когда ssh его запросит."
$line = & ssh "$User@$Server" "grep -m1 '^JARVIS_BEARER_TOKEN=' $RemoteEnv"
if ($LASTEXITCODE -ne 0 -or -not $line) { throw 'Токен не получен: проверьте пароль и путь к файлу на сервере.' }
$token = (([string]$line).Trim() -replace '^JARVIS_BEARER_TOKEN=', '').Trim('"', "'")
if ($token.Length -lt 24) { throw 'Токен короче 24 символов, значение не записано.' }

[void][IO.Directory]::CreateDirectory($AppDirectory)
$envFile = Join-Path $AppDirectory 'backend.env'
$text = if ([IO.File]::Exists($envFile)) { [IO.File]::ReadAllText($envFile) } else { '' }
$entry = "JARVIS_TTS_TOKEN=$token"
if ($text -match '(?m)^JARVIS_TTS_TOKEN=') {
    $text = [regex]::Replace($text, '(?m)^JARVIS_TTS_TOKEN=[^\r\n]*', [Text.RegularExpressions.MatchEvaluator]{ param($m) $entry })
} else {
    if ($text -and -not $text.EndsWith("`n")) { $text += [Environment]::NewLine }
    $text += $entry + [Environment]::NewLine
}
# Временный файл доступен только текущему пользователю, затем атомарно заменяет backend.env.
$temp = Join-Path $AppDirectory ([guid]::NewGuid().ToString() + '.env.tmp')
try {
    [IO.File]::WriteAllText($temp, '')
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $acl = [Security.AccessControl.FileSecurity]::new()
    $acl.SetOwner($sid)
    $acl.SetAccessRuleProtection($true, $false)
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($sid, 'FullControl', 'Allow'))
    Set-Acl -LiteralPath $temp -AclObject $acl
    [IO.File]::WriteAllText($temp, $text, [Text.UTF8Encoding]::new($false))
    if ([IO.File]::Exists($envFile)) { [IO.File]::Replace($temp, $envFile, [System.Management.Automation.Language.NullString]::Value) }
    else { [IO.File]::Move($temp, $envFile) }
} finally {
    [IO.File]::Delete($temp)
}
Write-Output 'Токен записан в backend.env. Перезапустите Jarvis.'
