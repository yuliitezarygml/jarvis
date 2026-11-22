---
title: Сброс сети и DNS (network reset, flush DNS, winsock reset)
description: Что делать, если нет интернета: сброс кэша DNS, обновление IP, сброс Winsock и стека TCP/IP, сброс сети в Параметрах.
ms.date: 11/22/2026
---
# Сброс сети и DNS

Ключевые слова: нет интернета, сброс сети, сбросить сеть, очистить кэш DNS, обновить IP, сеть не работает,
сайт не открывается; network reset, no internet, flush DNS, reset winsock, reset TCP/IP stack, renew IP address.

## Быстрая диагностика

```powershell
Get-NetAdapter                              # адаптеры и их состояние (Up/Disconnected)
Get-NetIPConfiguration                      # IP, шлюз, DNS
Test-NetConnection 8.8.8.8                  # есть ли связь
Test-NetConnection learn.microsoft.com -Port 443
Resolve-DnsName learn.microsoft.com         # работает ли DNS
```

```cmd
ipconfig /all
ping 8.8.8.8
tracert 8.8.8.8
nslookup learn.microsoft.com
```

## Сброс по шагам (от мягкого к жёсткому)

1. Очистить кэш DNS: `ipconfig /flushdns` или `Clear-DnsClientCache`.
2. Обновить адрес DHCP: `ipconfig /release`, затем `ipconfig /renew`.
3. Перезапустить адаптер: `Restart-NetAdapter -Name "Wi-Fi"` (имя — из `Get-NetAdapter`).
4. Сбросить Winsock и TCP/IP (нужны права администратора и перезагрузка):

```cmd
netsh winsock reset
netsh int ip reset
```

5. Полный сброс сети: Параметры → Сеть и Интернет → Дополнительные сетевые параметры → Сброс сети
   (Settings > Network & internet > Advanced network settings > Network reset). Удаляет и переустанавливает
   сетевые адаптеры, VPN-адаптеры придётся настроить заново.

См. также: `ipconfig`, `netsh winsock`, `netsh interface ip` в справочнике команд.
