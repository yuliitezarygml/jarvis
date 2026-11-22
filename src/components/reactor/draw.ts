// Реактор J.A.R.V.I.S. на Canvas 2D: сегментированные кольца с разной скоростью, штрихи-деления, дуги,
// радиальная волна от уровня звука, сканирующая дуга, ядро с треугольником. Чистые функции — без React.

import { PALETTE, withAlpha } from '@/lib/glow';
import type { AssistantState } from '@/lib/types';

export interface ReactorParams {
  /** Множитель скорости вращения колец. */
  speed: number;
  /** Яркость свечения 0..1. */
  glow: number;
  /** Основной цвет линий. */
  base: string;
  /** Цвет акцентов (янтарь при управлении ПК, красный при ошибке). */
  accent: string;
  /** Амплитуда радиальной волны 0..1. */
  wave: number;
  /** Пульс ядра 0..1 (в такт голосу). */
  pulse: number;
  /** Сканирующая дуга (думает). */
  scan: boolean;
  /** Общая прозрачность (мерцание при ошибке). */
  alpha: number;
}

/** Параметры кадра по состоянию. `t` — секунды, `level` — сглаженный уровень звука 0..1, `rand` — 0..1. */
export function reactorParams(state: AssistantState, t: number, level: number, rand: number): ReactorParams {
  const breath = 0.5 + 0.5 * Math.sin(t * 1.25);
  const lv = Math.min(1, Math.max(0, level));
  switch (state) {
    case 'listening':
      return { speed: 1.35, glow: 0.85 + 0.15 * lv, base: PALETTE.cyan, accent: PALETTE.ice, wave: 0.22 + 0.78 * lv, pulse: lv * 0.4, scan: false, alpha: 1 };
    case 'thinking':
      return { speed: 4.2, glow: 0.8, base: PALETTE.cyan, accent: PALETTE.ice, wave: 0.1, pulse: 0.15 * breath, scan: true, alpha: 1 };
    case 'speaking':
      return { speed: 1.4, glow: 0.7 + 0.3 * lv, base: PALETTE.cyan, accent: PALETTE.ice, wave: 0.12 + 0.75 * lv, pulse: lv, scan: false, alpha: 1 };
    case 'controlling':
      return { speed: 1.9, glow: 0.85, base: PALETTE.cyan, accent: PALETTE.amber, wave: 0.15 + 0.4 * lv, pulse: 0.2 * breath, scan: false, alpha: 1 };
    case 'error':
      return { speed: 0.35, glow: 0.75, base: PALETTE.red, accent: PALETTE.red, wave: 0.06, pulse: 0, scan: false, alpha: rand > 0.86 ? 0.35 : 0.9 + 0.1 * rand };
    case 'idle':
    default:
      return { speed: 1, glow: 0.5 + 0.22 * breath, base: PALETTE.cyan, accent: PALETTE.ice, wave: 0.06 + 0.3 * lv, pulse: 0.25 * breath, scan: false, alpha: 1 };
  }
}

/** Сглаживание уровня звука: быстрая атака, медленный спад. `dt` — секунды. */
export function smoothLevel(prev: number, target: number, dt: number): number {
  const k = target > prev ? 22 : 5;
  return prev + (target - prev) * Math.min(1, dt * k);
}

/** Целевой FPS: HUD — 60; виджет — 15 в простое, 30 при активности; скрытое окно — 0. */
export function targetFps(variant: 'hud' | 'corner', state: AssistantState, hidden: boolean): number {
  if (hidden) return 0;
  if (variant === 'hud') return 60;
  return state === 'idle' ? 15 : 30;
}

const TAU = Math.PI * 2;

/** Двойной штрих: широкий полупрозрачный «ореол» + тонкая яркая линия (дешевле shadowBlur). */
function glowStroke(ctx: CanvasRenderingContext2D, color: string, width: number, glow: number) {
  ctx.strokeStyle = withAlpha(color, 0.16 * glow);
  ctx.lineWidth = width * 3.2;
  ctx.stroke();
  ctx.strokeStyle = withAlpha(color, 0.55 + 0.45 * glow);
  ctx.lineWidth = width;
  ctx.stroke();
}

function arcPath(ctx: CanvasRenderingContext2D, r: number, a0: number, a1: number) {
  ctx.beginPath();
  ctx.arc(0, 0, r, a0, a1);
}

