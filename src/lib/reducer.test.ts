import { describe, expect, it } from 'vitest';

import { MAX_ACTIONS, TELEMETRY_POINTS, confirmRemainingMs, initialState, newestFirst, parseEvent, reduce, reduceAll } from './reducer';
import { demoScript } from './simulator';
import type { JarvisEvent } from './types';

const T = 1_000_000;

describe('reduce: все типы событий из jarvis-core', () => {
  it('state', () => {
    const s = reduce(initialState, { type: 'state', state: 'thinking' }, T);
    expect(s.assistant).toBe('thinking');
    expect(s.stateSince).toBe(T);
    // то же состояние — тот же объект (без лишних перерисовок)
    expect(reduce(s, { type: 'state', state: 'thinking' }, T + 5)).toBe(s);
  });

  it('subtitle: промежуточные не попадают в историю, финальные попадают', () => {
    let s = reduce(initialState, { type: 'subtitle', speaker: 'user', text: 'Джарвис,', is_final: false }, T);
    expect(s.subtitle).toEqual({ speaker: 'user', text: 'Джарвис,', final: false, at: T });
    expect(s.subtitleHistory).toHaveLength(0);
    s = reduce(s, { type: 'subtitle', speaker: 'user', text: 'Джарвис, привет', is_final: true }, T + 1);
    expect(s.subtitleHistory.map((l) => l.text)).toEqual(['Джарвис, привет']);
  });

  it('audio_level: раздельно для микрофона и TTS, с ограничением 0..1', () => {
    let s = reduce(initialState, { type: 'audio_level', source: 'mic', rms: 0.4 }, T);
    s = reduce(s, { type: 'audio_level', source: 'tts', rms: 7 }, T + 1);
    expect(s.audio).toEqual({ mic: 0.4, tts: 1, lastSource: 'tts', at: T + 1 });
    s = reduce(s, { type: 'audio_level', source: 'mic', rms: -3 }, T + 2);
    expect(s.audio.mic).toBe(0);
  });

  it('control_start / control_target / control_stop', () => {
    let s = reduce(initialState, { type: 'control_start', tier: 'T3' }, T);
    expect(s.control).toEqual({ active: true, tier: 'T3', since: T, target: null });
    const rect = { x: 10, y: 20, w: 30, h: 40 };
    s = reduce(s, { type: 'control_target', rect, label: 'Нажимаю: Сохранить' }, T + 10);
    expect(s.control.target).toEqual({ rect, label: 'Нажимаю: Сохранить', at: T + 10 });
    expect(s.control.since).toBe(T);
    s = reduce(s, { type: 'control_stop' }, T + 20);
    expect(s.control).toEqual({ active: false, tier: null, since: T + 20, target: null });
    expect(reduce(s, { type: 'control_stop' }, T + 30)).toBe(s);
  });

  it('action: новая строка, обновление по id, ограничение длины', () => {
    let s = reduce(initialState, { type: 'action', id: 'a', tool: 'ui_click', summary: 'Клик', status: 'running' }, T);
    s = reduce(s, { type: 'action', id: 'a', tool: 'ui_click', summary: 'Готово', status: 'done' }, T + 5);
    expect(s.actions).toHaveLength(1);
    expect(s.actions[0]).toMatchObject({ id: 'a', summary: 'Готово', status: 'done', startedAt: T, updatedAt: T + 5 });
    for (let i = 0; i < MAX_ACTIONS + 10; i++) {
      s = reduce(s, { type: 'action', id: `n${i}`, tool: 't', summary: 's', status: 'done' }, T + i);
    }
    expect(s.actions).toHaveLength(MAX_ACTIONS);
    expect(newestFirst(s.actions)[0].id).toBe(`n${MAX_ACTIONS + 9}`);
  });

  it('confirm_request / confirm_resolved: закрывается только свой id', () => {
    let s = reduce(initialState, { type: 'confirm_request', id: 'c1', summary: 'Удалить?', tier: 'T3', timeout_ms: 20000 }, T);
    expect(s.confirm).toEqual({ id: 'c1', summary: 'Удалить?', tier: 'T3', timeoutMs: 20000, receivedAt: T });
    expect(confirmRemainingMs(s.confirm!, T + 5000)).toBe(15000);
    expect(confirmRemainingMs(s.confirm!, T + 99999)).toBe(0);
    s = reduce(s, { type: 'confirm_resolved', id: 'other', approved: true }, T + 1);
    expect(s.confirm?.id).toBe('c1');
    s = reduce(s, { type: 'confirm_resolved', id: 'c1', approved: false }, T + 2);
    expect(s.confirm).toBeNull();
    expect(s.lastConfirm).toEqual({ id: 'c1', approved: false, at: T + 2 });
  });

  it('telemetry: текущие значения и история (gpu = null не портит историю)', () => {
    let s = reduce(initialState, { type: 'telemetry', cpu: 12.5, ram_used_mb: 8192, ram_total_mb: 16384, gpu: null }, T);
    expect(s.telemetry).toEqual({ cpu: 12.5, ramUsedMb: 8192, ramTotalMb: 16384, gpu: null, netKbps: null, at: T });
    expect(s.telemetryHistory.ram).toEqual([50]);
    expect(s.telemetryHistory.gpu).toEqual([]);
    for (let i = 0; i < TELEMETRY_POINTS + 5; i++) {
      s = reduce(s, { type: 'telemetry', cpu: i, ram_used_mb: 1, ram_total_mb: 2, gpu: 40, net_kbps: 900 }, T + i);
    }
    expect(s.telemetryHistory.cpu).toHaveLength(TELEMETRY_POINTS);
    expect(s.telemetryHistory.cpu.at(-1)).toBe(TELEMETRY_POINTS + 4);
    expect(s.telemetryHistory.gpu.at(-1)).toBe(40);
    expect(s.telemetry?.netKbps).toBe(900);
  });

  it('sources', () => {
    const items = [{ title: 'Rust', url: 'https://www.rust-lang.org' }];
    expect(reduce(initialState, { type: 'sources', items }, T).sources).toEqual(items);
  });

  it('error', () => {
    const s = reduce(initialState, { type: 'error', message: 'сбой' }, T);
    expect(s.lastError).toEqual({ message: 'сбой', at: T });
    expect(s.errors).toHaveLength(1);
  });
});

