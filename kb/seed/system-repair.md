---
title: Восстановление системы (sfc, DISM, chkdsk, точки восстановления)
description: Проверка и восстановление системных файлов sfc /scannow и DISM RestoreHealth, проверка диска chkdsk, точки восстановления, среда восстановления.
ms.date: 11/22/2026
---
# Восстановление системы

Ключевые слова: повреждены системные файлы, восстановить Windows, проверка целостности, проверить диск на ошибки,
точка восстановления, Windows тормозит после обновления; sfc /scannow, DISM RestoreHealth, repair Windows image,
chkdsk, check disk errors, system restore point.

Все команды — от администратора.

```cmd
:: 1. образ Windows (нужен интернет или источник)
DISM /Online /Cleanup-Image /CheckHealth
DISM /Online /Cleanup-Image /ScanHealth
DISM /Online /Cleanup-Image /RestoreHealth
:: 2. системные файлы — после DISM; журнал: %windir%\Logs\CBS\CBS.log
sfc /scannow
:: 3. диск
chkdsk C: /scan            :: онлайн-проверка без перезагрузки
chkdsk C: /f               :: исправление (для системного диска — при следующей загрузке)
```

```powershell
Repair-Volume -DriveLetter C -Scan
Repair-Volume -DriveLetter C -OfflineScanAndFix
Checkpoint-Computer -Description "Перед изменениями" -RestorePointType MODIFY_SETTINGS   # Windows PowerShell 5.1
Get-ComputerRestorePoint
```

- Откат к точке восстановления: `rstrui.exe`.
- Проверка памяти: `mdsched.exe`.
- Среда восстановления (безопасный режим, откат обновления, восстановление при загрузке):
  `shutdown /r /o /t 0` или Параметры → Система → Восстановление (`start ms-settings:recovery`).
