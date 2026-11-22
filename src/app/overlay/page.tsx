'use client';

import { useEffect, useRef, useState } from 'react';

import DevPanel from '@/components/DevPanel';
import { call, inTauri, listenTo } from '@/lib/bridge';
import { intersectsOverlay, labelPlacement, toOverlayBox, toOverlayPoint } from '@/lib/geometry';
import { glowFor, withAlpha } from '@/lib/glow';
import { useJarvis } from '@/lib/store';
import { CURSOR_EVENT, type OverlayGeometry } from '@/lib/types';

const TRAIL_MS = 700;

function useGeometry(): OverlayGeometry | null {
  const [g, setG] = useState<OverlayGeometry | null>(null);
  useEffect(() => {
    const load = () =>
      void call<OverlayGeometry | null>('overlay_geometry')
        .then((v) => setG(v ?? { x: 0, y: 0, width: window.innerWidth, height: window.innerHeight, scale: window.devicePixelRatio || 1 }))
        .catch(() => setG({ x: 0, y: 0, width: window.innerWidth, height: window.innerHeight, scale: window.devicePixelRatio || 1 }));
    load();
    window.addEventListener('resize', load);
    return () => window.removeEventListener('resize', load);
  }, []);
  return g;
}

/** След курсора: точки приходят из Rust (`jarvis://cursor`) только во время управления. */
function CursorTrail({ geometry, color }: { geometry: OverlayGeometry; color: string }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    const ctx = canvas?.getContext('2d');
    if (!canvas || !ctx) return;
    const pts: { x: number; y: number; t: number }[] = [];
    let raf = 0;
    const draw = () => {
      raf = 0;
      const dpr = window.devicePixelRatio || 1;
      const w = canvas.clientWidth;
      const h = canvas.clientHeight;
      if (canvas.width !== Math.round(w * dpr)) {
        canvas.width = Math.round(w * dpr);
        canvas.height = Math.round(h * dpr);
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);
      const now = performance.now();
      while (pts.length && now - pts[0].t > TRAIL_MS) pts.shift();
      ctx.lineCap = 'round';
      ctx.lineJoin = 'round';
      for (let i = 1; i < pts.length; i++) {
        const age = (now - pts[i].t) / TRAIL_MS;
        ctx.strokeStyle = withAlpha(color, 0.85 * (1 - age));
        ctx.lineWidth = 2 + 6 * (1 - age);
        ctx.beginPath();
        ctx.moveTo(pts[i - 1].x, pts[i - 1].y);
        ctx.lineTo(pts[i].x, pts[i].y);
        ctx.stroke();
      }
      const head = pts[pts.length - 1];
      if (head) {
        ctx.fillStyle = withAlpha(color, 0.35);
        ctx.beginPath();
        ctx.arc(head.x, head.y, 14, 0, Math.PI * 2);
        ctx.fill();
      }
      if (pts.length) raf = requestAnimationFrame(draw);
    };
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listenTo<{ x: number; y: number }>(CURSOR_EVENT, (p) => {
      const q = toOverlayPoint(p.x, p.y, geometry);
      pts.push({ ...q, t: performance.now() });
      if (!raf) raf = requestAnimationFrame(draw);
    }).then((u) => {
      if (disposed) u();
      else unlisten = u;
    });
    return () => {
      disposed = true;
      unlisten?.();
      if (raf) cancelAnimationFrame(raf);
    };
  }, [geometry, color]);
  return <canvas ref={ref} className="overlay-trail" aria-hidden="true" />;
}

/**
 * Оверлей «Джарвис управляет ПК»: пульсирующая рамка по краю экрана (циан T1/T2, янтарь T3,
 * красный — ждёт подтверждения/ошибка), подсветка цели с подписью, след курсора и бейдж со стоп-хоткеем.
 * Окно прозрачное и пропускает клики; пока управления нет — ничего не рисует.
 */
export default function OverlayPage() {
  const control = useJarvis((s) => s.control);
  const confirm = useJarvis((s) => s.confirm);
  const assistant = useJarvis((s) => s.assistant);
  const geometry = useGeometry();
  const [browser, setBrowser] = useState(false);

  useEffect(() => {
    setBrowser(!inTauri());
  }, []);

  const glow = glowFor({ tier: control.active ? control.tier : null, confirmPending: confirm !== null, assistant });
  const target = control.target;
  const box = target && geometry && intersectsOverlay(target.rect, geometry) ? toOverlayBox(target.rect, geometry) : null;
  const placement = box ? labelPlacement(box, window.innerHeight) : 'above';

  return (
    <main className="overlay" aria-hidden={!glow}>
      {glow && (
        <>
          <div
            className="overlay-frame"
            data-kind={glow.kind}
            style={{ ['--glow' as string]: glow.color, ['--pulse' as string]: `${glow.pulseMs}ms` }}
          />
          <div className="overlay-badge" data-kind={glow.kind} style={{ ['--glow' as string]: glow.color }}>
            <span className="badge-dot" aria-hidden="true" />
            ДЖАРВИС УПРАВЛЯЕТ ПК · Ctrl+Alt+K — стоп
          </div>
        </>
      )}
      {glow && box && target && (
        <div
          key={target.at}
          className="overlay-target"
          data-label={placement}
          style={{
            left: box.left,
            top: box.top,
            width: box.width,
            height: box.height,
            ['--glow' as string]: glow.color,
          }}
        >
          <span className="target-label">{target.label}</span>
        </div>
      )}
      {glow && geometry && <CursorTrail geometry={geometry} color={glow.color} />}
      {browser && <DevPanel defaultOpen />}
    </main>
  );
}
