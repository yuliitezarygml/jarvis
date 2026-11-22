import { describe, expect, it } from 'vitest';

import { PALETTE, glowFor } from './glow';
import {
  DEFAULT_APPEARANCE,
  THEMES,
  contrast,
  cssVars,
  hexToRgb,
  isHex,
  mix,
  normalizeAppearance,
  onAccent,
  paletteFor,
  readable,
  rgbToHex,
  signalClash,
} from './theme';

describe('цвета', () => {
  it('hex ↔ rgb и смешивание', () => {
    expect(hexToRgb('#4FE3FF')).toEqual([79, 227, 255]);
    expect(rgbToHex([79, 227, 255])).toBe('#4FE3FF');
    expect(mix('#000000', '#FFFFFF', 0.5)).toBe('#808080');
    expect(isHex('#12ab9f')).toBe(true);
    expect(isHex('#12ab9')).toBe(false);
    expect(isHex(null)).toBe(false);
  });

  it('контраст WCAG', () => {
    expect(contrast('#000000', '#FFFFFF')).toBeCloseTo(21, 0);
    expect(contrast('#777777', '#777777')).toBeCloseTo(1, 5);
  });

  it('тёмный акцент осветляется до читаемого', () => {
    const dark = '#202060';
    expect(contrast(dark, '#02080D')).toBeLessThan(4.5);
    const r = readable(dark, '#02080D');
    expect(contrast(r, '#02080D')).toBeGreaterThanOrEqual(4.5);
    expect(readable('#4FE3FF', '#02080D')).toBe('#4FE3FF');
    expect(onAccent('#4FE3FF')).toBe('#02080D');
    expect(onAccent('#202060')).toBe('#FFFFFF');
  });
});

describe('темы', () => {
  it('у каждой темы читаемый текст и уникальный id', () => {
    expect(new Set(THEMES.map((t) => t.id)).size).toBe(THEMES.length);
    for (const t of THEMES) {
      const p = paletteFor({ theme: t.id });
      expect(contrast(p.accentText, p.void), t.id).toBeGreaterThanOrEqual(4.5);
      expect(contrast(p.text, p.void), t.id).toBeGreaterThanOrEqual(7);
      expect(contrast(p.muted, p.void), t.id).toBeGreaterThanOrEqual(4.5);
    }
  });

  it('по умолчанию — Arc Reactor, как раньше', () => {
    const p = paletteFor(DEFAULT_APPEARANCE);
    expect(p.accent).toBe(PALETTE.cyan);
    expect(p.ice).toBe(PALETTE.ice);
    expect(cssVars(DEFAULT_APPEARANCE)['--accent-rgb']).toBe('79, 227, 255');
    expect(paletteFor({ theme: 'нет-такой' }).accent).toBe(PALETTE.cyan);
  });

  it('свой акцент поверх темы', () => {
    const p = paletteFor({ theme: 'violet', accent: '#00ff88' });
    expect(p.accent).toBe('#00FF88');
    expect(p.void).toBe(THEMES.find((t) => t.id === 'violet')!.void);
    expect(paletteFor({ accent: 'мусор' }).accent).toBe(PALETTE.cyan);
  });

  it('нормализация: пределы и мусор', () => {
    const a = normalizeAppearance({ glow: 9, animation_speed: -1, corner_size: 'huge' as never, accent: '#abc' });
    expect([a.glow, a.animation_speed, a.corner_size, a.accent]).toEqual([1.5, 0.25, 'medium', null]);
    expect(normalizeAppearance(undefined)).toEqual(DEFAULT_APPEARANCE);
  });
});

describe('сигнальные цвета не перекрашиваются', () => {
  it('акцент, похожий на янтарь или красный, не попадает в рамку действий', () => {
    expect(signalClash('#FFCB5C')).toBe('amber'); // Mark III
    expect(signalClash('#FF3355')).toBe('red');
    expect(signalClash('#4FE3FF')).toBeNull();
    expect(signalClash('#E4F4FF')).toBeNull();
    expect(paletteFor({ theme: 'mark-iii' }).overlay).toBe(PALETTE.cyan);
    expect(paletteFor({ theme: 'emerald' }).overlay).toBe('#3DFFAE');
  });

  it('T1/T2 — цвет темы, T3 и подтверждение — всегда свои', () => {
    const accent = paletteFor({ theme: 'violet' }).overlay;
    expect(glowFor({ tier: 'T1', confirmPending: false, assistant: 'controlling' }, accent)?.color).toBe(accent);
    expect(glowFor({ tier: 'T3', confirmPending: false, assistant: 'controlling' }, accent)?.color).toBe(PALETTE.amber);
    expect(glowFor({ tier: 'T1', confirmPending: true, assistant: 'controlling' }, accent)?.color).toBe(PALETTE.red);
  });
});
