---
title: Процессы (processes) — что грузит CPU и память, завершить процесс, кто занял порт
description: Топ процессов по CPU и памяти, завершение процесса, запуск от администратора, процесс на порту — PowerShell, tasklist, taskkill.
ms.date: 11/22/2026
---
# Процессы

Ключевые слова: процесс, что грузит процессор, что жрёт память, завершить процесс, убить процесс, зависла
программа, кто занял порт; process, high CPU, high memory usage, kill process, end task, port in use, taskkill.

Диспетчер задач: Ctrl+Shift+Esc; монитор ресурсов: `resmon`; подробно — Sysinternals Process Explorer.

```powershell
# топ-10 по CPU (секунды процессорного времени) и по памяти
Get-Process | Sort-Object CPU -Descending | Select-Object -First 10 Name, Id, CPU
Get-Process | Sort-Object WorkingSet64 -Descending |
  Select-Object -First 10 Name, Id, @{n='MB';e={[math]::Round($_.WorkingSet64/1MB)}}
Stop-Process -Name notepad            # завершить по имени
Stop-Process -Id 1234 -Force
Start-Process notepad
Start-Process pwsh -Verb RunAs        # от администратора (UAC)
# кто слушает порт 8080
Get-NetTCPConnection -LocalPort 8080 | Select-Object LocalPort, State, OwningProcess
Get-Process -Id (Get-NetTCPConnection -LocalPort 8080).OwningProcess
```

```cmd
tasklist
tasklist /fi "imagename eq chrome.exe"
tasklist /svc
taskkill /im notepad.exe /f
taskkill /pid 1234 /t /f
netstat -ano | findstr :8080
```

Завершение системных процессов (csrss, wininit, lsass, svchost) — опасно (T3): может вызвать синий экран.
