---
title: Переменные среды и PATH (environment variables)
description: Как посмотреть, временно и постоянно задать переменную среды и добавить папку в PATH в cmd и PowerShell.
ms.date: 11/22/2026
---
# Переменные среды и PATH

Ключевые слова: переменная среды, переменные окружения, добавить в PATH, путь, команда не найдена после установки;
environment variable, set environment variable permanently, add folder to PATH, setx, user and system variables.

## Посмотреть

```powershell
$env:Path -split ';'
Get-ChildItem Env:
[Environment]::GetEnvironmentVariable("Path", "User")      # только пользовательский PATH
[Environment]::GetEnvironmentVariable("Path", "Machine")   # системный
```

```cmd
set
echo %PATH%
```

## Задать

```powershell
$env:MY_VAR = "value"                                         # только текущий сеанс
[Environment]::SetEnvironmentVariable("MY_VAR", "value", "User")      # навсегда, для пользователя (T2)
[Environment]::SetEnvironmentVariable("MY_VAR", "value", "Machine")   # для всех, от администратора (T3)
Remove-Item Env:MY_VAR                                        # удалить в текущем сеансе
# добавить папку в PATH пользователя
$p = [Environment]::GetEnvironmentVariable("Path", "User")
[Environment]::SetEnvironmentVariable("Path", "$p;C:\Tools", "User")
```

```cmd
set MY_VAR=value
setx MY_VAR "value"
setx MY_VAR "value" /M
```

Не используйте `setx PATH "%PATH%;..."`: `setx` обрезает значение до 1024 символов и смешивает системный PATH с
пользовательским. Новые значения видят только **заново запущенные** программы — перезапустите терминал (иногда
Проводник). Окно: `rundll32 sysdm.cpl,EditEnvironmentVariables`.
