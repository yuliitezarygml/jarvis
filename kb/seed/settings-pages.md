---
title: Страницы Параметров и оснастки (ms-settings URI, .msc, .cpl)
description: Как открыть нужную страницу Параметров Windows по URI ms-settings и классические оснастки панели управления командой.
ms.date: 11/22/2026
---
# Открыть страницу Параметров или оснастку

Ключевые слова: открыть настройки, параметры Bluetooth, настройки звука, настройки экрана, панель управления,
диспетчер устройств; open settings page, ms-settings URI, launch Settings app, control panel applet, msc snap-in.

Запуск: `start ms-settings:bluetooth` (cmd), `Start-Process "ms-settings:bluetooth"` (PowerShell), Win+R.

| Что | URI |
|---|---|
| Bluetooth и устройства | `ms-settings:bluetooth` |
| Wi-Fi | `ms-settings:network-wifi` |
| Состояние сети | `ms-settings:network-status` |
| VPN / прокси | `ms-settings:network-vpn`, `ms-settings:network-proxy` |
| Звук | `ms-settings:sound` |
| Экран, ночной свет | `ms-settings:display`, `ms-settings:nightlight` |
| Уведомления | `ms-settings:notifications` |
| Питание | `ms-settings:powersleep` |
| Память (место на диске) | `ms-settings:storagesense` |
| Установленные приложения | `ms-settings:appsfeatures` |
| Приложения по умолчанию | `ms-settings:defaultapps` |
| Автозагрузка | `ms-settings:startupapps` |
| Учётные записи, другие пользователи | `ms-settings:yourinfo`, `ms-settings:otherusers` |
| Варианты входа | `ms-settings:signinoptions` |
| Дата и время, язык | `ms-settings:dateandtime`, `ms-settings:regionlanguage` |
| Персонализация, панель задач | `ms-settings:personalization`, `ms-settings:taskbar` |
| Конфиденциальность: микрофон, камера | `ms-settings:privacy-microphone`, `ms-settings:privacy-webcam` |
| Центр обновления Windows | `ms-settings:windowsupdate` |
| Безопасность Windows | `ms-settings:windowsdefender` |
| Восстановление | `ms-settings:recovery` |
| О системе | `ms-settings:about` |
| Принтеры и сканеры | `ms-settings:printers` |
| Мышь, сенсорная панель | `ms-settings:mousetouchpad` |

Классические оснастки: `devmgmt.msc` (диспетчер устройств), `diskmgmt.msc` (управление дисками), `services.msc`,
`eventvwr.msc`, `taskschd.msc`, `compmgmt.msc`, `gpedit.msc` (Pro), `lusrmgr.msc`; апплеты: `appwiz.cpl`
(программы и компоненты), `ncpa.cpl` (сетевые подключения), `sysdm.cpl` (свойства системы), `mmsys.cpl` (звук),
`main.cpl` (мышь), `timedate.cpl`, `powercfg.cpl`, `firewall.cpl`, `inetcpl.cpl`; а также `control`, `msconfig`,
`msinfo32`, `resmon`, `perfmon`, `regedit`, `winver`.
