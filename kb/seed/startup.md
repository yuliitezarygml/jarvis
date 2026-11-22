---
title: Автозагрузка программ (startup apps) — как отключить
description: Где в Windows 11 включается и отключается автозапуск программ: Диспетчер задач, Параметры, папка Startup, ключи Run, Autoruns.
ms.date: 11/22/2026
---
# Автозагрузка программ

Ключевые слова: автозагрузка, автозапуск, убрать из автозагрузки, отключить автозапуск программы, программы при
старте Windows, медленно загружается; startup apps, disable startup programs, run at startup, startup folder, Run key.

## Отключить программу (проще всего)

- Диспетчер задач (Ctrl+Shift+Esc) → вкладка «Автозагрузка приложений» (Startup apps) → правый клик → «Отключить».
  Столбец «Влияние на запуск» (Startup impact) показывает, что тормозит загрузку.
- Параметры → Приложения → Автозагрузка (Settings > Apps > Startup): `start ms-settings:startupapps`.

## Где ещё живёт автозапуск

- Папка автозагрузки пользователя: `shell:startup`, для всех пользователей: `shell:common startup` (открыть через Win+R).
- Реестр:
  - `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` — текущий пользователь;
  - `HKLM\Software\Microsoft\Windows\CurrentVersion\Run` — все пользователи (нужны права администратора);
  - `...\RunOnce` — однократный запуск.
- Задачи планировщика с триггером «При входе в систему» (`Get-ScheduledTask`), службы с типом запуска Automatic.

```powershell
# список автозапуска (Run-ключи и папки Startup)
Get-CimInstance Win32_StartupCommand | Select-Object Name, Command, Location, User
# посмотреть ключ Run текущего пользователя
Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
# удалить запись из Run (сначала сделайте экспорт ключа)
Remove-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'AppName'
```

Полный список всех мест автозапуска (драйверы, службы, расширения Проводника, задачи) — Sysinternals **Autoruns**
(`autoruns.exe`, консольная версия `autorunsc.exe`).
