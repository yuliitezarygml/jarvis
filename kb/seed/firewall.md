---
title: Брандмауэр Windows (firewall) — открыть порт, разрешить или заблокировать программу
description: Профили брандмауэра, правила для порта и программы через PowerShell (NetSecurity) и netsh advfirewall.
ms.date: 11/22/2026
---
# Брандмауэр Windows

Ключевые слова: брандмауэр, фаервол, открыть порт, разрешить программу, заблокировать программе интернет, правило;
Windows Firewall, open port, allow app through firewall, block program internet access, inbound rule, outbound rule.

Изменение брандмауэра — опасное действие (T3), нужны права администратора.
Интерфейс: `wf.msc` (расширенный), «Безопасность Windows» → «Брандмауэр и защита сети», `firewall.cpl`.

## Состояние

```powershell
Get-NetFirewallProfile | Select-Object Name, Enabled, DefaultInboundAction, DefaultOutboundAction
Get-NetFirewallRule -DisplayName "*8080*" | Format-Table DisplayName, Enabled, Direction, Action
```

## Правила

```powershell
# открыть входящий TCP-порт
New-NetFirewallRule -DisplayName "Allow TCP 8080" -Direction Inbound -Protocol TCP -LocalPort 8080 -Action Allow
# запретить программе выход в интернет
New-NetFirewallRule -DisplayName "Block app" -Direction Outbound -Program "C:\Apps\app.exe" -Action Block
Disable-NetFirewallRule -DisplayName "Allow TCP 8080"
Remove-NetFirewallRule -DisplayName "Allow TCP 8080"
# включить брандмауэр во всех профилях
Set-NetFirewallProfile -Profile Domain,Private,Public -Enabled True
```

```cmd
netsh advfirewall show allprofiles
netsh advfirewall set allprofiles state on
netsh advfirewall firewall add rule name="Allow TCP 8080" dir=in action=allow protocol=TCP localport=8080
netsh advfirewall firewall delete rule name="Allow TCP 8080"
netsh advfirewall reset
```
