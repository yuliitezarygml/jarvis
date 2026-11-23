@echo off
rem J.A.R.V.I.S. — запуск. Двойной клик: попросит права администратора (один раз UAC) и запустит приложение.
chcp 65001 >nul
setlocal
cd /d "%~dp0"
title J.A.R.V.I.S.

rem --- права администратора: Джарвис управляет службами, реестром и окнами других программ ---
net session >nul 2>&1
if errorlevel 1 (
    echo Запрашиваю права администратора...
    powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -WorkingDirectory '%~dp0' -Verb RunAs"
    exit /b
)

rem --- проверки ---
where cargo >nul 2>&1 || (echo [ошибка] Не найден Rust ^(cargo^). Установите: https://rustup.rs & goto :fail)
where npm   >nul 2>&1 || (echo [ошибка] Не найден Node.js ^(npm^). Установите: https://nodejs.org & goto :fail)

if not exist "config.json" (
    echo Нет config.json — создаю из config.example.json. Ключи моделей добавьте в Настройках.
    copy /y "config.example.json" "config.json" >nul
)

if not exist "node_modules" (
    echo Первый запуск: устанавливаю зависимости интерфейса...
    call npm install || goto :fail
)

echo.
echo Запускаю J.A.R.V.I.S. ^(первая сборка может занять несколько минут^)...
echo Окно не закрывайте: здесь журнал работы. Остановить — Ctrl+C или «Выход» в трее.
echo.
call cargo tauri dev
if errorlevel 1 goto :fail
exit /b 0

:fail
echo.
echo Запуск не удался. Журнал: data\logs\jarvis.log
pause
exit /b 1
