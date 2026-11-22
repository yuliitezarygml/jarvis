// Темы J.A.R.V.I.S.: акцентный цвет всех окон (CSS-переменные + палитра канваса реактора).
// Сигнальные цвета — янтарь (T3) и красный (подтверждение/ошибка) — темой НЕ меняются: это безопасность.

import { PALETTE } from './glow';
import type { AppearanceConfig } from './types';

export interface ThemePreset {
  id: string;
  name: string;
  desc: string;
  accent: string;
  ice: string;
  steel: string;
  void: string;
  abyss: string;
  deep: string;
  text: string;
  muted: string;
}

export const THEMES: ThemePreset[] = [
  {
    id: 'arc-reactor',
    name: 'Arc Reactor',
    desc: 'Классический циан, как в фильме',
    accent: '#4FE3FF',
    ice: '#B8F6FF',
    steel: '#1B6E86',
    void: '#02080D',
    abyss: '#061520',
    deep: '#0A2231',
    text: '#D3F3FB',
    muted: '#7AA3B2',
  },
  {
    id: 'mark-iii',
    name: 'Mark III',
    desc: 'Золото и красный хот-род',
    accent: '#FFCB5C',
    ice: '#FFEFC4',
    steel: '#9A2A1E',
    void: '#0B0504',
    abyss: '#1A0A07',
    deep: '#2B100B',
    text: '#FBEAD6',
    muted: '#B9998A',
  },
  {
    id: 'emerald',
    name: 'Изумруд',
    desc: 'Зелёное свечение',
    accent: '#3DFFAE',
    ice: '#C4FFE6',
    steel: '#18785A',
    void: '#020A07',
    abyss: '#061A12',
    deep: '#0A2A1E',
    text: '#D4FBEA',
    muted: '#7DB29C',
  },
  {
    id: 'violet',
    name: 'Фиолет',
    desc: 'Глубокий фиолетовый',
    accent: '#B98CFF',
    ice: '#E7DBFF',
    steel: '#5A3C9C',
    void: '#06040D',
    abyss: '#110B22',
    deep: '#1C1336',
    text: '#E9E0FB',
    muted: '#A091C2',
  },
  {
    id: 'white-ice',
    name: 'Белый лёд',
    desc: 'Холодный белый, почти монохром',
    accent: '#E4F4FF',
    ice: '#FFFFFF',
    steel: '#5F7C8C',
    void: '#05080B',
    abyss: '#0E151B',
    deep: '#18232C',
    text: '#F1F6FA',
    muted: '#9BAAB5',
  },
];

export const DEFAULT_THEME = THEMES[0];

export const DEFAULT_APPEARANCE: AppearanceConfig = {
  theme: DEFAULT_THEME.id,
  accent: null,
  glow: 1,
  animation_speed: 1,
  corner_size: 'medium',
  scanlines: true,
  show_telemetry: true,
  show_actions: true,
  show_sources: true,
  show_subtitles: true,
};

// ───────────── цвет ─────────────

export function isHex(s: string | null | undefined): s is string {
  return typeof s === 'string' && /^#[0-9a-fA-F]{6}$/.test(s.trim());
}

