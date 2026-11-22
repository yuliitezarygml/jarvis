---
title: Типовые ошибки терминала (access denied, execution policy, path not found, git typos)
description: Расшифровка частых ошибок cmd, PowerShell и git с исправлениями — нет прав, политика выполнения скриптов, путь не найден, команда не найдена, опечатки git.
ms.date: 11/22/2026
---
# Типовые ошибки терминала

Ключевые слова: ошибка, отказано в доступе, нет прав, запуск скриптов отключен, путь не найден, не является
внутренней или внешней командой, опечатка в команде git; error, access is denied, running scripts is disabled,
execution policy, cannot find path, is not recognized as the name of a cmdlet, git is not a git command.

## Нет прав (Access is denied)

Сообщения: `Access is denied.` / «Отказано в доступе.» (код 5, ERROR_ACCESS_DENIED),
`UnauthorizedAccessException`, `Requested registry access is not allowed.`,
`The requested operation requires elevation.` (код 740).
Исправление: запустить терминал от администратора (`Start-Process pwsh -Verb RunAs`); для файла проверить права
`icacls "C:\path\file"`, при необходимости `takeown /f "C:\path\file"` (T3); файл может быть занят другим процессом
(`The process cannot access the file because it is being used by another process.`, код 32 — найти процесс:
Монитор ресурсов → ЦП → «Связанные дескрипторы» или Sysinternals `handle.exe`).

## Запуск скриптов отключен (execution policy)

Сообщение: `File C:\x\script.ps1 cannot be loaded because running scripts is disabled on this system.`
(«Невозможно загрузить файл …, так как выполнение сценариев отключено в этой системе», `PSSecurityException`,
`UnauthorizedAccess`).

```powershell
Get-ExecutionPolicy -List
Set-ExecutionPolicy -Scope CurrentUser -ExecutionPolicy RemoteSigned   # только для текущего пользователя
powershell -ExecutionPolicy Bypass -File .\script.ps1                  # разово, без изменения политики
Unblock-File .\script.ps1         # скачанный файл: «is not digitally signed» / «не имеет цифровой подписи»
```

## Путь не найден

PowerShell: `Cannot find path 'C:\x' because it does not exist.` (`ItemNotFoundException`).
cmd: `The system cannot find the path specified.` (код 3) / `The system cannot find the file specified.` (код 2).
Проверить: опечатку, текущую папку (`Get-Location`), кавычки для путей с пробелами (`"C:\Program Files\App"`),
существование (`Test-Path`), раскрытие переменных (`$env:USERPROFILE` в PowerShell vs `%USERPROFILE%` в cmd).

## Команда не найдена

PowerShell: `The term 'x' is not recognized as the name of a cmdlet, function, script file, or operable program.`
(`CommandNotFoundException`). cmd: `'x' is not recognized as an internal or external command, operable program or
batch file.` Причины: опечатка; программа не установлена (`winget search x`); её папки нет в PATH (перезапустить
терминал после установки); скрипт из текущей папки нужно запускать как `.\script.ps1`.

## Опечатки и частые ошибки git

`git: 'comit' is not a git command. See 'git --help'.` + `The most similar command is commit` → `git commit`.
Частые опечатки: `comit` → `commit`, `pul` → `pull`, `psuh` → `push`, `stauts` → `status`, `chekout` → `checkout`,
`brnach` → `branch`, `cone` → `clone`. Автоисправление: `git config --global help.autocorrect prompt`.

- `fatal: not a git repository (or any of the parent directories): .git` — вы не в папке репозитория: `cd` в неё
  или `git init`.
- `fatal: The current branch feature has no upstream branch.` → `git push -u origin feature`.
- `! [rejected] main -> main (fetch first)` / `Updates were rejected because the remote contains work that you do
  not have locally` → `git pull --rebase`, затем `git push`.
- `Author identity unknown` / `Please tell me who you are` → `git config --global user.name "Имя"` и
  `git config --global user.email "почта"`.
- `error: pathspec 'x' did not match any file(s) known to git` — неверное имя ветки/файла: `git branch -a`, `git status`.
