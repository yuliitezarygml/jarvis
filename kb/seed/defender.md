---
title: Microsoft Defender (антивирус) — проверка, обновление, исключения
description: Состояние защиты, быстрая и полная проверка, обновление сигнатур, угрозы и исключения Microsoft Defender Antivirus через PowerShell и MpCmdRun.
ms.date: 11/22/2026
---
# Microsoft Defender Antivirus

Ключевые слова: антивирус, Защитник Windows, проверить на вирусы, сканирование, обновить базы, добавить исключение,
отключить защиту в реальном времени; Windows Defender, Microsoft Defender Antivirus, scan for viruses, quick scan,
full scan, update signatures, add exclusion, real-time protection.

Модуль PowerShell `Defender`; изменения настроек — от администратора (T3). Приложение «Безопасность Windows»:
`start windowsdefender:`.

## Состояние

```powershell
Get-MpComputerStatus | Select-Object AMServiceEnabled, AntivirusEnabled, RealTimeProtectionEnabled, AntivirusSignatureLastUpdated
Get-MpPreference | Select-Object ExclusionPath, ExclusionProcess, ExclusionExtension
```

## Проверка и обновление

```powershell
Update-MpSignature                               # обновить сигнатуры
Start-MpScan -ScanType QuickScan                  # быстрая проверка
Start-MpScan -ScanType FullScan                   # полная
Start-MpScan -ScanType CustomScan -ScanPath "D:\Downloads"
Start-MpWDOScan                                   # автономная проверка (перезагрузка)
Get-MpThreatDetection                             # что найдено
Get-MpThreat
Remove-MpThreat                                   # удалить активные угрозы
```

```cmd
"%ProgramFiles%\Windows Defender\MpCmdRun.exe" -SignatureUpdate
"%ProgramFiles%\Windows Defender\MpCmdRun.exe" -Scan -ScanType 1
```

## Исключения

```powershell
Add-MpPreference -ExclusionPath "D:\dev\build"
Add-MpPreference -ExclusionProcess "node.exe"
Remove-MpPreference -ExclusionPath "D:\dev\build"
```

Отключение защиты в реальном времени (`Set-MpPreference -DisableRealtimeMonitoring $true`) блокируется
«Защитой от подделки» (Tamper Protection) — её выключают только вручную в «Безопасности Windows». Не советуйте
отключать антивирус без крайней необходимости.
