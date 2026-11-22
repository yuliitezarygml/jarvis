---
title: Планировщик заданий (Task Scheduler) — создать, запустить, отключить задачу
description: Создание задач по расписанию и при входе в систему через PowerShell (ScheduledTasks) и schtasks, просмотр результата последнего запуска.
ms.date: 11/22/2026
---
# Планировщик заданий

Ключевые слова: планировщик заданий, задача по расписанию, запускать скрипт каждый день, при входе в систему,
при загрузке; Task Scheduler, scheduled task, run script daily, run at logon, run at startup, schtasks.

Графический интерфейс: `taskschd.msc`. Задачи пользователя — T2, от имени SYSTEM/администратора — T3.

## PowerShell (модуль ScheduledTasks)

```powershell
$action  = New-ScheduledTaskAction -Execute "powershell.exe" -Argument "-NoProfile -File C:\Scripts\backup.ps1"
$trigger = New-ScheduledTaskTrigger -Daily -At 3am          # или -AtLogOn, -AtStartup, -Once -At ...
Register-ScheduledTask -TaskName "Backup" -Action $action -Trigger $trigger -Description "Ежедневный бэкап"

Get-ScheduledTask -TaskName "Backup"
Get-ScheduledTaskInfo -TaskName "Backup"    # LastRunTime, LastTaskResult (0 = успех), NextRunTime
Start-ScheduledTask -TaskName "Backup"
Disable-ScheduledTask -TaskName "Backup"
Enable-ScheduledTask -TaskName "Backup"
Unregister-ScheduledTask -TaskName "Backup" -Confirm:$false
```

## schtasks (cmd)

```cmd
schtasks /query /fo LIST /v
schtasks /create /tn "Backup" /tr "C:\Scripts\backup.cmd" /sc daily /st 03:00
schtasks /create /tn "AtLogon" /tr "notepad.exe" /sc onlogon
schtasks /run /tn "Backup"
schtasks /change /tn "Backup" /disable
schtasks /delete /tn "Backup" /f
```

Код результата `0x1` обычно означает ошибку в самом скрипте/пути; `0x41301` — задача сейчас выполняется.
