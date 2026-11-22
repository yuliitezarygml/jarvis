'use client';

import { useEffect, useMemo, useState } from 'react';

import { call, errorText } from '@/lib/bridge';
import { fmtCtx, fmtLatency, fmtNumber } from '@/lib/format';
import {
  ROLE_LABEL,
  ROLE_NAMES,
  addAnotherKey,
  addProvider,
  addToChain,
  applyRanking,
  blockingDisabled,
  chainItemState,
  moveItem,
  presetBadges,
  presetFor,
  providerTitle,
  removeFromChain,
  removeProvider,
  rolesUsing,
  setProviderEnabled,
  shortName,
} from '@/lib/settings';
import type { Config, ProviderCheck, ProviderConfig, ProviderPreset, RoleName, RoleRanking } from '@/lib/types';

import { ExternalLink, Field, KeyField, SecretInput, Switch, type TabProps } from './common';

const STATUS_TEXT: Record<ProviderCheck['status'], string> = {
  ok: 'Ключ рабочий',
  bad_key: 'Ключ не принят',
  network: 'Провайдер недоступен',
};

/** Сколько моделей показывать в таблице до «показать все». */
const MODELS_FOLDED = 8;

type CheckState = ProviderCheck | 'loading' | { error: string };

function Mark({ v }: { v: boolean | null }) {
  if (v === null) return <span className="no">—</span>;
  return v ? <span className="yes">✓</span> : <span className="no">✕</span>;
}

function Badges({ preset }: { preset: ProviderPreset }) {
  return (
    <span className="badges">
      {presetBadges(preset).map((b) => (
        <span key={b.text} className="badge" data-tone={b.tone}>
          {b.text}
        </span>
      ))}
    </span>
  );
}

