# Jarvis

Основа desktop-приложения: **Rust + Tauri 2 + Next.js + React + TypeScript**.

## Запуск

Нужны Node.js 22 LTS, npm и Rust/Cargo. Для macOS также нужны Xcode Command Line Tools. Системные зависимости для Windows и Linux описаны в [документации Tauri](https://v2.tauri.app/start/prerequisites/).

```bash
npm install
npm run desktop:dev
```

Команда сама запускает Next.js и открывает нативное окно. На первом запуске Cargo скачивает и компилирует зависимости.

Только веб-интерфейс:

```bash
npm run dev
```

Адрес: http://127.0.0.1:3000. Rust-команды доступны в desktop-окне; веб-просмотр выводит подсказку при попытке их вызвать.

## Сборка

```bash
npm run desktop:build
```

Next.js экспортирует интерфейс в `out/`, Tauri встраивает его в приложение. Результат находится в `src-tauri/target/release/bundle/`. Для сборки на другую ОС обычно нужна соответствующая система и её инструменты.

```bash
npm run build       # Только статический интерфейс
npm run check       # ESLint и TypeScript
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

## Структура

```text
src/
  app/                 # Страницы, корневой layout, стили
  components/          # React-компоненты
  lib/tauri.ts         # Типизированные вызовы Rust-команд
src-tauri/
  src/commands.rs      # Команды greet и get_app_info
  src/lib.rs           # Запуск Tauri и регистрация команд
  capabilities/        # Разрешения нативного окна
  tauri.conf.json      # Настройки окна и сборки
```

На стартовом экране есть пример настоящего вызова `invoke` из React в Rust, с проверкой имени, обработкой ошибок и состоянием загрузки.

Чтобы добавить команду, создай функцию с `#[tauri::command]` в `commands.rs`, зарегистрируй её в `generate_handler!` в `lib.rs` и добавь типизированную обёртку в `src/lib/tauri.ts`.

## Особенности Next.js в Tauri

Используется App Router и `output: "export"`, как рекомендует [руководство Tauri для Next.js](https://v2.tauri.app/start/frontend/nextjs/). Server Actions, API Routes и SSR требуют отдельного сервера и в эту основу не входят. Локальная логика выполняется в Rust через `invoke`.

Перед публикацией замени `identifier` в `src-tauri/tauri.conf.json` на собственный идентификатор приложения. Исходник иконки находится в `public/icon.svg`; пересоздать платформенные иконки можно командой:

```bash
npm run tauri -- icon public/icon.svg --output src-tauri/icons
```