describe('reduce: устойчивость', () => {
  it('неизвестный тип события игнорируется', () => {
    expect(reduce(initialState, { type: 'emergency_stop' }, T)).toBe(initialState);
    expect(reduce(initialState, { type: 'something_new', foo: 1 }, T)).toBe(initialState);
  });

  it('битые и чужие данные игнорируются', () => {
    const junk: unknown[] = [
      null,
      undefined,
      42,
      'state',
      {},
      { type: 'state', state: 'dancing' },
      { type: 'subtitle', speaker: 'cat', text: 'x', is_final: true },
      { type: 'audio_level', source: 'mic', rms: 'loud' },
      { type: 'control_start', tier: 'T9' },
      { type: 'control_target', rect: { x: 1 }, label: 'x' },
      { type: 'action', id: 'a', tool: 't', summary: 's', status: 'exploded' },
      { type: 'confirm_request', id: 'c', summary: 's', tier: 'T3' },
      { type: 'telemetry', cpu: NaN, ram_used_mb: 1, ram_total_mb: 1, gpu: null },
      { type: 'sources', items: [{ title: 1 }] },
      { type: 'error' },
    ];
    for (const j of junk) expect(reduce(initialState, j, T)).toBe(initialState);
  });

  it('parseEvent пропускает корректные события без изменений', () => {
    const e: JarvisEvent = { type: 'control_target', rect: { x: 1, y: 2, w: 3, h: 4 }, label: 'L' };
    expect(parseEvent(e)).toBe(e);
  });
});

describe('демо-сценарий через редьюсер', () => {
  it('содержит все типы событий и заканчивается в покое', () => {
    const script = demoScript();
    const types = new Set(script.map(([, e]) => e.type));
    for (const t of [
      'state',
      'subtitle',
      'audio_level',
      'control_start',
      'control_target',
      'control_stop',
      'action',
      'confirm_request',
      'confirm_resolved',
      'telemetry',
      'sources',
      'error',
    ]) {
      expect(types.has(t as JarvisEvent['type'])).toBe(true);
    }
    expect(script.every(([t], i) => i === 0 || script[i - 1][0] <= t)).toBe(true);
    const end = reduceAll(initialState, script.map(([, e]) => e), T);
    expect(end.assistant).toBe('idle');
    expect(end.control.active).toBe(false);
    expect(end.confirm).toBeNull();
    expect(end.sources).toHaveLength(3);
    expect(end.actions.find((a) => a.id === 'demo-5')?.status).toBe('done');
  });
});