/** Нарисовать кадр. Канвас уже масштабирован на devicePixelRatio; `w`,`h` — CSS-пиксели. */
export function drawReactor(ctx: CanvasRenderingContext2D, w: number, h: number, t: number, p: ReactorParams) {
  ctx.clearRect(0, 0, w, h);
  const R = (Math.min(w, h) / 2) * 0.92;
  if (R <= 4) return;
  const s = p.speed;

  ctx.save();
  ctx.translate(w / 2, h / 2);
  ctx.globalAlpha = p.alpha;
  ctx.globalCompositeOperation = 'lighter';
  ctx.lineCap = 'butt';

  // ореол
  const halo = ctx.createRadialGradient(0, 0, R * 0.25, 0, 0, R * 1.06);
  halo.addColorStop(0, withAlpha(p.base, 0.14 * p.glow));
  halo.addColorStop(0.6, withAlpha(p.base, 0.05 * p.glow));
  halo.addColorStop(1, withAlpha(p.base, 0));
  ctx.fillStyle = halo;
  ctx.beginPath();
  ctx.arc(0, 0, R * 1.06, 0, TAU);
  ctx.fill();

  // 1. внешние деления
  ctx.save();
  ctx.rotate(t * 0.05 * s);
  ctx.beginPath();
  for (let i = 0; i < 120; i++) {
    const a = (i / 120) * TAU;
    const len = i % 10 === 0 ? 0.065 : i % 5 === 0 ? 0.04 : 0.022;
    const c = Math.cos(a);
    const si = Math.sin(a);
    ctx.moveTo(c * R * 0.985, si * R * 0.985);
    ctx.lineTo(c * R * (0.985 - len), si * R * (0.985 - len));
  }
  ctx.strokeStyle = withAlpha(p.base, 0.45 + 0.3 * p.glow);
  ctx.lineWidth = Math.max(1, R * 0.006);
  ctx.stroke();
  ctx.restore();

  // 2. сегментированное кольцо
  ctx.save();
  ctx.rotate(-t * 0.13 * s);
  for (let i = 0; i < 8; i++) {
    const a0 = (i / 8) * TAU;
    arcPath(ctx, R * 0.87, a0, a0 + (TAU / 8) * 0.78);
    glowStroke(ctx, i === 0 || i === 4 ? p.accent : p.base, R * 0.022, p.glow);
  }
  ctx.restore();

  // 3. тонкое кольцо и пунктир
  ctx.beginPath();
  ctx.arc(0, 0, R * 0.815, 0, TAU);
  ctx.strokeStyle = withAlpha(p.base, 0.25);
  ctx.lineWidth = Math.max(1, R * 0.004);
  ctx.stroke();
  ctx.save();
  ctx.rotate(t * 0.21 * s);
  ctx.setLineDash([R * 0.012, R * 0.03]);
  arcPath(ctx, R * 0.785, 0, TAU);
  ctx.strokeStyle = withAlpha(p.base, 0.55 * p.glow + 0.2);
  ctx.lineWidth = Math.max(1, R * 0.012);
  ctx.stroke();
  ctx.setLineDash([]);
  ctx.restore();

  // 4. радиальная волна от уровня звука
  if (p.wave > 0.01) {
    const n = 128;
    ctx.beginPath();
    for (let i = 0; i < n; i++) {
      const a = (i / n) * TAU + t * 0.08;
      const noise = 0.5 + 0.5 * Math.sin(i * 0.71 + t * 9.1) * Math.sin(i * 0.23 - t * 5.3);
      const amp = R * 0.14 * p.wave * (0.3 + 0.7 * noise);
      const c = Math.cos(a);
      const si = Math.sin(a);
      ctx.moveTo(c * R * 0.695, si * R * 0.695);
      ctx.lineTo(c * (R * 0.7 + amp), si * (R * 0.7 + amp));
    }
    ctx.strokeStyle = withAlpha(p.base, 0.12 * p.glow);
    ctx.lineWidth = Math.max(2, R * 0.016);
    ctx.stroke();
    ctx.strokeStyle = withAlpha(p.accent === p.base ? p.base : PALETTE.ice, 0.35 + 0.6 * p.wave);
    ctx.lineWidth = Math.max(1, R * 0.006);
    ctx.stroke();
  }

  // 5. толстые дуги
  ctx.save();
  ctx.rotate(t * 0.32 * s);
  for (let i = 0; i < 3; i++) {
    const a0 = (i / 3) * TAU;
    arcPath(ctx, R * 0.61, a0, a0 + TAU * 0.2);
    glowStroke(ctx, i === 1 ? p.accent : p.base, R * 0.045, p.glow);
  }
  ctx.restore();
  ctx.save();
  ctx.rotate(-t * 0.5 * s);
  for (let i = 0; i < 6; i++) {
    const a0 = (i / 6) * TAU;
    arcPath(ctx, R * 0.56, a0, a0 + TAU * 0.06);
    ctx.strokeStyle = withAlpha(p.base, 0.7);
    ctx.lineWidth = Math.max(1, R * 0.01);
    ctx.stroke();
  }
  ctx.restore();

  // 6. сканирующая дуга (думает)
  if (p.scan) {
    const head = t * 3.3;
    const steps = 16;
    for (let i = 0; i < steps; i++) {
      const a1 = head - (i / steps) * 1.3;
      arcPath(ctx, R * 0.665, a1 - 1.3 / steps, a1);
      ctx.strokeStyle = withAlpha(PALETTE.ice, 0.9 * (1 - i / steps) ** 1.6);
      ctx.lineWidth = Math.max(1.5, R * 0.03);
      ctx.stroke();
    }
  }

  // 7. точки
  ctx.save();
  ctx.rotate(-t * 0.42 * s);
  ctx.fillStyle = withAlpha(p.base, 0.75);
  ctx.beginPath();
  const dot = Math.max(0.8, R * 0.007);
  for (let i = 0; i < 60; i++) {
    const a = (i / 60) * TAU;
    ctx.moveTo(Math.cos(a) * R * 0.5 + dot, Math.sin(a) * R * 0.5);
    ctx.arc(Math.cos(a) * R * 0.5, Math.sin(a) * R * 0.5, dot, 0, TAU);
  }
  ctx.fill();
  ctx.restore();

  // 8. внутреннее кольцо
  arcPath(ctx, R * 0.42, 0, TAU);
  glowStroke(ctx, p.base, R * 0.012, p.glow);

  // 9. ядро
  const coreR = R * 0.36 * (1 + 0.1 * p.pulse);
  const core = ctx.createRadialGradient(0, 0, 0, 0, 0, coreR);
  core.addColorStop(0, withAlpha('#FFFFFF', 0.85 * (0.6 + 0.4 * p.glow)));
  core.addColorStop(0.22, withAlpha(PALETTE.ice, 0.6 * p.glow + 0.2 * p.pulse));
  core.addColorStop(0.55, withAlpha(p.base, 0.28 * p.glow + 0.15 * p.pulse));
  core.addColorStop(1, withAlpha(p.base, 0));
  ctx.fillStyle = core;
  ctx.beginPath();
  ctx.arc(0, 0, coreR, 0, TAU);
  ctx.fill();

  // 10. треугольник
  const tr = R * 0.25 * (1 + 0.04 * p.pulse);
  ctx.beginPath();
  for (let i = 0; i < 3; i++) {
    const a = -Math.PI / 2 + (i / 3) * TAU;
    const x = Math.cos(a) * tr;
    const y = Math.sin(a) * tr;
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  }
  ctx.closePath();
  ctx.lineJoin = 'round';
  glowStroke(ctx, p.base === PALETTE.red ? PALETTE.red : PALETTE.ice, R * 0.022, p.glow);

  ctx.restore();
}

