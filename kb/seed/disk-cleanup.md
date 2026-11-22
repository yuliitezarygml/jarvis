---
title: Очистка диска (disk cleanup) — освободить место на C
description: Как освободить место на диске: Контроль памяти, cleanmgr, временные файлы, WinSxS через DISM, корзина, поиск больших файлов.
ms.date: 11/22/2026
---
# Очистка диска

Ключевые слова: мало места на диске, освободить место, очистка диска, временные файлы, temp, диск C заполнен,
большие файлы; disk cleanup, free up disk space, low disk space, temporary files, storage sense, WinSxS cleanup.

## Сколько места

```powershell
Get-Volume                                   # свободно/всего по томам
Get-PSDrive -PSProvider FileSystem
```

## Встроенные средства

- Контроль памяти: Параметры → Система → Память (Settings > System > Storage, `start ms-settings:storagesense`) →
  «Временные файлы», «Рекомендации по очистке».
- Классическая очистка диска: `cleanmgr` (кнопка «Очистить системные файлы» — от администратора), `cleanmgr /d C:`.
  Сохранённый набор: `cleanmgr /sageset:1` (выбрать пункты), затем `cleanmgr /sagerun:1`.

## Команды

```powershell
# временные файлы пользователя (занятые файлы пропускаются)
Remove-Item "$env:TEMP\*" -Recurse -Force -ErrorAction SilentlyContinue
# корзина
Clear-RecycleBin -Force
# 20 самых больших файлов в профиле
Get-ChildItem $env:USERPROFILE -Recurse -File -ErrorAction SilentlyContinue |
  Sort-Object Length -Descending | Select-Object -First 20 FullName, @{n='MB';e={[math]::Round($_.Length/1MB)}}
```

```cmd
:: хранилище компонентов WinSxS (от администратора)
DISM /Online /Cleanup-Image /AnalyzeComponentStore
DISM /Online /Cleanup-Image /StartComponentCleanup
:: отключить гибернацию — удаляет hiberfil.sys (размер ~ объёма ОЗУ)
powercfg /hibernate off
```

Удаление файлов вне временных папок — опасное действие: Джарвис переносит в Корзину и спрашивает подтверждение.
