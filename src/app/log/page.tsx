'use client';

import { useEffect, useState } from 'react';

import AppShell, { DemoBadge } from '@/components/AppShell';
import { ActionRow } from '@/components/hud/ActionFeed';
import RichText from '@/components/RichText';
import { call, errorText } from '@/lib/bridge';
import { newestFirst } from '@/lib/reducer';
import { useJarvis } from '@/lib/store';
import type { HistoryResult } from '@/lib/types';

function time(iso: string | number): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? String(iso) : d.toLocaleString('ru-RU', { dateStyle: 'short', timeStyle: 'medium' });
}

export default function LogPage() {
  const [history, setHistory] = useState<HistoryResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const live = useJarvis((s) => s.subtitleHistory);
  const actions = useJarvis((s) => s.actions);
  const errors = useJarvis((s) => s.errors);

  useEffect(() => {
    call<HistoryResult>('history_list')
      .then(setHistory)
      .catch((e) => setError(errorText(e)));
  }, []);

  return (
    <AppShell>
      <div className="page-head">
        <h1 className="page-title">Журнал</h1>
        <DemoBadge show={history?.demo} />
      </div>
      {error && <p className="status" data-ok="false">{error}</p>}
      <div className="log-grid">
        <section>
          <h2 className="section-title">Диалоги</h2>
          <p className="section-lead small">Сохранённая история и реплики этой сессии.</p>
          <ul className="list-plain">
            {history?.items.map((m) => (
              <li key={m.id} className="msg" data-speaker={m.speaker}>
                <div className="msg-who">
                  {m.speaker === 'user' ? 'Вы' : 'Джарвис'} · {time(m.at)}
                </div>
                <div className="msg-text"><RichText text={m.text} /></div>
              </li>
            ))}
            {live.map((m) => (
              <li key={`live-${m.at}-${m.text}`} className="msg" data-speaker={m.speaker}>
                <div className="msg-who">
                  {m.speaker === 'user' ? 'Вы' : 'Джарвис'} · {time(m.at)} · сейчас
                </div>
                <div className="msg-text"><RichText text={m.text} /></div>
              </li>
            ))}
          </ul>
          {history && history.items.length === 0 && live.length === 0 && <p className="muted">Пока пусто.</p>}
        </section>
        <section>
          <h2 className="section-title">Действия</h2>
          <p className="section-lead small">Что Джарвис делал на ПК в этой сессии — новые сверху.</p>
          {actions.length === 0 ? (
            <p className="muted">Действий ещё не было.</p>
          ) : (
            <ol className="actions">
              {newestFirst(actions).map((a) => (
                <ActionRow key={a.id} a={a} showTime />
              ))}
            </ol>
          )}
          {errors.length > 0 && (
            <>
              <h2 className="section-title" style={{ marginTop: 24 }}>
                Ошибки
              </h2>
              <ul className="list-plain">
                {errors
                  .slice()
                  .reverse()
                  .map((e) => (
                    <li key={`${e.at}-${e.message}`} className="msg">
                      <div className="msg-who">{time(e.at)}</div>
                      <div className="error-text">{e.message}</div>
                    </li>
                  ))}
              </ul>
            </>
          )}
        </section>
      </div>
    </AppShell>
  );
}
