---
title: WSL — подсистема Windows для Linux (установка, дистрибутивы, ошибки)
description: Установка WSL и дистрибутива, wsl --list, версия 2, остановка, обновление, доступ к файлам, лимиты памяти и частые ошибки.
ms.date: 11/22/2026
---
# WSL (Windows Subsystem for Linux)

Ключевые слова: WSL, линукс в Windows, установить Ubuntu, подсистема Linux, wsl не запускается, виртуализация;
install WSL, Ubuntu on Windows, wsl --install, WSL 2, wsl --shutdown, enable virtualization, 0x80370102.

```powershell
wsl --install                     # WSL + Ubuntu по умолчанию (от администратора, затем перезагрузка)
wsl --list --online               # доступные дистрибутивы (wsl -l -o)
wsl --install -d Debian
wsl --list --verbose              # установленные и их версия (wsl -l -v)
wsl --set-default-version 2
wsl --set-default Ubuntu
wsl -d Ubuntu -u root             # войти от root (например, сбросить пароль: passwd имя)
wsl --shutdown                    # остановить все дистрибутивы и ВМ
wsl --terminate Ubuntu
wsl --update
wsl --status
wsl --version
wsl --export Ubuntu D:\backup\ubuntu.tar
wsl --import Ubuntu2 D:\WSL\Ubuntu2 D:\backup\ubuntu.tar
wsl --unregister Ubuntu           # УДАЛЯЕТ дистрибутив со всеми данными (T3)
```

Файлы Linux из Windows: `\\wsl.localhost\Ubuntu\home\...` (или `\\wsl$\Ubuntu`); диски Windows из Linux: `/mnt/c`.
Лимиты ресурсов — файл `%UserProfile%\.wslconfig`:

```ini
[wsl2]
memory=4GB
processors=2
```

Ошибка `0x80370102` (или «Please enable the Virtual Machine Platform Windows feature and ensure virtualization is
enabled in the BIOS») — включите виртуализацию (Intel VT-x / AMD-V) в BIOS/UEFI и компонент:

```cmd
dism.exe /online /enable-feature /featurename:VirtualMachinePlatform /all /norestart
dism.exe /online /enable-feature /featurename:Microsoft-Windows-Subsystem-Linux /all /norestart
```
