---
title: Реестр Windows (registry) — основы, reg и PowerShell
description: Разделы реестра, типы значений, чтение и изменение через reg.exe и PowerShell, резервная копия перед изменением.
ms.date: 11/22/2026
---
# Реестр Windows: основы

Ключевые слова: реестр, regedit, ключ реестра, значение, изменить реестр, экспорт реестра, резервная копия;
registry, registry key, registry value, reg add, reg query, reg export, HKCU, HKLM, DWORD.

Разделы: `HKEY_LOCAL_MACHINE` (HKLM, весь компьютер — изменения T3), `HKEY_CURRENT_USER` (HKCU, текущий
пользователь — T2), `HKEY_CLASSES_ROOT` (HKCR, ассоциации файлов), `HKEY_USERS` (HKU), `HKEY_CURRENT_CONFIG`.
Типы: `REG_SZ` (строка), `REG_EXPAND_SZ` (строка с %переменными%), `REG_DWORD` (32 бита), `REG_QWORD`,
`REG_MULTI_SZ` (список строк), `REG_BINARY`. Редактор: `regedit`.

**Перед изменением всегда экспортируйте ключ** (Джарвис делает это сам — `data/undo/`).

## reg.exe

```cmd
reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Run"
reg export "HKCU\Software\MyApp" C:\backup\myapp.reg
reg add "HKCU\Software\MyApp" /v Enabled /t REG_DWORD /d 1 /f
reg add "HKCU\Software\MyApp" /v Name /t REG_SZ /d "value" /f
reg delete "HKCU\Software\MyApp" /v Enabled /f
reg import C:\backup\myapp.reg
```

## PowerShell (диски HKCU: и HKLM:)

```powershell
Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
Get-ItemPropertyValue -Path 'HKCU:\Control Panel\Desktop' -Name WallPaper
New-Item -Path 'HKCU:\Software\MyApp' -Force
New-ItemProperty -Path 'HKCU:\Software\MyApp' -Name Enabled -Value 1 -PropertyType DWord -Force
Set-ItemProperty -Path 'HKCU:\Software\MyApp' -Name Enabled -Value 0
Remove-ItemProperty -Path 'HKCU:\Software\MyApp' -Name Enabled
Remove-Item -Path 'HKCU:\Software\MyApp' -Recurse
```

Ошибка «Requested registry access is not allowed» / «Доступ запрещён» в HKLM — нужен запуск от администратора.
