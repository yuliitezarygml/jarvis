'use client';

import { useEffect, useState } from 'react';

import AppShell, { DemoBadge } from '@/components/AppShell';
import { call, errorText } from '@/lib/bridge';
import { FACT_CATEGORIES, categoryLabel, filterFacts, type FactFilter } from '@/lib/persona';
import type { Fact, Lesson, MemoryData, MemoryKind, Playbook } from '@/lib/types';

const KINDS: { id: MemoryKind; name: string; lead: string }[] = [
  { id: 'facts', name: 'Факты', lead: 'Что Джарвис знает о вас и вашем ПК: предпочтения, папки, привычки.' },
  { id: 'lessons', name: 'Уроки', lead: 'Память ошибок: что пошло не так и как это исправить в следующий раз.' },
  { id: 'playbooks', name: 'Плейбуки', lead: 'Удачные последовательности действий, которые Джарвис повторяет по похожей просьбе.' },
];

type Item = Fact | Lesson | Playbook;

function emptyItem(kind: MemoryKind): Item {
  switch (kind) {
    case 'facts':
      return { id: '', text: '', confidence: 1, source: 'вручную', updated: new Date().toISOString().slice(0, 10), category: 'general', important: false };
    case 'lessons':
      return { id: '', trigger: '', what_went_wrong: '', fix: '', tool: '', seen: 0, last_seen: new Date().toISOString().slice(0, 10) };
    case 'playbooks':
      return { id: '', name: '', when: '', steps: [], successes: 0 };
  }
}

function Editor({
  kind,
  item,
  onSave,
  onCancel,
  defaultCategory,
}: {
  kind: MemoryKind;
  item: Item;
  onSave: (i: Item) => void;
  onCancel: () => void;
  /** Категория нового факта (в фильтре «Характер» — style). */
  defaultCategory?: string;
}) {
  const [v, setV] = useState<Item>(() => (kind === 'facts' && defaultCategory && !(item as Fact).id ? { ...(item as Fact), category: defaultCategory } : item));
  const set = (patch: Partial<Item>) => setV((x) => ({ ...x, ...patch }) as Item);
  return (
    <div className="card">
      {kind === 'facts' && (
        <label className="field">
          <span className="field-label">Факт</span>
          <textarea className="textarea" value={(v as Fact).text} onChange={(e) => set({ text: e.target.value })} />
        </label>
      )}
      {kind === 'facts' && (
        <div className="row" style={{ marginBottom: 12 }}>
          <label className="field" style={{ marginBottom: 0 }}>
            <span className="field-label">Категория</span>
            <select className="select" value={(v as Fact).category || 'general'} onChange={(e) => set({ category: e.target.value })}>
              {FACT_CATEGORIES.map((c) => (
                <option key={c} value={c}>
                  {categoryLabel(c)}
                  {c === 'style' ? ' (как вы общаетесь)' : ''}
                </option>
              ))}
              {(v as Fact).category && !(FACT_CATEGORIES as readonly string[]).includes((v as Fact).category) && (
                <option value={(v as Fact).category}>{(v as Fact).category}</option>
              )}
            </select>
          </label>
          <label className="toggle" style={{ borderBottom: 'none' }}>
            <input type="checkbox" checked={Boolean((v as Fact).important)} onChange={(e) => set({ important: e.target.checked })} />
            <span className="toggle-text">
              <span>Учитывать всегда</span>
            </span>
          </label>
        </div>
      )}
      {kind === 'lessons' && (
        <>
          <label className="field">
            <span className="field-label">Когда срабатывает (текст ошибки или ситуация)</span>
            <input className="input" value={(v as Lesson).trigger} onChange={(e) => set({ trigger: e.target.value })} />
          </label>
          <label className="field">
            <span className="field-label">Что пошло не так</span>
            <input className="input" value={(v as Lesson).what_went_wrong} onChange={(e) => set({ what_went_wrong: e.target.value })} />
          </label>
          <label className="field">
            <span className="field-label">Как исправить</span>
            <input className="input" value={(v as Lesson).fix} onChange={(e) => set({ fix: e.target.value })} />
          </label>
          <label className="field">
            <span className="field-label">Инструмент или приложение</span>
            <input className="input" value={(v as Lesson).tool} onChange={(e) => set({ tool: e.target.value })} />
          </label>
        </>
      )}
      {kind === 'playbooks' && (
        <>
          <label className="field">
            <span className="field-label">Название</span>
            <input className="input" value={(v as Playbook).name} onChange={(e) => set({ name: e.target.value })} />
          </label>
          <label className="field">
            <span className="field-label">Когда применять</span>
            <input className="input" value={(v as Playbook).when} onChange={(e) => set({ when: e.target.value })} />
          </label>
          <label className="field">
            <span className="field-label">Шаги, по одному в строке</span>
            <textarea
              className="textarea mono"
              value={(v as Playbook).steps.join('\n')}
              onChange={(e) => set({ steps: e.target.value.split('\n') })}
            />
          </label>
        </>
      )}
      <div className="row">
        <button type="button" className="btn btn-primary" onClick={() => onSave(kind === 'playbooks' ? { ...(v as Playbook), steps: (v as Playbook).steps.map((s) => s.trim()).filter(Boolean) } : v)}>
          Сохранить
        </button>
        <button type="button" className="btn btn-ghost" onClick={onCancel}>
          Отмена
        </button>
      </div>
    </div>
  );
}

