// Перевод координат виртуального рабочего стола (физические пиксели, как в `ControlTarget.rect`)
// в CSS-пиксели окна оверлея, которое занимает основной монитор.

import type { OverlayGeometry, Rect } from './types';

export interface CssBox {
  left: number;
  top: number;
  width: number;
  height: number;
}

export function toOverlayBox(rect: Rect, g: Pick<OverlayGeometry, 'x' | 'y' | 'scale'>): CssBox {
  const s = g.scale > 0 ? g.scale : 1;
  return {
    left: (rect.x - g.x) / s,
    top: (rect.y - g.y) / s,
    width: rect.w / s,
    height: rect.h / s,
  };
}

export function toOverlayPoint(x: number, y: number, g: Pick<OverlayGeometry, 'x' | 'y' | 'scale'>) {
  const s = g.scale > 0 ? g.scale : 1;
  return { x: (x - g.x) / s, y: (y - g.y) / s };
}

/** Пересекается ли прямоугольник с монитором оверлея (иначе цель на другом мониторе). */
export function intersectsOverlay(rect: Rect, g: OverlayGeometry): boolean {
  return rect.x < g.x + g.width && rect.x + rect.w > g.x && rect.y < g.y + g.height && rect.y + rect.h > g.y;
}

/** Где поставить подпись цели: над рамкой, а если не помещается — под ней. */
export function labelPlacement(box: CssBox, viewportHeight: number, labelHeight = 30): 'above' | 'below' {
  if (box.top - labelHeight - 8 >= 0) return 'above';
  return box.top + box.height + labelHeight + 8 <= viewportHeight ? 'below' : 'above';
}
