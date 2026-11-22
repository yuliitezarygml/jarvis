---
title: Учётные записи пользователей (local users) — создать, пароль, администратор
description: Локальные пользователи и группы через PowerShell (LocalAccounts) и net user, кто я и мои права (whoami).
ms.date: 11/22/2026
---
# Учётные записи пользователей

Ключевые слова: пользователь, учётная запись, создать пользователя, сменить пароль, сделать администратором,
отключить учётную запись, кто я; local user account, create user, change password, add to administrators group, whoami.

Изменение учётных записей — опасное действие (T3), нужны права администратора.
Интерфейс: Параметры → Учётные записи (`start ms-settings:otherusers`), `netplwiz`, `lusrmgr.msc` (не в Home).

## PowerShell (модуль Microsoft.PowerShell.LocalAccounts, Windows PowerShell 5.1)

```powershell
Get-LocalUser
Get-LocalGroupMember -Group "Administrators"
$pw = Read-Host -AsSecureString "Пароль"
New-LocalUser -Name "guest2" -Password $pw -FullName "Гость"
Add-LocalGroupMember -Group "Administrators" -Member "guest2"
Remove-LocalGroupMember -Group "Administrators" -Member "guest2"
Set-LocalUser -Name "guest2" -Password $pw
Disable-LocalUser -Name "guest2"
Remove-LocalUser -Name "guest2"
```

## cmd

```cmd
net user
net user guest2 * /add
net user guest2 *
net user guest2 /active:no
net localgroup administrators guest2 /add
whoami
whoami /groups
whoami /priv
```

В русской Windows группа администраторов называется «Администраторы» (`net localgroup Администраторы user /add`);
`Get-LocalGroupMember -SID S-1-5-32-544` работает в любой локализации.
