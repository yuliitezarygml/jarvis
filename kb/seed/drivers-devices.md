---
title: Драйверы и устройства (drivers, Device Manager, pnputil)
description: Устройства с ошибками, список драйверов, установка и удаление драйвера, отключение устройства через pnputil и PowerShell PnpDevice.
ms.date: 11/22/2026
---
# Драйверы и устройства

Ключевые слова: драйвер, устройство не работает, диспетчер устройств, обновить драйвер, удалить драйвер, код 43,
неизвестное устройство; driver, Device Manager, device not working, install driver, uninstall driver, pnputil, Code 43.

Установка/удаление драйверов — опасное действие (T3). Интерфейс: `devmgmt.msc`. Драйверы через Центр обновления:
Параметры → Центр обновления Windows → Дополнительные параметры → Необязательные обновления.

## Посмотреть

```powershell
Get-PnpDevice -Status ERROR                 # устройства с проблемами
Get-PnpDevice -Class Display, Net
Get-CimInstance Win32_PnPSignedDriver | Select-Object DeviceName, DriverVersion, Manufacturer, DriverDate
```

```cmd
pnputil /enum-devices /problem
pnputil /enum-drivers
driverquery /v
```

## Действия

```cmd
pnputil /add-driver C:\Drivers\device.inf /install
pnputil /add-driver C:\Drivers\*.inf /subdirs /install
pnputil /delete-driver oem12.inf /uninstall
pnputil /scan-devices
```

```powershell
Disable-PnpDevice -InstanceId 'USB\VID_...' -Confirm:$false
Enable-PnpDevice -InstanceId 'USB\VID_...' -Confirm:$false
```

Частые коды ошибок диспетчера устройств: 10 — устройство не может запуститься, 28 — драйвер не установлен,
43 — Windows остановила устройство (сбой драйвера или железа), 45 — устройство не подключено.
