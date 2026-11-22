---
title: Основы PowerShell (basics) — справка, конвейер, файлы, версии
description: Get-Help и Get-Command, конвейер и фильтрация, работа с файлами, вывод, версии Windows PowerShell 5.1 и PowerShell 7, запуск от администратора.
ms.date: 11/22/2026
---
# Основы PowerShell

Ключевые слова: PowerShell, командлет, справка по команде, конвейер, фильтр, файлы, версия PowerShell, pwsh;
PowerShell basics, cmdlet help, Get-Help, pipeline, Where-Object, Select-Object, PowerShell 7 vs Windows PowerShell.

- **Windows PowerShell 5.1** — `powershell.exe`, встроен в Windows. **PowerShell 7** — `pwsh.exe`, ставится
  отдельно: `winget install --id Microsoft.PowerShell -e`. Версия: `$PSVersionTable.PSVersion`.
- Команды — «Глагол-Существительное» (`Get-Process`), параметры через `-`, регистр не важен.

## Справка и поиск команд

```powershell
Get-Help Get-Process -Examples       # примеры; -Online — страница на learn.microsoft.com
Update-Help -UICulture en-US         # скачать справку (от администратора)
Get-Command *service*                # найти команду по маске
Get-Command -Module NetTCPIP
Get-Process | Get-Member             # свойства и методы объектов
Get-Alias ls                         # ls → Get-ChildItem
```

## Конвейер

```powershell
Get-Process | Where-Object CPU -gt 100 | Sort-Object CPU -Descending | Select-Object -First 5 Name, CPU
Get-Service | Where-Object { $_.Status -eq 'Stopped' -and $_.StartType -eq 'Automatic' }
Get-ChildItem C:\Logs -Filter *.log | Measure-Object Length -Sum
```

Операторы сравнения: `-eq -ne -gt -ge -lt -le -like -match -contains`, логика: `-and -or -not`.

## Файлы и вывод

```powershell
Get-ChildItem -Path C:\Temp -Recurse -File       # ls / dir
Set-Location D:\Projects                          # cd
New-Item -ItemType Directory -Path D:\New
Copy-Item a.txt D:\New\; Move-Item b.txt D:\New\; Remove-Item c.txt
Test-Path D:\New\a.txt
Get-Content app.log -Tail 50 -Wait                # как tail -f
Set-Content out.txt "text"; Add-Content out.txt "more"
Get-Process | Export-Csv procs.csv -NoTypeInformation
Get-Service | ConvertTo-Json | Out-File services.json
Get-Process | Format-Table Name, Id -AutoSize
```

Запуск от администратора: `Start-Process pwsh -Verb RunAs`. Профиль: `$PROFILE`. Скрипты `.ps1` из текущей
папки запускаются как `.\script.ps1` (см. «Типовые ошибки» про политику выполнения).
