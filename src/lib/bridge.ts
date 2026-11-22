// Мост к Rust: `invoke` команд и подписка на события. Вне Tauri (обычный браузер, `npm run dev`)
// подменяется локальными заглушками из `mock.ts` — так интерфейс можно смотреть и править без оболочки.

import { EVENT_NAME } from './types';

export function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** Вызов Tauri-команды. Ошибка команды (`Err(String)`) приходит как исключение со строкой. */
export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (inTauri()) {
    const { invoke } = await import('@tauri-apps/api/core');
    return invoke<T>(cmd, args);
  }
  const { mockInvoke } = await import('./mock');
  return mockInvoke<T>(cmd, args ?? {});
}

/** Подписка на Tauri-событие (или на локальную шину в браузере). Возвращает отписку. */
export async function listenTo<T>(name: string, handler: (payload: T) => void): Promise<() => void> {
  if (inTauri()) {
    const { listen } = await import('@tauri-apps/api/event');
    return listen<T>(name, (ev) => handler(ev.payload));
  }
  const { browserBus } = await import('./mock');
  return browserBus.subscribe(name, (p) => handler(p as T));
}

export function onJarvisEvent(handler: (payload: unknown) => void): Promise<() => void> {
  return listenTo<unknown>(EVENT_NAME, handler);
}

/** Текст ошибки команды для показа человеку. */
export function errorText(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

/** Метка текущего окна Tauri (`hud`, `corner`, …) или null в браузере. */
export async function windowLabel(): Promise<string | null> {
  if (!inTauri()) return null;
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  return getCurrentWindow().label;
}

/** Начать перетаскивание окна (виджет в углу). */
export async function startDragging(): Promise<void> {
  if (!inTauri()) return;
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  await getCurrentWindow().startDragging();
}