function Summary({ kind, item }: { kind: MemoryKind; item: Item }) {
  if (kind === 'facts') {
    const f = item as Fact;
    return (
      <>
        <p className="mem-main">
          <span className="mem-cat" data-cat={f.category || 'general'}>
            {categoryLabel(f.category)}
          </span>
          {f.important && <span className="mem-cat" data-cat="important">всегда</span>}
          {f.text}
        </p>
        <p className="mem-meta">
          уверенность {Math.round(f.confidence * 100)} % · источник: {f.source} · {f.updated}
        </p>
      </>
    );
  }
  if (kind === 'lessons') {
    const l = item as Lesson;
    return (
      <>
        <p className="mem-main">
          <code>{l.trigger}</code>
        </p>
        <p className="mem-main">
          {l.what_went_wrong} → <span className="ok-text">{l.fix}</span>
        </p>
        <p className="mem-meta">
          {l.tool && <>инструмент: {l.tool} · </>}встречалось: {l.seen} · последний раз: {l.last_seen}
        </p>
      </>
    );
  }
  const p = item as Playbook;
  return (
    <>
      <p className="mem-main">{p.name}</p>
      <p className="mem-meta">когда: {p.when}</p>
      <ol className="small">
        {p.steps.map((s, i) => (
          <li key={i} className="mono">
            {s}
          </li>
        ))}
      </ol>
      <p className="mem-meta">успешных запусков: {p.successes}</p>
    </>
  );
}

export default function MemoryPage() {
  const [kind, setKind] = useState<MemoryKind>('facts');
  const [data, setData] = useState<MemoryData | null>(null);
  const [editing, setEditing] = useState<string | null>(null); // id или 'new'
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [factFilter, setFactFilter] = useState<FactFilter>('all');

  useEffect(() => {
    call<MemoryData>('memory_list')
      .then(setData)
      .catch((e) => setError(errorText(e)));
  }, []);

  const save = async (item: Item) => {
    try {
      setData(await call<MemoryData>('memory_upsert', { kind, item }));
      setEditing(null);
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  };
  const remove = async (id: string) => {
    try {
      setData(await call<MemoryData>('memory_delete', { kind, id }));
      setConfirmDelete(null);
    } catch (e) {
      setError(errorText(e));
    }
  };

  const items: Item[] = data ? (kind === 'facts' ? filterFacts(data.facts, factFilter) : data[kind]) : [];
  const styleCount = data ? filterFacts(data.facts, 'style').length : 0;
  const meta = KINDS.find((k) => k.id === kind)!;

  return (
    <AppShell>
      <div className="page-head">
        <h1 className="page-title">Память</h1>
        <DemoBadge show={data?.demo} />
      </div>
      <div className="tabs" role="tablist">
        {KINDS.map((k) => (
          <button
            key={k.id}
            type="button"
            role="tab"
            className="tab"
            aria-selected={kind === k.id}
            onClick={() => {
              setKind(k.id);
              setEditing(null);
              setConfirmDelete(null);
            }}
          >
            {k.name}
            {data && <span className="muted"> {data[k.id].length}</span>}
          </button>
        ))}
      </div>
      <p className="section-lead">{meta.lead}</p>
      {kind === 'facts' && (
        <div className="mem-filters" role="group" aria-label="Фильтр фактов">
          <button type="button" className="chip" aria-pressed={factFilter === 'all'} onClick={() => setFactFilter('all')}>
            Все
          </button>
          <button
            type="button"
            className="chip"
            aria-pressed={factFilter === 'style'}
            title="Как вы общаетесь: мат, юмор, что злит. Джарвис всегда учитывает это в разговоре."
            onClick={() => setFactFilter('style')}
          >
            Характер{data && <span className="muted"> {styleCount}</span>}
          </button>
        </div>
      )}
      {kind === 'facts' && factFilter === 'style' && (
        <p className="muted small">Это Джарвис понял о вашей манере общения. Неверное можно исправить или удалить.</p>
      )}
      {error && <p className="status" data-ok="false">{error}</p>}
      <div className="row" style={{ marginBottom: 12 }}>
        <button type="button" className="btn" onClick={() => setEditing('new')} disabled={editing === 'new'}>
          Добавить
        </button>
      </div>
      {editing === 'new' && (
        <Editor
          kind={kind}
          item={emptyItem(kind)}
          defaultCategory={kind === 'facts' && factFilter === 'style' ? 'style' : undefined}
          onSave={(i) => void save(i)}
          onCancel={() => setEditing(null)}
        />
      )}
      {data && items.length === 0 && editing !== 'new' && <p className="muted">Пусто. Джарвис заполнит раздел сам по ходу работы, или добавьте запись вручную.</p>}
      <ul className="list-plain">
        {items.map((it) => (
          <li key={it.id} className="mem-item">
            {editing === it.id ? (
              <Editor kind={kind} item={it} onSave={(i) => void save(i)} onCancel={() => setEditing(null)} />
            ) : (
              <>
                <Summary kind={kind} item={it} />
                <div className="mem-actions">
                  <button type="button" className="btn btn-small" onClick={() => setEditing(it.id)}>
                    Изменить
                  </button>
                  {confirmDelete === it.id ? (
                    <>
                      <button type="button" className="btn btn-small btn-danger" onClick={() => void remove(it.id)}>
                        Удалить навсегда
                      </button>
                      <button type="button" className="btn btn-small btn-ghost" onClick={() => setConfirmDelete(null)}>
                        Оставить
                      </button>
                    </>
                  ) : (
                    <button type="button" className="btn btn-small btn-ghost" onClick={() => setConfirmDelete(it.id)}>
                      Удалить
                    </button>
                  )}
                </div>
              </>
            )}
          </li>
        ))}
      </ul>
    </AppShell>
  );
}