/** Гекс-сетка фона HUD (рисуется один раз при изменении размера). */
export function drawHexGrid(ctx: CanvasRenderingContext2D, w: number, h: number, size = 28) {
  ctx.clearRect(0, 0, w, h);
  const dx = size * Math.sqrt(3);
  const dy = size * 1.5;
  ctx.beginPath();
  for (let row = -1, y = 0; y < h + size * 2; row++, y = row * dy) {
    const off = row % 2 === 0 ? 0 : dx / 2;
    for (let x = -dx + off; x < w + dx; x += dx) {
      for (let k = 0; k < 6; k++) {
        const a = Math.PI / 6 + (k * Math.PI) / 3;
        const px = x + Math.cos(a) * size;
        const py = y + Math.sin(a) * size;
        if (k === 0) ctx.moveTo(px, py);
        else ctx.lineTo(px, py);
      }
      ctx.closePath();
    }
  }
  ctx.strokeStyle = withAlpha(PALETTE.steel, 0.16);
  ctx.lineWidth = 1;
  ctx.stroke();
  // виньетка: сетка гаснет к краям и под реактором
  const g = ctx.createRadialGradient(w / 2, h / 2, Math.min(w, h) * 0.18, w / 2, h / 2, Math.max(w, h) * 0.75);
  g.addColorStop(0, 'rgba(2, 8, 13, 0.85)');
  g.addColorStop(0.45, 'rgba(2, 8, 13, 0)');
  g.addColorStop(1, 'rgba(2, 8, 13, 0.9)');
  ctx.globalCompositeOperation = 'destination-out';
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, w, h);
  ctx.globalCompositeOperation = 'source-over';
}
