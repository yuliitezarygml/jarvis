---
title: Журнал событий (Event Viewer, Get-WinEvent) — ошибки, сбои, неожиданные выключения
description: Как найти ошибки в журнале событий Windows: Get-WinEvent с фильтрами, wevtutil, важные коды событий.
ms.date: 11/22/2026
---
# Журнал событий

Ключевые слова: журнал событий, просмотр событий, ошибки системы, почему перезагрузился компьютер, синий экран,
вылетает программа; Event Viewer, event log, Get-WinEvent, system errors, unexpected shutdown, application crash.

Интерфейс: `eventvwr.msc`; монитор стабильности: `perfmon /rel`.

```powershell
Get-WinEvent -LogName System -MaxEvents 50
# ошибки (Level 2) и критические (1) за сутки
Get-WinEvent -FilterHashtable @{ LogName = 'System'; Level = 1, 2; StartTime = (Get-Date).AddDays(-1) }
# падения приложений
Get-WinEvent -FilterHashtable @{ LogName = 'Application'; Id = 1000 } -MaxEvents 20 | Format-List TimeCreated, Message
# кто и почему перезагрузил ПК
Get-WinEvent -FilterHashtable @{ LogName = 'System'; Id = 1074 } -MaxEvents 10
Get-WinEvent -ListLog * | Where-Object RecordCount -gt 0
```

```cmd
wevtutil el
wevtutil qe System /c:20 /rd:true /f:text
wevtutil cl Application
```

Полезные коды: System 41 (Kernel-Power, питание пропало/зависание), 6008 (неожиданное выключение), 1074
(инициированная перезагрузка), 7000/7001 (служба не запустилась), 1001 (BugCheck — синий экран);
Application 1000 (сбой приложения), 1002 (зависание); Security 4624/4625 (успешный/неудачный вход).
`Get-EventLog` — устаревший командлет только Windows PowerShell 5.1, используйте `Get-WinEvent`.
