---
title: winget — установка и обновление программ (Windows Package Manager)
description: Поиск, установка, обновление, удаление программ и перенос списка приложений через winget.
ms.date: 11/22/2026
---
# winget (диспетчер пакетов Windows)

Ключевые слова: установить программу, обновить программы, удалить программу, менеджер пакетов, обновить всё;
winget install, winget upgrade --all, Windows Package Manager, install app from command line, uninstall program.

Установка/удаление программ — опасное действие (T3). Если `winget` не найден — установите/обновите
«Установщик приложений» (App Installer) из Microsoft Store.

```powershell
winget search vscode                           # поиск (имя, id, источник)
winget show --id Microsoft.VisualStudioCode    # подробности пакета
winget install --id Git.Git -e                 # -e: точное совпадение id
winget install --id Microsoft.PowerShell -e --scope user
winget install --id 7zip.7zip -e --silent --accept-package-agreements --accept-source-agreements
winget list                                    # установленные программы
winget upgrade                                 # что можно обновить
winget upgrade --id Git.Git -e
winget upgrade --all                           # обновить всё
winget uninstall --id 7zip.7zip -e
winget export -o apps.json                     # список приложений → файл
winget import -i apps.json                     # установить по списку на другом ПК
winget pin add --id Discord.Discord            # не обновлять пакет
winget source update                           # обновить источники
winget source reset --force                    # сбросить источники (от администратора)
```

Ошибка «No package found matching input criteria» — проверьте id через `winget search`. Ошибки установщика
показываются кодом (`0x8A15...`); подробный журнал: `winget install ... --verbose-logs`, логи в
`%LOCALAPPDATA%\Packages\Microsoft.DesktopAppInstaller_8wekyb3d8bbwe\LocalState\DiagOutputDir`.
