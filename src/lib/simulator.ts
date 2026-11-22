// Демо-сценарий для режима «просто браузер» (`npm run dev` без Tauri). В Tauri используется Rust-команда
// `simulate_demo` (src-tauri/src/simulator.rs) — тот же сценарий, события идут через настоящий мост.

import type { AssistantState, AudioSource, JarvisEvent, Speaker } from './types';

export type Timed = [number, JarvisEvent];

function lcg(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s / 0xffffffff;
  };
}

export function demoScript(): Timed[] {
  const out: Timed[] = [];
  const rnd = lcg(0x4a525653);
  const at = (t: number, e: JarvisEvent) => out.push([t, e]);
  const state = (t: number, s: AssistantState) => at(t, { type: 'state', state: s });
  const audio = (from: number, to: number, source: AudioSource, loud: number) => {
    for (let t = from; t < to; t += 33) {
      const p = (t - from) / 1000;
      const rms = Math.min(1, Math.max(0, loud * Math.abs(Math.sin(p * 7.3)) * (0.6 + 0.4 * Math.sin(p * 1.7)) + 0.06 * rnd()));
      at(t, { type: 'audio_level', source, rms });
    }
    at(to, { type: 'audio_level', source, rms: 0 });
  };
  const subtitle = (from: number, speaker: Speaker, text: string, wordMs: number) => {
    const words = text.split(' ');
    words.forEach((_, i) =>
      at(from + i * wordMs, { type: 'subtitle', speaker, text: words.slice(0, i + 1).join(' '), is_final: i === words.length - 1 }),
    );
  };
  const action = (t: number, id: string, tool: string, summary: string, status: Extract<JarvisEvent, { type: 'action' }>['status']) =>
    at(t, { type: 'action', id, tool, summary, status });

  state(0, 'idle');
  state(1500, 'listening');
  audio(1500, 4300, 'mic', 0.85);
  subtitle(1700, 'user', 'Джарвис, открой блокнот и напиши привет', 330);
  state(4400, 'thinking');
  state(6000, 'speaking');
  audio(6000, 7900, 'tts', 0.7);
  subtitle(6050, 'jarvis', 'Сию минуту, сэр.', 380);

  state(8000, 'controlling');
  at(8000, { type: 'control_start', tier: 'T1' });
  action(8100, 'demo-1', 'open_target', 'Открываю Блокнот', 'running');
  at(8300, { type: 'control_target', rect: { x: 0, y: 1032, w: 48, h: 48 }, label: 'Нажимаю: Пуск' });
  at(9400, { type: 'control_target', rect: { x: 620, y: 180, w: 680, h: 52 }, label: 'Ввожу: блокнот' });
  action(10400, 'demo-1', 'open_target', 'Открыл Блокнот', 'done');
  action(10500, 'demo-2', 'ui_type', 'Пишу «привет»', 'running');
  at(10500, { type: 'control_target', rect: { x: 360, y: 260, w: 1200, h: 560 }, label: 'Ввожу: привет' });
  action(11900, 'demo-2', 'ui_type', 'Написал «привет»', 'done');
  action(12000, 'demo-3', 'ui_click', 'Сохраняю документ', 'running');
  at(12000, { type: 'control_target', rect: { x: 1180, y: 860, w: 120, h: 36 }, label: 'Нажимаю: Сохранить' });
  action(13200, 'demo-3', 'ui_click', 'Документ сохранён', 'done');
  at(13400, { type: 'control_stop' });

  state(13500, 'thinking');
  action(13600, 'demo-4', 'web_search', 'Ищу: что нового в Rust', 'running');
  action(15000, 'demo-4', 'web_search', 'Нашёл 8 результатов', 'done');
  at(15100, {
    type: 'sources',
    items: [
      { title: 'Rust Blog — Announcing Rust 1.100', url: 'https://blog.rust-lang.org/' },
      { title: 'This Week in Rust', url: 'https://this-week-in-rust.org/' },
      { title: 'Rust Release Notes', url: 'https://doc.rust-lang.org/releases.html' },
    ],
  });
  state(15300, 'speaking');
  audio(15300, 19000, 'tts', 0.75);
  subtitle(15350, 'jarvis', 'Вот что нашёл, сэр: вышла новая версия компилятора, источники на экране.', 300);

  state(19400, 'controlling');
  at(19400, { type: 'control_start', tier: 'T3' });
  action(19500, 'demo-5', 'service_stop', 'Остановить службу Spooler', 'awaiting_confirm');
  at(19600, { type: 'confirm_request', id: 'demo-confirm', summary: 'Остановить службу печати (Spooler)?', tier: 'T3', timeout_ms: 20000 });
  audio(19700, 22000, 'tts', 0.6);
  subtitle(19700, 'jarvis', 'Сэр, остановить службу печати? Скажите «да» или нажмите кнопку.', 260);
  at(26500, { type: 'confirm_resolved', id: 'demo-confirm', approved: true });
  action(26600, 'demo-5', 'service_stop', 'Останавливаю службу Spooler', 'running');
  at(26700, { type: 'control_target', rect: { x: 700, y: 400, w: 520, h: 280 }, label: 'Останавливаю: Spooler' });
  action(28000, 'demo-5', 'service_stop', 'Служба Spooler остановлена', 'done');
  action(28100, 'demo-6', 'powershell_run', 'Проверка состояния службы — не удалось', 'failed');
  at(28300, { type: 'control_stop' });

  at(28600, { type: 'error', message: 'Сервер распознавания речи не отвечает (демо)' });
  state(28600, 'error');
  state(31500, 'idle');
  action(31600, 'demo-7', 'kb_search', 'Отменено пользователем', 'cancelled');

  let cpu = 9;
  let ram = 9800;
  for (let t = 0; t <= 32000; t += 1000) {
    cpu = Math.min(96, Math.max(3, cpu + (rnd() - 0.45) * 14));
    ram = Math.min(15500, Math.max(6000, ram + (rnd() - 0.5) * 300));
    at(t, { type: 'telemetry', cpu, ram_used_mb: Math.round(ram), ram_total_mb: 16384, gpu: Math.min(100, cpu * 0.6 + rnd() * 10) });
  }
  // стабильная сортировка по времени
  return out.map((x, i) => [x, i] as const).sort((a, b) => a[0][0] - b[0][0] || a[1] - b[1]).map(([x]) => x);
}

/** Проиграть сценарий в `emit`. Возвращает функцию остановки (гасит свечение, возвращает idle). */
export function playDemo(emit: (e: JarvisEvent) => void): () => void {
  const timers: ReturnType<typeof setTimeout>[] = [];
  for (const [t, e] of demoScript()) timers.push(setTimeout(() => emit(e), t));
  let stopped = false;
  return () => {
    if (stopped) return;
    stopped = true;
    timers.forEach(clearTimeout);
    emit({ type: 'control_stop' });
    emit({ type: 'state', state: 'idle' });
  };
}
