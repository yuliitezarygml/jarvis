import { describe, expect, it } from 'vitest';

import { fmtClock, fmtCountdown, fmtCtx, fmtLatency, fmtMb, fmtNet, fmtNumber, fmtPercent, fmtRam, loadLevel, ramPercent, sparkPoints } from './format';
import { intersectsOverlay, labelPlacement, toOverlayBox, toOverlayPoint } from './geometry';
import { maskKey } from './mock';
import { addProvider, addToChain, applyRanking, isDirty, moveItem, removeProvider } from './settings';
import { defaultConfig } from './mock';

const NB = ' ';

describe('форматирование телеметрии', () => {
  it('проценты', () => {
    expect(fmtPercent(12.6)).toBe(`13${NB}%`);
    expect(fmtPercent(140)).toBe(`100${NB}%`);
    expect(fmtPercent(null)).toBe('—');
    expect(fmtPercent(NaN)).toBe('—');
  });

  it('память', () => {
    expect(fmtMb(512)).toBe(`512${NB}МБ`);
    expect(fmtMb(8000)).toBe(`7,8${NB}ГБ`);
    expect(fmtRam(8000, 16384)).toBe(`7,8 / 16,0${NB}ГБ`);
    expect(fmtRam(1, 0)).toBe('—');
    expect(ramPercent(8192, 16384)).toBe(50);
    expect(ramPercent(1, 0)).toBe(0);
  });

  it('сеть, задержка, контекст, таймер, часы', () => {
    expect(fmtNet(850)).toBe(`850${NB}кбит/с`);
    expect(fmtNet(12400)).toBe(`12,4${NB}Мбит/с`);
    expect(fmtNet(null)).toBe('—');
    expect(fmtLatency(380)).toBe(`380${NB}мс`);
    expect(fmtLatency(1250)).toBe(`1,3${NB}с`);
    expect(fmtCtx(131072)).toBe('128k');
    expect(fmtCtx(200000)).toBe('195k');
    expect(fmtCtx(2_097_152)).toBe('2,0M');
    expect(fmtCountdown(19_400)).toBe(`20${NB}с`);
    expect(fmtCountdown(-5)).toBe(`0${NB}с`);
    expect(fmtClock(new Date(2026, 10, 22, 9, 5, 3))).toBe('09:05:03');
    expect(fmtNumber(Infinity)).toBe('—');
  });

  it('уровни нагрузки', () => {
    expect(loadLevel(50)).toBe('ok');
    expect(loadLevel(70)).toBe('warn');
    expect(loadLevel(95)).toBe('crit');
    expect(loadLevel(null)).toBe('ok');
  });

  it('спарклайн', () => {
    expect(sparkPoints([], 100, 10)).toBe('');
    expect(sparkPoints([0, 100], 100, 10)).toBe('0.0,10.0 100.0,0.0');
    expect(sparkPoints([50], 100, 10)).toBe('0.0,5.0');
    expect(sparkPoints([500], 100, 10)).toBe('0.0,0.0');
  });
});

describe('геометрия оверлея', () => {
  const g = { x: 0, y: 0, width: 2880, height: 1620, scale: 1.5 };
  it('физические пиксели → CSS', () => {
    expect(toOverlayBox({ x: 300, y: 150, w: 120, h: 60 }, g)).toEqual({ left: 200, top: 100, width: 80, height: 40 });
    expect(toOverlayPoint(1500, 750, { x: 0, y: 0, scale: 1.5 })).toEqual({ x: 1000, y: 500 });
    expect(toOverlayBox({ x: 1920, y: 0, w: 10, h: 10 }, { x: 1920, y: 0, scale: 0 })).toEqual({ left: 0, top: 0, width: 10, height: 10 });
  });
  it('цель на другом мониторе не рисуется', () => {
    expect(intersectsOverlay({ x: 100, y: 100, w: 10, h: 10 }, g)).toBe(true);
    expect(intersectsOverlay({ x: 3000, y: 100, w: 10, h: 10 }, g)).toBe(false);
  });
  it('подпись цели: над рамкой, у верхнего края — под ней', () => {
    expect(labelPlacement({ left: 0, top: 200, width: 10, height: 10 }, 1000)).toBe('above');
    expect(labelPlacement({ left: 0, top: 5, width: 10, height: 10 }, 1000)).toBe('below');
  });
});

describe('настройки', () => {
  it('маска ключа как в jarvis-core', () => {
    expect(maskKey('gsk_abcdefghijklmnop1234')).toBe('gsk…1234');
    expect(maskKey('short')).toBe('•••••');
    expect(maskKey('')).toBe('');
    expect(maskKey('sk-proj-aaaaaaaaaaaaaaaaaaaaZZZZ')).not.toContain('aaaa');
  });

  it('провайдеры и цепочки', () => {
    const preset = { id: 'groq', name: 'Groq', kind: 'openai_compat' as const, base_url: 'https://g/v1', key_url: '', needs_key: true, note: '' };
    let c = addProvider(defaultConfig(), preset, ' key ');
    expect(c.providers.map((p) => p.id)).toEqual(['groq', 'groq-2']);
    expect(c.providers[1].api_key).toBe('key');
    c = { ...c, roles: addToChain(c.roles, 'fast', 'groq-2/m') };
    expect(addToChain(c.roles, 'fast', 'groq-2/m')).toBe(c.roles);
    c = removeProvider(c, 'groq');
    expect(c.roles.brain).toEqual([]);
    expect(c.roles.fast).toEqual(['groq-2/m']);
    expect(moveItem([1, 2, 3], 0, 1)).toEqual([2, 1, 3]);
    expect(moveItem([1, 2, 3], 0, -1)).toEqual([1, 2, 3]);
    const roles = applyRanking({ brain: ['x/old'], fast: [], vision: [], reflection: [] }, [
      { role: 'brain', top: [{ model: 'g/best', reason: '' }, { model: 'x/old', reason: '' }] },
      { role: 'vision', top: [] },
    ]);
    expect(roles.brain).toEqual(['g/best', 'x/old']);
    expect(roles.vision).toEqual([]);
    expect(isDirty(defaultConfig(), defaultConfig())).toBe(false);
    expect(isDirty(defaultConfig(), { ...defaultConfig(), autonomy: 'never' })).toBe(true);
  });
});
