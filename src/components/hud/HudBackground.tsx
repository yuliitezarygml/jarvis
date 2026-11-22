'use client';

import { useEffect, useRef } from 'react';

import { drawHexGrid } from '@/components/reactor/draw';
import { onThemeChange } from '@/lib/theme';

/** Фон HUD: гекс-сетка (рисуется только при изменении размера) + линии сканирования на CSS. */
export default function HudBackground() {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    const ctx = canvas?.getContext('2d');
    if (!canvas || !ctx) return;
    const redraw = () => {
      const dpr = window.devicePixelRatio || 1;
      const w = canvas.clientWidth;
      const h = canvas.clientHeight;
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      drawHexGrid(ctx, w, h);
    };
    redraw();
    const ro = new ResizeObserver(redraw);
    ro.observe(canvas);
    const unsub = onThemeChange(redraw); // сетка в цвете темы
    return () => {
      ro.disconnect();
      unsub();
    };
  }, []);
  return (
    <div className="hud-bg" aria-hidden="true">
      <canvas ref={ref} className="hud-grid" />
      <div className="hud-scan" />
    </div>
  );
}
