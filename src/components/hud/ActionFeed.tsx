'use client';

import { newestFirst, type ActionItem } from '@/lib/reducer';
import { useJarvis } from '@/lib/store';
import type { ActionStatus } from '@/lib/types';

export const STATUS_LABEL: Record<ActionStatus, string> = {
  running: 'выполняю',
  done: 'готово',
  failed: 'ошибка',
  cancelled: 'отменено',
  awaiting_confirm: 'жду подтверждения',
};

const GLYPH: Record<ActionStatus, string> = {
  running: '◌',
  done: '✓',
  failed: '✕',
  cancelled: '–',
  awaiting_confirm: '?',
};

export function ActionRow({ a, showTime }: { a: ActionItem; showTime?: boolean }) {
  return (
    <li className="action" data-status={a.status}>
      <span className="action-glyph" aria-hidden="true">
        {GLYPH[a.status]}
      </span>
      <span className="action-body">
        <span className="action-summary">{a.summary}</span>
        <span className="action-meta">
          <code>{a.tool}</code> · {STATUS_LABEL[a.status]}
          {showTime && <> · {new Date(a.updatedAt).toLocaleTimeString('ru-RU')}</>}
        </span>
      </span>
    </li>
  );
}

/** Живая лента действий: текущее сверху, затем предыдущие шаги. */
export default function ActionFeed({ limit = 12 }: { limit?: number }) {
  const actions = useJarvis((s) => s.actions);
  const list = newestFirst(actions).slice(0, limit);
  return (
    <section className="panel feed" aria-label="Лента действий">
      <h2 className="panel-title">Действия</h2>
      {list.length === 0 ? (
        <p className="muted small">Здесь появятся шаги, когда Джарвис начнёт работать на ПК.</p>
      ) : (
        <ol className="actions" aria-live="polite">
          {list.map((a) => (
            <ActionRow key={a.id} a={a} />
          ))}
        </ol>
      )}
    </section>
  );
}
