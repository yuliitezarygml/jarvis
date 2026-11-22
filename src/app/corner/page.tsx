'use client';

import { useEffect, useRef, useState } from 'react';

import ConfirmDialog from '@/components/hud/ConfirmDialog';
import Reactor from '@/components/reactor/Reactor';
import RichText from '@/components/RichText';
import { call, startDragging } from '@/lib/bridge';
import { STATE_LABEL } from '@/lib/glow';
import type { ActionItem, SubtitleLine } from '@/lib/reducer';
import { useJarvis, useNow } from '@/lib/store';

const DRAG_THRESHOLD = 4;
/** Сколько панель подписей висит после последней активности. */
const CAPTION_HOLD_MS = 9000;

type Side = 'left' | 'right' | 'none';

function lastOf(lines: SubtitleLine[], speaker: 'user' | 'jarvis', current: SubtitleLine | null): SubtitleLine | null {
  if (current && current.speaker === speaker) return current;
  for (let i = lines.length - 1; i >= 0; i--) if (lines[i].speaker === speaker) return lines[i];
  return null;
}

function actionIcon(a: ActionItem): string {
  switch (a.status) {
    case 'running':
      return '⟳';
    case 'done':
      return '✓';
    case 'failed':
      return '✕';
    case 'cancelled':
      return '–';
    case 'awaiting_confirm':
      return '?';
  }
}

/** Виджет в углу: реактор + панель подписей (что вы сказали, что отвечает Джарвис, что он делает).
 *  Клик по реактору — во весь экран, перетаскивание — переместить (позиция запоминается). */
export default function CornerPage() {
  const assistant = useJarvis((s) => s.assistant);
  const stateSince = useJarvis((s) => s.stateSince);
  const confirm = useJarvis((s) => s.confirm);
  const subtitle = useJarvis((s) => s.subtitle);
  const history = useJarvis((s) => s.subtitleHistory);
  const actions = useJarvis((s) => s.actions);
  const lastError = useJarvis((s) => s.lastError);
  const now = useNow(500);
  const down = useRef<{ x: number; y: number; dragging: boolean } | null>(null);
  const [side, setSide] = useState<Side>('none');

  const expand = () => void call('set_hud_mode', { mode: 'fullscreen' });

  const user = lastOf(history, 'user', subtitle);
  const jarvis = lastOf(history, 'jarvis', subtitle);
  const action = actions.length > 0 ? actions[actions.length - 1] : null;
  const error = lastError && now - lastError.at < CAPTION_HOLD_MS ? lastError : null;

  const lastActivity = Math.max(
    subtitle?.at ?? 0,
    action?.updatedAt ?? 0,
    error?.at ?? 0,
    assistant !== 'idle' ? stateSince : 0,
  );
  const busy = assistant !== 'idle' || action?.status === 'running' || action?.status === 'awaiting_confirm';
  const wantOpen = busy || (lastActivity > 0 && now - lastActivity < CAPTION_HOLD_MS);

  useEffect(() => {
    let alive = true;
    call<string>('corner_caption', { open: wantOpen })
      .then((s) => alive && setSide((s as Side) ?? 'none'))
      .catch(() => alive && setSide(wantOpen ? 'left' : 'none'));
    return () => {
      alive = false;
    };
  }, [wantOpen]);

  // Показываем только свежее: старые реплики из прошлых разговоров не нужны.
  const fresh = (l: SubtitleLine | null) => (l && now - l.at < CAPTION_HOLD_MS * 3 ? l : null);
  const u = fresh(user);
  const j = fresh(jarvis);
  const act = action && (action.status === 'running' || action.status === 'awaiting_confirm' || now - action.updatedAt < CAPTION_HOLD_MS) ? action : null;

  return (
    <main className="corner" data-state={assistant} data-caption={side}>
      {side !== 'none' && (
        <section className="corner-caption" aria-live="polite">
          <div className="cc-state">
            <span className="state-dot" aria-hidden="true" />
            {STATE_LABEL[assistant]}
          </div>
          {u && (
            <p className="cc-line cc-user" data-final={u.final}>
              <span className="cc-who">Вы</span>
              {u.text}
            </p>
          )}
          {j && (
            <p className="cc-line cc-jarvis" data-final={j.final}>
              <span className="cc-who">Джарвис</span>
              <RichText text={j.text} />
            </p>
          )}
          {act && (
            <p className="cc-action" data-status={act.status}>
              <span className="cc-icon" aria-hidden="true">
                {actionIcon(act)}
              </span>
              {act.summary || act.tool}
            </p>
          )}
          {error && <p className="cc-error">{error.message}</p>}
          {!u && !j && !act && !error && <p className="cc-hint">Скажите «Джарвис, …»</p>}
        </section>
      )}
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
