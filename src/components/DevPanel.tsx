'use client';

import { useEffect, useState } from 'react';

import { call, errorText, inTauri } from '@/lib/bridge';
import { STATE_LABEL } from '@/lib/glow';
import type { AssistantState, JarvisEvent } from '@/lib/types';

const STATES: AssistantState[] = ['idle', 'listening', 'thinking', 'speaking', 'controlling', 'error'];

const send = (event: JarvisEvent) => call('simulate_event', { event });

/** Генератор волны звука на 2,5 с (для ручной проверки реактора). */
function burst(source: 'mic' | 'tts') {
  const start = performance.now();
  const tick = () => {
    const p = (performance.now() - start) / 1000;
    const rms = p > 2.5 ? 0 : Math.abs(Math.sin(p * 7.3)) * (0.6 + 0.4 * Math.sin(p * 1.7));
    void send({ type: 'audio_level', source, rms });
    if (p <= 2.5) setTimeout(tick, 33);
  };
  tick();
}

/**
 * Панель симулятора: весь сценарий целиком или отдельные события. Видна в отладочной сборке и в браузере;
 * в релизе открывается сочетанием Ctrl+Shift+D.
 */
export default function DevPanel({ defaultOpen = false }: { defaultOpen?: boolean }) {
  const [visible, setVisible] = useState(false);
  const [open, setOpen] = useState(defaultOpen);
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    if (!inTauri()) {
      setVisible(true);
    } else {
      call<{ debug_build: boolean }>('app_info')
        .then((i) => setVisible(i.debug_build))
        .catch(() => setVisible(false));
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && e.code === 'KeyD') {
        e.preventDefault();
        setVisible(true);
        setOpen((o) => !o);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  if (!visible) return null;

  const run = async (f: () => Promise<unknown>, done?: string) => {
    try {
      await f();
      setStatus(done ?? null);
    } catch (e) {
      setStatus(errorText(e));
    }
  };

  return (
    <aside className={open ? 'devpanel open' : 'devpanel'} aria-label="Симулятор">
      <button type="button" className="devpanel-toggle" onClick={() => setOpen(!open)} aria-expanded={open}>
        Симулятор {open ? '▾' : '▸'}
      </button>
      {open && (
        <div className="devpanel-body">
          <div className="dev-row">
            <button type="button" className="btn btn-primary" onClick={() => void run(() => call<number>('simulate_demo').then((ms) => setStatus(`Демо идёт ~${Math.round(ms / 1000)} с`)))}>
              Запустить демо
            </button>
            <button type="button" className="btn" onClick={() => void run(() => call('simulate_stop'), 'Демо остановлено')}>
              Остановить
            </button>
          </div>
          <p className="dev-label">Состояние</p>
          <div className="dev-row wrap">
            {STATES.map((s) => (
              <button key={s} type="button" className="chip" onClick={() => void run(() => send({ type: 'state', state: s }))}>
                {STATE_LABEL[s]}
              </button>
            ))}
          </div>
          <p className="dev-label">Звук и субтитры</p>
          <div className="dev-row wrap">
            <button type="button" className="chip" onClick={() => burst('mic')}>
              Волна микрофона
            </button>
            <button type="button" className="chip" onClick={() => burst('tts')}>
              Голос Джарвиса
            </button>
            <button
              type="button"
              className="chip"
              onClick={() => void run(() => send({ type: 'subtitle', speaker: 'jarvis', text: 'Всё готово, сэр. Блокнот открыт.', is_final: true }))}
            >
              Субтитр
            </button>
          </div>
          <p className="dev-label">Управление ПК</p>
          <div className="dev-row wrap">
            <button type="button" className="chip" onClick={() => void run(() => send({ type: 'control_start', tier: 'T1' }))}>
              Свечение T1
            </button>
            <button type="button" className="chip" onClick={() => void run(() => send({ type: 'control_start', tier: 'T3' }))}>
              Свечение T3
            </button>
            <button
              type="button"
              className="chip"
              onClick={() =>
                void run(() => send({ type: 'control_target', rect: { x: 840, y: 520, w: 240, h: 44 }, label: 'Нажимаю: Сохранить' }))
              }
            >
              Цель
            </button>
            <button type="button" className="chip" onClick={() => void run(() => send({ type: 'control_stop' }))}>
              Погасить
            </button>
            <button
              type="button"
              className="chip"
              onClick={() =>
                void run(() =>
                  send({ type: 'confirm_request', id: `dev-${Date.now()}`, summary: 'Удалить папку C:\\temp\\old со всем содержимым?', tier: 'T3', timeout_ms: 20000 }),
                )
              }
            >
              Подтверждение
            </button>
          </div>
          <p className="dev-label">Данные</p>
          <div className="dev-row wrap">
            <button
              type="button"
              className="chip"
              onClick={() =>
                void run(() =>
                  send({ type: 'action', id: `dev-${Date.now()}`, tool: 'ui_click', summary: 'Нажимаю «Сохранить»', status: 'running' }),
                )
              }
            >
              Действие
            </button>
            <button
              type="button"
              className="chip"
              onClick={() =>
                void run(() =>
                  send({
                    type: 'sources',
                    items: [
                      { title: 'Microsoft Learn — Stop-Service', url: 'https://learn.microsoft.com/powershell/module/microsoft.powershell.management/stop-service' },
                      { title: 'Rust Blog', url: 'https://blog.rust-lang.org/' },
                    ],
                  }),
                )
              }
            >
              Источники
            </button>
            <button
              type="button"
              className="chip"
              onClick={() =>
                void run(() =>
                  send({ type: 'telemetry', cpu: 20 + Math.random() * 70, ram_used_mb: 6000 + Math.random() * 9000, ram_total_mb: 16384, gpu: Math.random() * 100 }),
                )
              }
            >
              Телеметрия
            </button>
            <button type="button" className="chip" onClick={() => void run(() => send({ type: 'error', message: 'Проверка: сервер речи недоступен' }))}>
              Ошибка
            </button>
          </div>
          {status && <p className="small muted">{status}</p>}
        </div>
      )}
    </aside>
  );
}