function ModelTable({ providerId, check, onAdd }: { providerId: string; check: ProviderCheck; onAdd: (role: RoleName, model: string) => void }) {
  const [all, setAll] = useState(false);
  if (check.models.length === 0) return null;
  const rows = all ? check.models : check.models.slice(0, MODELS_FOLDED);
  return (
    <div className="models">
      <div className="table-wrap">
        <table className="table">
          <thead>
            <tr>
              <th>Модель</th>
              <th>Рекомендуем</th>
              <th>Контекст</th>
              <th title="Умеет вызывать инструменты (нужно «Мозгу»)">Инструменты</th>
              <th>Зрение</th>
              <th>Первый токен</th>
              <th>Скорость</th>
              <th>В роль</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((m) => (
              <tr key={m.id} data-rec={m.recommended_for.length > 0}>
                <td className="mono">{m.id}</td>
                <td>
                  {m.recommended_for.length === 0 ? (
                    <span className="no">—</span>
                  ) : (
                    m.recommended_for.map((r) => (
                      <span key={r} className="rec">
                        ★ {ROLE_LABEL[r].name}
                      </span>
                    ))
                  )}
                </td>
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
                  <select
                    className="select select-small"
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
      {check.models.length > MODELS_FOLDED && (
        <button type="button" className="btn btn-small btn-ghost" onClick={() => setAll((v) => !v)}>
          {all ? 'Свернуть' : `Показать все модели (${check.models.length})`}
        </button>
      )}
    </div>
  );
}

/** Карточка подключённого провайдера: выключатель, ключ, проверка, модели. */
function ProviderCard({
  p,
  preset,
  draft,
  check,
  update,
  onCheck,
  onBest,
}: {
  p: ProviderConfig;
  preset: ProviderPreset | undefined;
  draft: Config;
  check: CheckState | undefined;
  update: TabProps['update'];
  onCheck: () => void;
  onBest: (ranking: RoleRanking[]) => void;
}) {
  const [confirmRemove, setConfirmRemove] = useState(false);
  const edit = (patch: Partial<ProviderConfig>) =>
    update((cfg) => ({ ...cfg, providers: cfg.providers.map((x) => (x.id === p.id ? { ...x, ...patch } : x)) }));
  const used = rolesUsing(draft, p.id);
  const blocks = ROLE_NAMES.filter((r) => blockingDisabled(draft, r).includes(p.id));
  const title = providerTitle(p, preset, draft.providers);
  const result = check && typeof check === 'object' ? check : null;
  return (
    <article className="card prov-card" data-enabled={p.enabled}>
      <header className="prov-head">
        <div className="prov-title">
          <span className="card-title">{title}</span>
          {preset && title === shortName(preset.name) && p.id !== preset.id && <span className="mono muted small">{p.id}</span>}
          {preset?.tagline && <span className="prov-tagline">{preset.tagline}</span>}
        </div>
        <Switch checked={p.enabled} onChange={(v) => edit({ enabled: v })} label={`Провайдер ${title}`} />
      </header>

      {!p.enabled && (
        <p className="status" data-ok="false">
          Выключен — Джарвис пропускает его модели.
          {blocks.length > 0 && (
            <>
              {' '}
              <strong>{blocks.map((r) => `«${ROLE_LABEL[r].name}»`).join(', ')} останется без модели</strong>
              {blocks.includes('brain') && ' — Джарвис не сможет отвечать'}.
            </>
          )}
        </p>
      )}
      {p.enabled && used.length > 0 && (
        <p className="muted small prov-used">Используется в ролях: {used.map((r) => ROLE_LABEL[r].name).join(', ')}</p>
      )}

      <div className="prov-body">
        {(preset?.needs_key ?? true) || p.api_key ? (
          <KeyField
            key={p.api_key.includes('…') ? p.api_key : `${p.id}-key`}
            value={p.api_key}
            onChange={(v) => edit({ api_key: v })}
          />
        ) : (
          <p className="muted small">Ключ не нужен.</p>
        )}
        <details className="adv" open={!preset || !p.base_url}>
          <summary>
            Адрес API <span className="mono muted">{p.base_url || 'не задан'}</span>
          </summary>
          <Field label="Адрес API">
            <input className="input mono" value={p.base_url} spellCheck={false} onChange={(e) => edit({ base_url: e.target.value })} />
          </Field>
          <p className="muted small">{p.kind === 'anthropic' ? 'Протокол: Anthropic Messages' : 'Протокол: OpenAI-совместимый'}</p>
        </details>
      </div>

      <div className="row prov-actions">
        <button type="button" className="btn btn-primary" disabled={check === 'loading'} onClick={onCheck}>
          {check === 'loading' ? 'Проверяю…' : 'Проверить'}
        </button>
        {result && 'ranking' in result && result.ranking.some((r) => r.top.length > 0) && (
          <button type="button" className="btn" onClick={() => onBest(result.ranking)}>
            ★ Выбрать лучшее автоматически
          </button>
        )}
        {preset?.key_url && <ExternalLink href={preset.key_url}>Где взять ключ</ExternalLink>}
        {(preset?.needs_key ?? true) && (
          <button
            type="button"
            className="btn btn-small btn-ghost"
            title="Второй ключ того же провайдера: свои лимиты, Джарвис переключается между ключами"
            onClick={() => update((cfg) => addAnotherKey(cfg, p, preset))}
          >
            + Добавить ещё ключ
          </button>
        )}
        <span className="grow-spacer" />
        {confirmRemove ? (
          <>
            <span className="small muted">Удалить {title} и его модели из ролей?</span>
            <button type="button" className="btn btn-small btn-danger" onClick={() => update((cfg) => removeProvider(cfg, p.id))}>
              Удалить
            </button>
            <button type="button" className="btn btn-small btn-ghost" onClick={() => setConfirmRemove(false)}>
              Отмена
            </button>
          </>
        ) : (
          <button type="button" className="btn btn-small btn-ghost" onClick={() => setConfirmRemove(true)}>
            Удалить провайдера
          </button>
        )}
      </div>

      {result && 'error' in result && (
        <p className="status" data-ok="false">
          {result.error}
        </p>
      )}
      {result && 'status' in result && (
        <>
          <p className="status" data-ok={result.status === 'ok'}>
            {STATUS_TEXT[result.status]}: {result.message}
            {result.demo && <span className="muted"> · демо-данные</span>}
          </p>
          <ModelTable providerId={p.id} check={result} onAdd={(role, model) => update((cfg) => ({ ...cfg, roles: addToChain(cfg.roles, role, model) }))} />
        </>
      )}
    </article>
  );
}

/** Плитки «Добавить провайдера» и форма выбранного. */
function AddProvider({ presets, draft, onAdd }: { presets: ProviderPreset[]; draft: Config; onAdd: (p: ProviderPreset, key: string, url: string) => void }) {
  const [selId, setSelId] = useState('');
  const [url, setUrl] = useState('');
  const [key, setKey] = useState('');
  const sel = presets.find((p) => p.id === selId);
  const connected = (id: string) => draft.providers.some((p) => p.id === id);
  const pick = (p: ProviderPreset) => {
    setSelId(p.id === selId ? '' : p.id);
    setUrl(p.base_url);
    setKey('');
  };
  if (presets.length === 0) return <p className="muted">Список провайдеров загружается…</p>;
  return (
    <>
      <div className="prov-grid" role="list">
        {presets.map((p) => (
          <button
            key={p.id}
            type="button"
            role="listitem"
            className="prov-tile"
            aria-pressed={p.id === selId}
            data-connected={connected(p.id)}
            onClick={() => pick(p)}
          >
            <span className="prov-tile-name">{shortName(p.name)}</span>
            <span className="prov-tile-tag">{p.tagline || ' '}</span>
            <span className="prov-tile-foot">
              <Badges preset={p} />
              {connected(p.id) && <span className="badge" data-tone="on">подключён</span>}
            </span>
          </button>
        ))}
      </div>

      {sel && (
        <div className="panel prov-form">
          <div className="prov-form-head">
            <span className="panel-title">{sel.name}</span>
            <Badges preset={sel} />
          </div>
          {sel.tagline && <p className="section-lead">{sel.tagline}.</p>}
          {connected(sel.id) && <p className="muted small">Уже подключён — будет добавлен ещё один ({sel.id}-2), например с другим ключом.</p>}

          {sel.needs_key && (
            <Field label="Ключ API">
              <SecretInput value={key} onChange={setKey} placeholder="вставьте ключ" />
            </Field>
          )}
          {sel.key_url && (
            <p className="small">
              <ExternalLink href={sel.key_url}>Где взять ключ</ExternalLink>
              <span className="muted"> — откроется в браузере; скопируйте ключ и вставьте сюда.</span>
            </p>
          )}
          {sel.local && <p className="muted small">Сервер должен быть запущен на этом ПК. Ключ не нужен.</p>}

          {sel.base_url ? (
            <details className="adv">
              <summary>
                Адрес API <span className="mono muted">{url}</span>
              </summary>
              <Field label="Адрес API">
                <input className="input mono" value={url} spellCheck={false} onChange={(e) => setUrl(e.target.value)} />
              </Field>
            </details>
          ) : (
            <Field label="Адрес API (обязательно)">
              <input className="input mono" value={url} spellCheck={false} onChange={(e) => setUrl(e.target.value)} placeholder="http://localhost:8000/v1" />
            </Field>
          )}

          <div className="row">
            <button
              type="button"
              className="btn btn-primary"
              disabled={!url.trim() || (sel.needs_key && !key.trim())}
              onClick={() => {
                onAdd(sel, key, url);
                setSelId('');
                setKey('');
              }}
            >
              Добавить и проверить
            </button>
            <button type="button" className="btn btn-ghost" onClick={() => setSelId('')}>
              Отмена
            </button>
          </div>

          {(sel.note || sel.docs_url) && (
            <details className="adv adv-notes">
              <summary>Подробнее для опытных</summary>
              {sel.note && <p className="small muted">{sel.note}</p>}
              {sel.docs_url && <ExternalLink href={sel.docs_url}>Документация API</ExternalLink>}
            </details>
          )}
        </div>
      )}
    </>
  );
}

/** Провайдеры LLM: выключатели, ключи, проверка, модели с метками и цепочки по ролям. */
export default function ProvidersTab({ draft, update, base, onSaved }: TabProps & { base?: Config | null; onSaved?: (c: Config) => void }) {
  const [presets, setPresets] = useState<ProviderPreset[]>([]);
  const [checks, setChecks] = useState<Record<string, CheckState>>({});
  const [chainInput, setChainInput] = useState<Record<RoleName, string>>({ brain: '', fast: '', vision: '', reflection: '' });
  const [bestMsg, setBestMsg] = useState<string | null>(null);

  useEffect(() => {
    void call<ProviderPreset[]>('provider_presets').then(setPresets).catch(() => setPresets([]));
  }, []);

  const presetOf = (id: string) => {
    const pc = draft.providers.find((x) => x.id === id);
    return pc ? presetFor(presets, pc) : presets.find((x) => x.id === id);
  };
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

  /** «Выбрать лучшее автоматически»: рейтинг jarvis-llm (`apply_best`) записывается в config.json и применяется сразу. */
  const applyBest = async (providerIds: string[] | null, ranking: RoleRanking[]) => {
    try {
      const saved = await call<Config>('provider_apply_best', { config: draft, base: base ?? null, providerIds });
      if (onSaved) onSaved(saved);
      else update(() => saved);
      setBestMsg('Лучшие модели выбраны и сохранены — Джарвис уже использует их');
    } catch (e) {
      // нет результатов проверки на стороне ядра — подставляем рейтинг в черновик, сохранит кнопка «Сохранить»
      update((cfg) => ({ ...cfg, roles: applyRanking(cfg.roles, ranking, cfg.providers) }));
      setBestMsg(`${errorText(e)}. Рейтинг подставлен в черновик — нажмите «Сохранить».`);
    }
  };

  const brainBlocked = blockingDisabled(draft, 'brain');

  return (
    <>
      {brainBlocked.length > 0 && (
        <div className="alert" role="alert">
          <span>
            <strong>«Мозг» без модели:</strong>{' '}
            {brainBlocked.map((id) => providerTitle({ id }, presetOf(id), draft.providers)).join(', ')}{' '}
            {brainBlocked.length === 1 ? 'выключен' : 'выключены'}. Джарвис не сможет отвечать.
          </span>
          <button
            type="button"
            className="btn btn-primary btn-small"
            onClick={() => update((cfg) => brainBlocked.reduce((c, id) => setProviderEnabled(c, id, true), cfg))}
          >
            Включить
          </button>
          <span className="muted small">затем «Сохранить»</span>
        </div>
      )}

      <section className="section">
        <h2 className="section-title">Подключённые провайдеры</h2>
        <p className="section-lead">
          Ключи хранятся только в config.json на этом ПК и показываются замаскированными. «Проверить» запрашивает список моделей и коротко
          тестирует лучшие из них.
        </p>
        {draft.providers.length === 0 && <p className="muted">Пока ни одного — выберите провайдера ниже.</p>}
        {draft.providers.map((p) => (
          <ProviderCard
            key={p.id}
            p={p}
            preset={presetFor(presets, p)}
            draft={draft}
            check={checks[p.id]}
            update={update}
            onCheck={() => void check(p.id, p.base_url, p.api_key)}
            onBest={(ranking) => void applyBest([p.id], ranking)}
          />
        ))}
      </section>

      <section className="section">
        <h2 className="section-title">Добавить провайдера</h2>
        <p className="section-lead">Нажмите на карточку. Для старта хватит одного бесплатного ключа — например, Groq.</p>
        <AddProvider
          presets={presets}
          draft={draft}
          onAdd={(preset, key, url) => {
            const next = addProvider(draft, preset, key, url);
            const added = next.providers[next.providers.length - 1];
            update(() => next);
            void check(added.id, added.base_url, added.api_key);
          }}
        />
      </section>

      <section className="section">
        <h2 className="section-title">Модели по ролям</h2>
        <p className="section-lead">
          Цепочка — порядок перехода при сбоях: если первая модель не ответила (лимит, ошибка сервера), Джарвис берёт следующую.
        </p>
        {allRanking.length > 0 && (
          <button type="button" className="btn" onClick={() => void applyBest(null, allRanking)}>
            ★ Выбрать лучшее автоматически по всем проверкам
          </button>
        )}
        {bestMsg && <p className="status small">{bestMsg}</p>}
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
                {draft.roles[role].map((m, i) => {
                  const st = chainItemState(draft, m);
                  return (
                    <li key={m} data-state={st}>
                      <span className="chain-n">{i + 1}</span>
                      <span className="chain-model" title={m}>
                        {m}
                      </span>
                      {st === 'disabled' && <span className="badge" data-tone="key">провайдер выключен</span>}
                      {st === 'missing' && <span className="badge" data-tone="key">провайдер не подключён</span>}
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
                  );
                })}
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
