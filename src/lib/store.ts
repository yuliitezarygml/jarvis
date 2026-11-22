'use client';

// Стор состояния окна: события `jarvis://event` → `reduce` → подписчики. `useJarvis(selector)` перерисовывает
// компонент только при смене выбранного среза (селектор должен возвращать готовый объект из состояния).
// Канвас-реактор читает `jarvisStore.getState()` прямо в кадре — без перерисовок React 30 раз в секунду.

import { useEffect, useRef, useSyncExternalStore } from 'react';

import { call, listenTo, onJarvisEvent } from './bridge';
import { initialState, reduce, reduceAll, type JarvisState } from './reducer';

type Listener = () => void;

class JarvisStore {
  private state: JarvisState = initialState;
  private listeners = new Set<Listener>();

  getState = (): JarvisState => this.state;

  subscribe = (l: Listener): (() => void) => {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  };

  dispatch = (raw: unknown) => {
    const next = reduce(this.state, raw, Date.now());
    if (next !== this.state) {
      this.state = next;
      this.listeners.forEach((l) => l());
    }
  };

  replay = (events: unknown[]) => {
    const next = reduceAll(this.state, events, Date.now());
    if (next !== this.state) {
      this.state = next;
      this.listeners.forEach((l) => l());
    }
  };
}

export const jarvisStore = new JarvisStore();

let connecting: Promise<void> | null = null;

/** Подписаться на события (один раз на окно) и подтянуть снимок текущего состояния. */
export function connectJarvis(): Promise<void> {
  if (connecting) return connecting;
  connecting = (async () => {
    await onJarvisEvent(jarvisStore.dispatch);
    try {
      jarvisStore.replay(await call<unknown[]>('events_replay'));
    } catch {
      /* бэкенд без снимка — начинаем с чистого состояния */
    }
  })();
  return connecting;
}

export function useJarvis<T>(selector: (s: JarvisState) => T): T {
  useEffect(() => {
    void connectJarvis();
  }, []);
  return useSyncExternalStore(
    jarvisStore.subscribe,
    () => selector(jarvisStore.getState()),
    () => selector(initialState),
  );
}

/**
 * Подписка на Tauri-событие на время жизни компонента. Подписка асинхронная, поэтому отписка
 * учитывает размонтирование до её завершения (StrictMode монтирует эффекты дважды).
 */
export function useTauriEvent<T>(name: string, handler: (payload: T) => void) {
  const ref = useRef(handler);
  useEffect(() => {
    ref.current = handler;
  });
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listenTo<T>(name, (p) => ref.current(p)).then((u) => {
      if (disposed) u();
      else unlisten = u;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [name]);
}

/** Текущее время, обновляемое с периодом `ms` (часы, таймеры). */
export function useNow(ms = 1000): number {
  return useSyncExternalStore(
    (cb) => {
      const t = setInterval(cb, ms);
      return () => clearInterval(t);
    },
    () => Math.floor(Date.now() / ms) * ms,
    () => 0,
  );
}
