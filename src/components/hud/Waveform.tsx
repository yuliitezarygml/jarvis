'use client';

import { useEffect, useRef } from 'react';

import { withAlpha } from '@/lib/glow';
import { livePalette } from '@/lib/theme';
import { jarvisStore } from '@/lib/store';

const BARS = 48;

/** Полоса уровня звука под субтитрами (микрофон — циан, голос Джарвиса — светлый). Рисует только при звуке. */
export default function Waveform() {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = ref.current;
    const ctx = canvas?.getContext('2d');
    if (!canvas || !ctx) return;
    const levels = new Array<number>(BARS).fill(0);
    let raf = 0;
    let lastPush = 0;
    let idleFrames = 0;

    const draw = (now: number) => {
      raf = 0;
      if (document.hidden) return;
      const s = jarvisStore.getState();
      const fresh = Date.now() - s.audio.at < 300;
      if (now - lastPush > 33) {
        lastPush = now;
        levels.shift();
        levels.push(fresh ? (s.audio.lastSource === 'tts' ? s.audio.tts : s.audio.mic) : 0);
      }
      const dpr = window.devicePixelRatio || 1;
      const w = canvas.clientWidth;
      const h = canvas.clientHeight;
      if (canvas.width !== Math.round(w * dpr)) {
        canvas.width = Math.round(w * dpr);
        canvas.height = Math.round(h * dpr);
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);
      const pal = livePalette();
      const color = s.audio.lastSource === 'tts' ? pal.ice : pal.accent;
      const bw = w / BARS;
      for (let i = 0; i < BARS; i++) {
        const v = levels[i];
        const bh = Math.max(1, v * h);
        ctx.fillStyle = withAlpha(color, 0.25 + 0.75 * (i / BARS));
        ctx.fillRect(i * bw + 1, (h - bh) / 2, Math.max(1, bw - 2), bh);
      }
      idleFrames = levels.some((v) => v > 0.01) ? 0 : idleFrames + 1;
      if (idleFrames < 2) raf = requestAnimationFrame(draw);
    };

    const kick = () => {
      if (!raf) raf = requestAnimationFrame(draw);
    };
    const unsub = jarvisStore.subscribe(() => {
      if (Date.now() - jarvisStore.getState().audio.at < 300) kick();
    });
    const onVis = () => !document.hidden && kick();
    document.addEventListener('visibilitychange', onVis);
    kick();
    return () => {
      unsub();
      document.removeEventListener('visibilitychange', onVis);
      if (raf) cancelAnimationFrame(raf);
    };
  }, []);

  return <canvas ref={ref} className="waveform" aria-hidden="true" />;
}
