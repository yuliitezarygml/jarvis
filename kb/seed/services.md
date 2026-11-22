---
title: Службы Windows (services) — просмотр, запуск, остановка, тип запуска
description: Как посмотреть, запустить, остановить и отключить службу Windows через PowerShell, sc.exe и services.msc.
ms.date: 11/22/2026
---
# Службы Windows

Ключевые слова: служба, службы, запустить службу, остановить службу, перезапустить, отключить автозапуск службы,
тип запуска; Windows service, start service, stop service, restart service, disable service, startup type, services.msc.

Изменение служб требует прав администратора. Остановка/отключение системных служб — опасное действие (T3).

## Посмотреть службы

```powershell
Get-Service                                   # все службы
Get-Service -Name Spooler                     # одна служба (имя или маска: *print*)
Get-Service | Where-Object Status -eq Running # только запущенные
Get-Service -DisplayName "*Windows Update*"
# тип запуска и путь к exe
Get-CimInstance Win32_Service -Filter "Name='Spooler'" | Select-Object Name, StartMode, State, PathName
```

```cmd
sc.exe query spooler
sc.exe qc spooler
```

В PowerShell `sc` — это псевдоним `Set-Content`, поэтому всегда пишите `sc.exe`.

## Запуск, остановка, перезапуск

```powershell
Start-Service -Name Spooler
Stop-Service -Name Spooler            # -Force — вместе с зависимыми службами
Restart-Service -Name Spooler
```

```cmd
net start spooler
net stop spooler
sc.exe stop spooler
```

## Тип запуска (автозапуск службы)

```powershell
Set-Service -Name Spooler -StartupType Disabled   # Automatic, Manual, Disabled
Set-Service -Name Spooler -StartupType Automatic
```

```cmd
sc.exe config spooler start= disabled
sc.exe config spooler start= auto
sc.exe config spooler start= demand
```

После `start=` обязателен пробел. Графический интерфейс: `services.msc`.

См. также: справка `Get-Service`, `Set-Service`, `sc config` в базе знаний.
