'use client';

import { useEffect, useRef } from 'react';

import { jarvisStore } from '@/lib/store';
import { liveMotion } from '@/lib/theme';

import { drawReactor, reactorParams, smoothLevel, targetFps } from './draw';

interface Props {
  variant: 'hud' | 'corner';
  className?: string;
}

/**
 * Канвас реактора. Читает состояние из стора в каждом кадре (без перерисовок React),
 * ограничивает FPS (HUD — 60, виджет — 15/30), останавливается при скрытом окне, учитывает devicePixelRatio.
 */
export default function Reactor({ variant, className }: Props) {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    let w = 0;
    let h = 0;
    const resize = () => {
      const rect = canvas.getBoundingClientRect();
      const dpr = Math.min(window.devicePixelRatio || 1, variant === 'corner' ? 2 : 3);
      w = rect.width;
      h = rect.height;
      canvas.width = Math.max(1, Math.round(w * dpr));
      canvas.height = Math.max(1, Math.round(h * dpr));
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);

    let raf = 0;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let last = performance.now();
    let level = 0;
    let curFps = 0;
    let clock = 0;

    const frame = (now: number) => {
      raf = 0;
      const s = jarvisStore.getState();
      const fps = targetFps(variant, s.assistant, document.hidden);
      curFps = fps;
      if (fps === 0) return; // возобновит visibilitychange
      const dt = Math.min(0.1, (now - last) / 1000);
      last = now;
      const fresh = Date.now() - s.audio.at < 350;
      const target = !fresh
        ? 0
        : s.assistant === 'listening'
          ? s.audio.mic
          : s.assistant === 'speaking'
            ? s.audio.tts
            : Math.max(s.audio.mic, s.audio.tts);
      level = smoothLevel(level, target, dt);
      // скорость анимации и яркость свечения из «Внешнего вида»: время копится с множителем — без рывков при смене
      const motion = liveMotion();
      clock += dt * motion.speed;
      const p = reactorParams(s.assistant, clock, level, Math.random());
      p.glow = Math.min(1.6, p.glow * motion.glow);
      drawReactor(ctx, w, h, clock, p);
      schedule(fps);
    };

    const schedule = (fps: number) => {
      if (fps >= 60) {
        raf = requestAnimationFrame(frame);
      } else {
        timer = setTimeout(() => {
          timer = null;
          raf = requestAnimationFrame(frame);
        }, 1000 / fps);
      }
    };

    const kick = () => {
      if (raf || timer) return;
      last = performance.now();
      raf = requestAnimationFrame(frame);
    };
    const onVisibility = () => {
      if (!document.hidden) kick();
    };
    document.addEventListener('visibilitychange', onVisibility);
    // смена состояния может поднять FPS (виджет: 15 → 30) — перезапускаем цикл без ожидания таймера
    const unsub = jarvisStore.subscribe(() => {
      if (timer && targetFps(variant, jarvisStore.getState().assistant, document.hidden) > curFps) {
        clearTimeout(timer);
        timer = null;
        kick();
      }
    });
    kick();

    return () => {
      ro.disconnect();
      document.removeEventListener('visibilitychange', onVisibility);
      unsub();
      if (raf) cancelAnimationFrame(raf);
      if (timer) clearTimeout(timer);
    };
  }, [variant]);

  return <canvas ref={ref} className={className} aria-hidden="true" />;
}
