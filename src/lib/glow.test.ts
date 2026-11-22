import { describe, expect, it } from 'vitest';

import { reactorParams, smoothLevel, targetFps } from '@/components/reactor/draw';

import { PALETTE, glowFor, reactorAccent, tierColor, withAlpha } from './glow';

describe('glowFor: цвет рамки оверлея', () => {
  it('нет управления — нет свечения', () => {
    expect(glowFor({ tier: null, confirmPending: false, assistant: 'idle' })).toBeNull();
  });

  it('T0 (чтение) не светится', () => {
    expect(glowFor({ tier: 'T0', confirmPending: false, assistant: 'controlling' })).toBeNull();
  });

  it('T1 и T2 — циан', () => {
    expect(glowFor({ tier: 'T1', confirmPending: false, assistant: 'controlling' })?.color).toBe(PALETTE.cyan);
    expect(glowFor({ tier: 'T2', confirmPending: false, assistant: 'controlling' })?.kind).toBe('cyan');
  });

  it('T3 — янтарь', () => {
    expect(glowFor({ tier: 'T3', confirmPending: false, assistant: 'controlling' })?.color).toBe(PALETTE.amber);
  });

  it('ожидание подтверждения — красный, даже без управления', () => {
    expect(glowFor({ tier: 'T3', confirmPending: true, assistant: 'controlling' })?.color).toBe(PALETTE.red);
    expect(glowFor({ tier: null, confirmPending: true, assistant: 'speaking' })?.kind).toBe('red');
  });

  it('ошибка во время управления — красный', () => {
    const g = glowFor({ tier: 'T1', confirmPending: false, assistant: 'error' });
    expect(g?.color).toBe(PALETTE.red);
    expect(g?.label).toBe('Ошибка');
  });

  it('тревожные цвета пульсируют чаще', () => {
    const cyan = glowFor({ tier: 'T1', confirmPending: false, assistant: 'controlling' })!;
    const amber = glowFor({ tier: 'T3', confirmPending: false, assistant: 'controlling' })!;
    const red = glowFor({ tier: 'T3', confirmPending: true, assistant: 'controlling' })!;
    expect(red.pulseMs).toBeLessThan(amber.pulseMs);
    expect(amber.pulseMs).toBeLessThan(cyan.pulseMs);
  });
});

describe('цвета по состоянию', () => {
  it('акцент реактора', () => {
    expect(reactorAccent('idle')).toBe(PALETTE.cyan);
    expect(reactorAccent('listening')).toBe(PALETTE.cyan);
    expect(reactorAccent('controlling')).toBe(PALETTE.amber);
    expect(reactorAccent('error')).toBe(PALETTE.red);
  });

  it('метка tier', () => {
    expect(tierColor('T3')).toBe(PALETTE.amber);
    expect(tierColor('T1')).toBe(PALETTE.cyan);
    expect(tierColor('T0')).toBe(PALETTE.steel);
  });

  it('withAlpha', () => {
    expect(withAlpha('#4FE3FF', 0.5)).toBe('rgba(79, 227, 255, 0.5)');
    expect(withAlpha('#fff', 2)).toBe('rgba(255, 255, 255, 1)');
  });
});

describe('реактор: параметры состояний', () => {
  it('думает — быстрое вращение и сканирующая дуга', () => {
    const idle = reactorParams('idle', 1, 0, 0.5);
    const think = reactorParams('thinking', 1, 0, 0.5);
    expect(think.speed).toBeGreaterThan(idle.speed * 3);
    expect(think.scan).toBe(true);
    expect(idle.scan).toBe(false);
  });

  it('слушает — волна растёт с уровнем микрофона; говорит — пульс в такт', () => {
    expect(reactorParams('listening', 0, 1, 0).wave).toBeGreaterThan(reactorParams('listening', 0, 0, 0).wave);
    expect(reactorParams('speaking', 0, 0.8, 0).pulse).toBeCloseTo(0.8);
  });

  it('ожидание дышит (яркость меняется во времени)', () => {
    const a = reactorParams('idle', 0, 0, 0).glow;
    const b = reactorParams('idle', 1.25, 0, 0).glow;
    expect(Math.abs(a - b)).toBeGreaterThan(0.05);
  });

  it('управление — янтарные акценты, ошибка — красное мерцание', () => {
    expect(reactorParams('controlling', 0, 0, 0).accent).toBe(PALETTE.amber);
    const err = reactorParams('error', 0, 0, 0.95);
    expect(err.base).toBe(PALETTE.red);
    expect(err.alpha).toBeLessThan(0.5);
  });

  it('сглаживание уровня: быстрая атака, медленный спад', () => {
    const up = smoothLevel(0, 1, 1 / 60);
    const down = 1 - smoothLevel(1, 0, 1 / 60);
    expect(up).toBeGreaterThan(down);
  });

  it('FPS: HUD 60, виджет 15 в простое и 30 при активности, 0 когда окно скрыто', () => {
    expect(targetFps('hud', 'idle', false)).toBe(60);
    expect(targetFps('corner', 'idle', false)).toBe(15);
    expect(targetFps('corner', 'speaking', false)).toBe(30);
    expect(targetFps('hud', 'speaking', true)).toBe(0);
  });
});
