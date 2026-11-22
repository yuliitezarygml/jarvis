'use client';

import { useRef } from 'react';

import ConfirmDialog from '@/components/hud/ConfirmDialog';
import Reactor from '@/components/reactor/Reactor';
import { call, startDragging } from '@/lib/bridge';
import { STATE_LABEL } from '@/lib/glow';
import { useJarvis } from '@/lib/store';

const DRAG_THRESHOLD = 4;

/** Виджет в углу: тот же реактор. Клик — во весь экран, перетаскивание — переместить (позиция запоминается). */
export default function CornerPage() {
  const assistant = useJarvis((s) => s.assistant);
  const confirm = useJarvis((s) => s.confirm);
  const down = useRef<{ x: number; y: number; dragging: boolean } | null>(null);

  const expand = () => void call('set_hud_mode', { mode: 'fullscreen' });

  return (
    <main className="corner" data-state={assistant}>
      <div
        className="corner-hit"
        role="button"
        tabIndex={0}
        aria-label={`Джарвис: ${STATE_LABEL[assistant]}. Развернуть во весь экран`}
        onPointerDown={(e) => {
          if (e.button !== 0) return;
          down.current = { x: e.clientX, y: e.clientY, dragging: false };
        }}
        onPointerMove={(e) => {
          const d = down.current;
          if (!d || d.dragging) return;
          if (Math.hypot(e.clientX - d.x, e.clientY - d.y) > DRAG_THRESHOLD) {
            d.dragging = true;
            void startDragging();
          }
        }}
        onPointerUp={() => {
          const d = down.current;
          down.current = null;
          if (d && !d.dragging) expand();
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') expand();
        }}
      >
        <Reactor variant="corner" className="reactor reactor-corner" />
        <span className="corner-state">{STATE_LABEL[assistant]}</span>
      </div>
      {confirm && (
        <div className="corner-confirm">
          <ConfirmDialog compact />
        </div>
      )}
    </main>
  );
}
