'use client';

import { useEffect, useMemo, useState } from 'react';

import { call, errorText } from '@/lib/bridge';
import { fmtCtx, fmtLatency, fmtNumber } from '@/lib/format';
import { ROLE_LABEL, ROLE_NAMES, addProvider, addToChain, applyRanking, moveItem, removeFromChain, removeProvider } from '@/lib/settings';
import type { ProviderCheck, ProviderPreset, RoleName } from '@/lib/types';

import { Field, KeyField, type TabProps } from './common';

const STATUS_TEXT: Record<ProviderCheck['status'], string> = {
  ok: 'Ключ рабочий',
  bad_key: 'Ключ не принят',
  network: 'Провайдер недоступен',
};

function Mark({ v }: { v: boolean | null }) {
  if (v === null) return <span className="no">—</span>;
  return v ? <span className="yes">✓</span> : <span className="no">✕</span>;
}

function ModelTable({ providerId, check, onAdd }: { providerId: string; check: ProviderCheck; onAdd: (role: RoleName, model: string) => void }) {
  if (check.models.length === 0) return null;
  return (
    <div className="table-wrap">
      <table className="table">
        <thead>
          <tr>
            <th>Модель</th>
            <th>Контекст</th>
            <th>Инструменты</th>
            <th>Зрение</th>
            <th>Первый токен</th>
            <th>Скорость</th>
            <th>Рекомендуем</th>
            <th>В роль</th>
          </tr>
        </thead>
        <tbody>
          {check.models.map((m) => (
            <tr key={m.id}>
              <td className="mono">{m.id}</td>
              <td>{fmtCtx(m.ctx)}</td>
              <td>
                <Mark v={m.tools_ok ?? m.tools} />
              </td>
              <td>
                <Mark v={m.vision_ok ?? (m.vision ? null : false)} />
              </td>
              <td>{fmtLatency(m.latency_ms)}</td>
              <td>{m.tokens_per_s ? `${fmtNumber(m.tokens_per_s, 0)} ток/с` : '—'}</td>
              <td>
                {m.recommended_for.map((r) => (
                  <span key={r} className="rec">
                    {ROLE_LABEL[r].name}
                  </span>
                ))}
              </td>
              <td>
                <select
                  className="select"
                  value=""
                  aria-label={`Добавить ${m.id} в роль`}
                  onChange={(e) => e.target.value && onAdd(e.target.value as RoleName, `${providerId}/${m.id}`)}
                >
                  <option value="">добавить…</option>
                  {ROLE_NAMES.map((r) => (
                    <option key={r} value={r}>
                      {ROLE_LABEL[r].name}
                    </option>
                  ))}
                </select>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** Провайдеры LLM: ключи, проверка, таблица моделей с метками и назначение цепочек по ролям. */
export default function ProvidersTab({ draft, update }: TabProps) {
  const [presets, setPresets] = useState<ProviderPreset[]>([]);
  const [checks, setChecks] = useState<Record<string, ProviderCheck | 'loading' | { error: string }>>({});
  const [addId, setAddId] = useState('');
  const [addUrl, setAddUrl] = useState('');
  const [addKey, setAddKey] = useState('');
  const [chainInput, setChainInput] = useState<Record<RoleName, string>>({ brain: '', fast: '', vision: '', reflection: '' });

  useEffect(() => {
    void call<ProviderPreset[]>('provider_presets').then(setPresets).catch(() => setPresets([]));
  }, []);

  const preset = presets.find((p) => p.id === addId);
  const knownModels = useMemo(() => {
    const out: string[] = [];
    for (const [pid, c] of Object.entries(checks)) {
      if (c && typeof c === 'object' && 'models' in c) out.push(...c.models.map((m) => `${pid}/${m.id}`));
    }
    return out;
  }, [checks]);

  const check = async (id: string, baseUrl: string, apiKey: string) => {
    setChecks((c) => ({ ...c, [id]: 'loading' }));
    try {
      const res = await call<ProviderCheck>('provider_check', { providerId: id, baseUrl, apiKey });
      setChecks((c) => ({ ...c, [id]: res }));
    } catch (e) {
      setChecks((c) => ({ ...c, [id]: { error: errorText(e) } }));
    }
  };

  const allRanking = Object.values(checks).flatMap((c) => (c && typeof c === 'object' && 'ranking' in c ? c.ranking : []));

  return (
    <>
      <section className="section">
        <h2 className="section-title">Подключённые провайдеры</h2>
        <p className="section-lead">
          Ключи хранятся только в config.json на этом ПК и показываются замаскированными. «Проверить» запрашивает список моделей и
          коротко тестирует лучшие из них.
        </p>
        {draft.providers.length === 0 && <p className="muted">Пока ни одного — добавьте провайдера ниже.</p>}
        {draft.providers.map((p, i) => {
          const c = checks[p.id];
          return (
            <div className="card" key={`${p.id}-${i}`}>
              <div className="card-head">
                <span className="card-title">{p.id}</span>
                <span className="muted small">{p.kind === 'anthropic' ? 'Anthropic Messages' : 'OpenAI-совместимый'}</span>
                <label className="row small" style={{ marginLeft: 'auto' }}>
                  <input
                    type="checkbox"
                    checked={p.enabled}
                    onChange={(e) =>
                      update((cfg) => ({ ...cfg, providers: cfg.providers.map((x, j) => (j === i ? { ...x, enabled: e.target.checked } : x)) }))
                    }
                  />
                  включён
                </label>
              </div>
              <Field label="Адрес API">
                <input
                  className="input"
                  value={p.base_url}
                  spellCheck={false}
                  onChange={(e) => update((cfg) => ({ ...cfg, providers: cfg.providers.map((x, j) => (j === i ? { ...x, base_url: e.target.value } : x)) }))}
                />
              </Field>
              <KeyField
                key={p.api_key.includes('…') ? p.api_key : `${p.id}-key`}
                value={p.api_key}
                onChange={(v) => update((cfg) => ({ ...cfg, providers: cfg.providers.map((x, j) => (j === i ? { ...x, api_key: v } : x)) }))}
              />
              <div className="row">
                <button type="button" className="btn btn-primary" disabled={c === 'loading'} onClick={() => void check(p.id, p.base_url, p.api_key)}>
                  {c === 'loading' ? 'Проверяю…' : 'Проверить'}
                </button>
                {c && typeof c === 'object' && 'ranking' in c && c.ranking.length > 0 && (
                  <button type="button" className="btn" onClick={() => update((cfg) => ({ ...cfg, roles: applyRanking(cfg.roles, c.ranking) }))}>
                    Выбрать лучшее автоматически
                  </button>
                )}
                <button type="button" className="btn btn-ghost" onClick={() => update((cfg) => removeProvider(cfg, p.id))}>
                  Удалить провайдера
                </button>
              </div>
              {c && typeof c === 'object' && 'error' in c && <p className="status" data-ok="false">{c.error}</p>}
              {c && typeof c === 'object' && 'status' in c && (
                <>
                  <p className="status" data-ok={c.status === 'ok'}>
                    {STATUS_TEXT[c.status]}: {c.message}
                    {c.demo && <span className="muted"> · демо-данные</span>}
                  </p>
                  <ModelTable providerId={p.id} check={c} onAdd={(role, model) => update((cfg) => ({ ...cfg, roles: addToChain(cfg.roles, role, model) }))} />
                </>
              )}
            </div>
          );
        })}
      </section>

      <section className="section">
        <h2 className="section-title">Добавить провайдера</h2>
        <div className="card">
          <Field label="Провайдер">
            <select
              className="select"
              value={addId}
              onChange={(e) => {
                setAddId(e.target.value);
                setAddUrl(presets.find((p) => p.id === e.target.value)?.base_url ?? '');
              }}
            >
              <option value="">выберите из списка…</option>
              {presets.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                  {p.note ? ` — ${p.note}` : ''}
                </option>
              ))}
            </select>
          </Field>
          {preset && (
            <>
              <Field label="Адрес API">
                <input className="input" value={addUrl} spellCheck={false} onChange={(e) => setAddUrl(e.target.value)} placeholder="https://…/v1" />
              </Field>
              {preset.needs_key && (
                <Field label="Ключ API">
                  <input
                    className="input"
                    type="password"
                    autoComplete="off"
                    value={addKey}
                    onChange={(e) => setAddKey(e.target.value)}
                    placeholder="вставьте ключ"
                  />
                </Field>
              )}
              {preset.key_url && (
                <p className="muted small">
                  Где взять ключ: <span className="mono">{preset.key_url}</span>
                </p>
              )}
              <button
                type="button"
                className="btn btn-primary"
                disabled={!addUrl.trim() || (preset.needs_key && !addKey.trim())}
                onClick={() => {
                  const next = addProvider(draft, preset, addKey, addUrl);
                  const added = next.providers[next.providers.length - 1];
                  update(() => next);
                  setAddId('');
                  setAddKey('');
                  void check(added.id, added.base_url, added.api_key);
                }}
              >
                Добавить и проверить
              </button>
            </>
          )}
        </div>
      </section>

      <section className="section">
        <h2 className="section-title">Модели по ролям</h2>
        <p className="section-lead">
          Цепочка — порядок перехода при сбоях: если первая модель не ответила (лимит, ошибка сервера), Джарвис берёт следующую.
        </p>
        {allRanking.length > 0 && (
          <button type="button" className="btn" onClick={() => update((cfg) => ({ ...cfg, roles: applyRanking(cfg.roles, allRanking) }))}>
            Выбрать лучшее автоматически по всем проверкам
          </button>
        )}
        <datalist id="known-models">
          {knownModels.map((m) => (
            <option key={m} value={m} />
          ))}
        </datalist>
        {ROLE_NAMES.map((role) => (
          <div className="card" key={role}>
            <div className="card-head">
              <span className="card-title">{ROLE_LABEL[role].name}</span>
              <span className="muted small">{ROLE_LABEL[role].what}</span>
            </div>
            {draft.roles[role].length === 0 ? (
              <p className="muted small">Не задано{role === 'brain' ? ' — без этой роли Джарвис не сможет отвечать' : ''}.</p>
            ) : (
              <ol className="chain">
                {draft.roles[role].map((m, i) => (
                  <li key={m}>
                    <span className="chain-n">{i + 1}</span>
                    <span className="chain-model" title={m}>
                      {m}
                    </span>
                    <button
                      type="button"
                      className="btn btn-small btn-ghost"
                      aria-label="Выше"
                      disabled={i === 0}
                      onClick={() => update((cfg) => ({ ...cfg, roles: { ...cfg.roles, [role]: moveItem(cfg.roles[role], i, -1) } }))}
                    >
                      ↑
                    </button>
                    <button
                      type="button"
                      className="btn btn-small btn-ghost"
                      aria-label="Ниже"
                      disabled={i === draft.roles[role].length - 1}
                      onClick={() => update((cfg) => ({ ...cfg, roles: { ...cfg.roles, [role]: moveItem(cfg.roles[role], i, 1) } }))}
                    >
                      ↓
                    </button>
                    <button
                      type="button"
                      className="btn btn-small btn-ghost"
                      aria-label="Убрать"
                      onClick={() => update((cfg) => ({ ...cfg, roles: removeFromChain(cfg.roles, role, m) }))}
                    >
                      ✕
                    </button>
                  </li>
                ))}
              </ol>
            )}
            <div className="row">
              <input
                className="input grow"
                list="known-models"
                placeholder="провайдер/модель, например groq/openai/gpt-oss-120b"
                value={chainInput[role]}
                spellCheck={false}
                onChange={(e) => setChainInput((s) => ({ ...s, [role]: e.target.value }))}
              />
              <button
                type="button"
                className="btn"
                disabled={!chainInput[role].includes('/')}
                onClick={() => {
                  update((cfg) => ({ ...cfg, roles: addToChain(cfg.roles, role, chainInput[role].trim()) }));
                  setChainInput((s) => ({ ...s, [role]: '' }));
                }}
              >
                Добавить
              </button>
            </div>
          </div>
        ))}
      </section>
    </>
  );
}