export function hexToRgb(hex: string): [number, number, number] {
  const n = parseInt(hex.trim().replace('#', ''), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

export function rgbToHex([r, g, b]: [number, number, number]): string {
  const h = (v: number) => Math.round(Math.min(255, Math.max(0, v))).toString(16).padStart(2, '0');
  return `#${h(r)}${h(g)}${h(b)}`.toUpperCase();
}

/** Смесь `a` и `b`: t=0 → a, t=1 → b. */
export function mix(a: string, b: string, t: number): string {
  const x = hexToRgb(a);
  const y = hexToRgb(b);
  return rgbToHex([x[0] + (y[0] - x[0]) * t, x[1] + (y[1] - x[1]) * t, x[2] + (y[2] - x[2]) * t]);
}

/** Относительная яркость WCAG 2. */
export function luminance(hex: string): number {
  const lin = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  const [r, g, b] = hexToRgb(hex);
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** Контраст WCAG (1…21). */
export function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** Осветлить `fg`, пока он не станет читаемым на `bg` (текст и ссылки акцентным цветом). */
export function readable(fg: string, bg: string, min = 4.5): string {
  if (contrast(fg, bg) >= min) return fg.toUpperCase();
  for (let t = 0.05; t <= 1; t += 0.05) {
    const c = mix(fg, '#FFFFFF', t);
    if (contrast(c, bg) >= min) return c;
  }
  return '#FFFFFF';
}

/** Цвет текста поверх заливки акцентом. */
export function onAccent(accent: string): string {
  return contrast(accent, '#000000') >= contrast(accent, '#FFFFFF') ? '#02080D' : '#FFFFFF';
}

/** Тон (°) и насыщенность HSL. */
export function hueSat(hex: string): { h: number; s: number; l: number } {
  const [r, g, b] = hexToRgb(hex).map((v) => v / 255) as [number, number, number];
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const d = max - min;
  if (d === 0) return { h: 0, s: 0, l };
  const s = d / (1 - Math.abs(2 * l - 1));
  let h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  h *= 60;
  if (h < 0) h += 360;
  return { h, s, l };
}

const hueDist = (a: number, b: number) => Math.min(Math.abs(a - b), 360 - Math.abs(a - b));

/** Акцент похож на сигнальный янтарь (T3) или красный (подтверждение/ошибка)? */
export function signalClash(accent: string): 'amber' | 'red' | null {
  const { h, s } = hueSat(accent);
  if (s < 0.45) return null;
  if (hueDist(h, hueSat(PALETTE.amber).h) < 24) return 'amber';
  if (hueDist(h, hueSat(PALETTE.red).h) < 24) return 'red';
  return null;
}

// ───────────── палитра темы ─────────────

export interface Palette {
  /** Акцент как выбран (канвас реактора). */
  accent: string;
  /** Акцент для текста и рамок в CSS (не темнее контраста 4.5:1 на фоне). */
  accentText: string;
  ice: string;
  steel: string;
  void: string;
  abyss: string;
  deep: string;
  text: string;
  muted: string;
  /** Рамка действий T1/T2 на оверлее: акцент, если он не похож на сигнальные цвета. */
  overlay: string;
}

export function themeById(id: string | undefined): ThemePreset {
  return THEMES.find((t) => t.id === id) ?? DEFAULT_THEME;
}

export function paletteFor(ap: Partial<AppearanceConfig> | null | undefined): Palette {
  const t = themeById(ap?.theme);
  const custom = isHex(ap?.accent) ? ap.accent.trim().toUpperCase() : null;
  const accent = custom ?? t.accent;
  const ice = custom ? mix(custom, '#FFFFFF', 0.62) : t.ice;
  const steel = custom ? mix(custom, t.void, 0.6) : t.steel;
  return {
    accent,
    accentText: readable(accent, t.void),
    ice: readable(ice, t.void, 7),
    steel,
    void: t.void,
    abyss: t.abyss,
    deep: t.deep,
    text: t.text,
    muted: readable(t.muted, t.void, 4.5),
    overlay: signalClash(accent) ? DEFAULT_THEME.accent : accent,
  };
}

const clamp = (v: unknown, lo: number, hi: number, def: number) => (typeof v === 'number' && Number.isFinite(v) ? Math.min(hi, Math.max(lo, v)) : def);

export function normalizeAppearance(ap: Partial<AppearanceConfig> | null | undefined): AppearanceConfig {
  const a = { ...DEFAULT_APPEARANCE, ...(ap ?? {}) };
  return {
    ...a,
    theme: themeById(a.theme).id,
    accent: isHex(a.accent) ? a.accent.trim().toUpperCase() : null,
    glow: clamp(a.glow, 0.3, 1.5, 1),
    animation_speed: clamp(a.animation_speed, 0.25, 2, 1),
    corner_size: ['small', 'medium', 'large'].includes(a.corner_size) ? a.corner_size : 'medium',
  };
}

const rgb = (hex: string) => hexToRgb(hex).join(', ');

/** CSS-переменные темы (globals.css читает их на :root). */
export function cssVars(ap: Partial<AppearanceConfig> | null | undefined): Record<string, string> {
  const a = normalizeAppearance(ap);
  const p = paletteFor(a);
  return {
    '--cyan': p.accentText,
    '--accent-raw': p.accent,
    '--accent-rgb': rgb(p.accentText),
    '--on-accent': onAccent(p.accentText),
    '--ice': p.ice,
    '--steel': p.steel,
    '--void': p.void,
    '--void-rgb': rgb(p.void),
    '--abyss': p.abyss,
    '--abyss-rgb': rgb(p.abyss),
    '--deep': p.deep,
    '--text': p.text,
    '--muted': p.muted,
    '--muted-rgb': rgb(p.muted),
    '--line': `rgba(${rgb(p.accentText)}, 0.2)`,
    '--line-strong': `rgba(${rgb(p.accentText)}, 0.45)`,
    '--panel': `rgba(${rgb(p.abyss)}, 0.72)`,
    '--glow-k': String(a.glow),
    '--anim-k': String(a.animation_speed),
  };
}

// ───────────── живая тема окна ─────────────

let live = { palette: paletteFor(DEFAULT_APPEARANCE), glow: 1, speed: 1 };
type Listener = () => void;
const listeners = new Set<Listener>();

/** Палитра и движение для канваса (реактор, волна, гекс-сетка) — читаются в каждом кадре. */
export function livePalette(): Palette {
  return live.palette;
}
export function liveMotion(): { glow: number; speed: number } {
  return { glow: live.glow, speed: live.speed };
}
export function onThemeChange(l: Listener): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

/** Применить внешний вид к окну: CSS-переменные, атрибуты панелей HUD, палитра канваса. */
export function applyAppearance(ap: Partial<AppearanceConfig> | null | undefined, root: HTMLElement = document.documentElement) {
  const a = normalizeAppearance(ap);
  for (const [k, v] of Object.entries(cssVars(a))) root.style.setProperty(k, v);
  const flags: [string, boolean][] = [
    ['data-no-scanlines', !a.scanlines],
    ['data-hide-telemetry', !a.show_telemetry],
    ['data-hide-actions', !a.show_actions],
    ['data-hide-sources', !a.show_sources],
    ['data-hide-subtitles', !a.show_subtitles],
  ];
  for (const [attr, on] of flags) {
    if (on) root.setAttribute(attr, '');
    else root.removeAttribute(attr);
  }
  live = { palette: paletteFor(a), glow: a.glow, speed: a.animation_speed };
  listeners.forEach((l) => l());
}
