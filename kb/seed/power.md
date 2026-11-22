---
title: Питание и выключение (power plans, sleep, hibernate, shutdown)
description: Схемы электропитания powercfg, сон и гибернация, отчёт о батарее, что не даёт уснуть, выключение и перезагрузка по команде.
ms.date: 11/22/2026
---
# Электропитание

Ключевые слова: схема питания, высокая производительность, сон, гибернация, батарея, отчёт о батарее, не уходит в
сон, выключить компьютер, перезагрузка по таймеру; power plan, high performance, sleep, hibernate, battery report,
powercfg, shutdown timer, restart computer.

```cmd
powercfg /list
powercfg /getactivescheme
powercfg /setactive SCHEME_MIN          :: высокая производительность
powercfg /setactive SCHEME_BALANCED     :: сбалансированная
powercfg /setactive SCHEME_MAX          :: экономия энергии
powercfg /duplicatescheme e9a42b02-d5df-448d-aa00-03f14749eb61   :: добавить «Максимальную производительность»
powercfg /change monitor-timeout-ac 10
powercfg /change standby-timeout-ac 0   :: не засыпать от сети
powercfg /hibernate off                 :: выключить гибернацию (удаляет hiberfil.sys)
powercfg /batteryreport                 :: battery-report.html в текущей папке
powercfg /energy                        :: диагностика энергопотребления (от администратора)
powercfg /requests                      :: что мешает уйти в сон
powercfg /lastwake                      :: что разбудило ПК
```

## Выключение и перезагрузка (T3)

```cmd
shutdown /s /t 0          :: выключить сейчас
shutdown /r /t 0          :: перезагрузить
shutdown /s /t 3600       :: выключить через час
shutdown /a               :: отменить запланированное выключение
shutdown /h               :: гибернация
shutdown /r /o /t 0       :: перезагрузка в среду восстановления (особые варианты загрузки)
```

```powershell
Restart-Computer
Stop-Computer
```

Параметры: Система → Питание и батарея (`start ms-settings:powersleep`).
