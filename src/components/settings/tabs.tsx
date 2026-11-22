'use client';

import { useEffect, useState } from 'react';

import { call, errorText } from '@/lib/bridge';
import { addSearch, moveItem } from '@/lib/settings';
import type { AppInfo, AudioDevices, Autonomy, HudMode, SearchPreset, ServiceCheck } from '@/lib/types';

import { CheckStatus, Field, KeyField, Toggle, type TabProps } from './common';

// ───────────── Голос и серверы ─────────────

export function VoiceTab({ draft, update }: TabProps) {
  const [tts, setTts] = useState<ServiceCheck | 'loading'>();
  const [stt, setStt] = useState<ServiceCheck | 'loading'>();
  const run = async (kind: 'tts' | 'stt', url: string, set: (v: ServiceCheck | 'loading') => void) => {
    set('loading');
    try {
      set(await call<ServiceCheck>('voice_check', { kind, url }));
    } catch (e) {
      set({ demo: false, ok: false, message: errorText(e), latency_ms: null });
    }
  };
  return (
    <>
      <section className="section">
        <h2 className="section-title">Синтез речи (голос Джарвиса)</h2>
        <p className="section-lead">Сервер jarvis-tts-rs на Xeon. Соединение защищено закреплённым сертификатом CA.</p>
        <Field label="Адрес сервера">
          <input className="input" value={draft.tts.url} spellCheck={false} onChange={(e) => update((c) => ({ ...c, tts: { ...c.tts, url: e.target.value } }))} />
        </Field>
        <Field label="Голос">
          <input className="input" value={draft.tts.voice} onChange={(e) => update((c) => ({ ...c, tts: { ...c.tts, voice: e.target.value } }))} />
        </Field>
        <Field label="Сертификат CA (путь от корня проекта)">
          <input className="input mono" value={draft.tts.ca_path} onChange={(e) => update((c) => ({ ...c, tts: { ...c.tts, ca_path: e.target.value } }))} />
        </Field>
        <button type="button" className="btn btn-primary" onClick={() => void run('tts', draft.tts.url, setTts)}>
          Проверить
        </button>
        <CheckStatus check={tts} />
      </section>
      <section className="section">
        <h2 className="section-title">Распознавание речи</h2>
        <p className="section-lead">Сервер на Xeon («не у всех мощный ПК»); запасной вариант — Groq Whisper по ключу Groq.</p>
        <Field label="Движок">
          <select className="select" value={draft.stt.engine} onChange={(e) => update((c) => ({ ...c, stt: { ...c.stt, engine: e.target.value } }))}>
            <option value="server">Свой сервер (jarvis-stt-rs)</option>
            <option value="groq_whisper">Groq Whisper (облако)</option>
          </select>
        </Field>
        <Field label="Адрес сервера">
          <input className="input" value={draft.stt.url} spellCheck={false} onChange={(e) => update((c) => ({ ...c, stt: { ...c.stt, url: e.target.value } }))} />
        </Field>
        <Field label="Сертификат CA">
          <input className="input mono" value={draft.stt.ca_path} onChange={(e) => update((c) => ({ ...c, stt: { ...c.stt, ca_path: e.target.value } }))} />
        </Field>
        <button type="button" className="btn btn-primary" onClick={() => void run('stt', draft.stt.url, setStt)}>
          Проверить
        </button>
        <CheckStatus check={stt} />
      </section>
    </>
  );
}

// ───────────── Аудио ─────────────

export function AudioTab() {
  const [dev, setDev] = useState<AudioDevices | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    call<AudioDevices>('audio_devices')
      .then(setDev)
      .catch((e) => setError(errorText(e)));
  }, []);
  return (
    <section className="section">
      <h2 className="section-title">Микрофон и динамики</h2>
      <p className="section-lead">
        Джарвис берёт устройства Windows по умолчанию (отмечены в списке); выбор другого устройства в config.json пока не
        поддерживается. Микрофон включается и выключается кнопкой в HUD.
        {dev?.demo && ' Список ниже — демо-данные.'}
      </p>
      {error && <p className="status" data-ok="false">{error}</p>}
      {dev && (
        <>
          <Field label="Микрофон">
            <select className="select" defaultValue={dev.inputs.find((d) => d.default)?.id} disabled>
              {dev.inputs.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.name}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Вывод звука">
            <select className="select" defaultValue={dev.outputs.find((d) => d.default)?.id} disabled>
              {dev.outputs.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.name}
                </option>
              ))}
            </select>
          </Field>
          <p className="muted small">Без наушников Джарвис может услышать сам себя: аппаратного подавления эха в Windows нет.</p>
        </>
      )}
    </section>
  );
}

// ───────────── Разрешения ─────────────

const AUTONOMY: { v: Autonomy; title: string; desc: string }[] = [
  { v: 'ask_dangerous', title: 'Спрашивать только про опасное', desc: 'Подтверждение для T3: удаление вне рабочих папок, службы, реестр HKLM, установка программ, выключение.' },
  { v: 'ask_all_changes', title: 'Спрашивать про любые изменения', desc: 'Подтверждение для T2 и T3: любые изменения файлов, настроек и реестра.' },
  { v: 'never', title: 'Не спрашивать', desc: 'Джарвис выполняет всё сам. Только если вы полностью доверяете распознаванию речи.' },
];

export function PermissionsTab({ draft, update }: TabProps) {
  const [info, setInfo] = useState<AppInfo | null>(null);
  useEffect(() => {
    void call<AppInfo>('app_info').then(setInfo).catch(() => setInfo(null));
  }, []);
  return (
    <>
      <section className="section">
        <h2 className="section-title">Когда спрашивать подтверждение</h2>
        {AUTONOMY.map((a) => (
          <label className="toggle" key={a.v}>
            <input type="radio" name="autonomy" checked={draft.autonomy === a.v} onChange={() => update((c) => ({ ...c, autonomy: a.v }))} />
            <span className="toggle-text">
              <span>{a.title}</span>
              <span className="toggle-desc">{a.desc}</span>
            </span>
          </label>
        ))}
      </section>
      <section className="section">
        <h2 className="section-title">Права и возможности</h2>
        <Toggle
          checked={draft.run_as_admin}
          onChange={(v) => update((c) => ({ ...c, run_as_admin: v }))}
          title="Запускать от администратора"
          desc={
            info
              ? `Сейчас: ${info.elevated ? 'с правами администратора' : 'без прав администратора'}${info.debug_build ? ' (отладочная сборка запускается без UAC)' : ''}.`
              : 'Релизная сборка запрашивает права при старте (один запрос UAC).'
          }
        />
        <Toggle
          checked={draft.vision_enabled}
          onChange={(v) => update((c) => ({ ...c, vision_enabled: v }))}
          title="Разрешить зрение"
          desc="Скриншоты экрана отправляются выбранной vision-модели, когда UI Automation не хватает. Окна Джарвиса на скриншоты не попадают."
        />
        <Toggle
          checked={draft.terminal_autorun}
          onChange={(v) => update((c) => ({ ...c, terminal_autorun: v }))}
          title="Сам вводить исправленную команду в терминал"
          desc="Для безопасных команд (T0–T2). Опасные — только после «да, сэр». Выключено — Джарвис только подсказывает."
        />
        <Toggle
          checked={draft.resource_guard}
          onChange={(v) => update((c) => ({ ...c, resource_guard: v }))}
          title="Следить за ресурсами"
          desc="Память > 90 % 30 с, процессор > 95 % минуту или диск почти заполнен — Джарвис скажет об этом и предложит, что закрыть."
        />
      </section>
      <section className="section">
        <h2 className="section-title">Где искать файлы</h2>
        <Field label="Папки для поиска файлов, по одной в строке">
          <textarea
            className="textarea mono"
            value={draft.search_roots.join('\n')}
            onChange={(e) => update((c) => ({ ...c, search_roots: e.target.value.split('\n').map((s) => s.trim()).filter(Boolean) }))}
          />
        </Field>
      </section>
    </>
  );
}

// ───────────── Окна ─────────────

export function WindowsTab({ draft, update }: TabProps) {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  useEffect(() => {
    void call<AppInfo>('app_info').then(setInfo).catch(() => setInfo(null));
  }, []);
  const switchNow = async (mode: HudMode) => {
    try {
      await call('set_hud_mode', { mode });
      update((c) => ({ ...c, hud: { ...c.hud, mode } }));
      setMsg(mode === 'fullscreen' ? 'HUD развёрнут' : 'HUD свёрнут в угол');
    } catch (e) {
      setMsg(errorText(e));
    }
  };
  return (
    <>
      <section className="section">
        <h2 className="section-title">Режим HUD</h2>
        <p className="section-lead">Переключается и сразу запоминается. То же делают Ctrl+Alt+J, меню в трее и голос: «Джарвис, во весь экран» / «в угол».</p>
        <div className="row">
          <button type="button" className={draft.hud.mode === 'fullscreen' ? 'btn btn-primary' : 'btn'} onClick={() => void switchNow('fullscreen')}>
            Во весь экран
          </button>
          <button type="button" className={draft.hud.mode === 'corner' ? 'btn btn-primary' : 'btn'} onClick={() => void switchNow('corner')}>
            В угол
          </button>
        </div>
        {msg && <p className="status">{msg}</p>}
      </section>
      <section className="section">
        <h2 className="section-title">Виджет в углу</h2>
        <p className="section-lead">
          Перетащите виджет мышью — позиция сохранится.{' '}
          {draft.hud.corner_x !== null && draft.hud.corner_y !== null
            ? `Сейчас: ${draft.hud.corner_x}, ${draft.hud.corner_y} (пиксели экрана).`
            : 'Сейчас: правый нижний угол основного монитора.'}
        </p>
        <button type="button" className="btn" onClick={() => update((c) => ({ ...c, hud: { ...c.hud, corner_x: null, corner_y: null } }))}>
          Вернуть в правый нижний угол
        </button>
        <p className="muted small">Новая позиция применится после сохранения и перезапуска.</p>
      </section>
      <section className="section">
        <h2 className="section-title">Горячие клавиши</h2>
        <table className="table" style={{ maxWidth: 560 }}>
          <tbody>
            <tr>
              <td className="mono">Ctrl+Alt+J</td>
              <td>Весь экран ↔ угол</td>
            </tr>
            <tr>
              <td className="mono">Ctrl+Alt+K</td>
              <td>Аварийная остановка: отменить задачу, отпустить мышь и клавиатуру</td>
            </tr>
            <tr>
              <td className="mono">Esc</td>
              <td>В HUD — свернуть в угол</td>
            </tr>
            <tr>
              <td className="mono">Ctrl+Shift+D</td>
              <td>Показать симулятор</td>
            </tr>
          </tbody>
        </table>
      </section>
      {info && (
        <section className="section">
          <h2 className="section-title">О приложении</h2>
          <p className="muted small">
            Версия {info.version} · {info.debug_build ? 'отладочная сборка' : 'релиз'} · конфиг: <span className="mono">{info.config_path}</span>
          </p>
        </section>
      )}
    </>
  );
}

// ───────────── Поиск ─────────────

export function SearchTab({ draft, update }: TabProps) {
  const [presets, setPresets] = useState<SearchPreset[]>([]);
  const [checks, setChecks] = useState<Record<string, ServiceCheck | 'loading'>>({});
  const [addId, setAddId] = useState('');
  useEffect(() => {
    void call<SearchPreset[]>('search_presets').then(setPresets).catch(() => setPresets([]));
  }, []);
  const check = async (id: string, apiKey: string, extra: string) => {
    setChecks((c) => ({ ...c, [id]: 'loading' }));
    try {
      const r = await call<ServiceCheck>('search_check', { id, apiKey, extra });
      setChecks((c) => ({ ...c, [id]: r }));
    } catch (e) {
      setChecks((c) => ({ ...c, [id]: { demo: false, ok: false, message: errorText(e), latency_ms: null } }));
    }
  };
  const available = presets.filter((p) => !draft.web_search.some((s) => s.id === p.id));
  return (
    <>
      <section className="section">
        <h2 className="section-title">Поиск в интернете</h2>
        <p className="section-lead">
          Порядок — цепочка запасных вариантов: если первый поисковик не ответил, Джарвис спросит следующий. DuckDuckGo работает без
          ключа и стоит последним.
        </p>
        {draft.web_search.map((s, i) => {
          const p = presets.find((x) => x.id === s.id);
          return (
            <div className="card" key={s.id}>
              <div className="card-head">
                <span className="chain-n">{i + 1}</span>
                <span className="card-title">{p?.name ?? s.id}</span>
                <label className="row small" style={{ marginLeft: 'auto' }}>
                  <input
                    type="checkbox"
                    checked={s.enabled}
                    onChange={(e) => update((c) => ({ ...c, web_search: c.web_search.map((x, j) => (j === i ? { ...x, enabled: e.target.checked } : x)) }))}
                  />
                  включён
                </label>
                <button type="button" className="btn btn-small btn-ghost" aria-label="Выше" disabled={i === 0} onClick={() => update((c) => ({ ...c, web_search: moveItem(c.web_search, i, -1) }))}>
                  ↑
                </button>
                <button
                  type="button"
                  className="btn btn-small btn-ghost"
                  aria-label="Ниже"
                  disabled={i === draft.web_search.length - 1}
                  onClick={() => update((c) => ({ ...c, web_search: moveItem(c.web_search, i, 1) }))}
                >
                  ↓
                </button>
              </div>
              {(p?.needs_key ?? true) && (
                <KeyField
                  key={s.api_key.includes('…') ? s.api_key : `${s.id}-key`}
                  value={s.api_key}
                  onChange={(v) => update((c) => ({ ...c, web_search: c.web_search.map((x, j) => (j === i ? { ...x, api_key: v } : x)) }))}
                  label="Ключ поиска"
                />
              )}
              {p?.extra_label && (
                <Field label={p.extra_label}>
                  <input
                    className="input"
                    value={s.extra}
                    onChange={(e) => update((c) => ({ ...c, web_search: c.web_search.map((x, j) => (j === i ? { ...x, extra: e.target.value } : x)) }))}
                  />
                </Field>
              )}
              <div className="row">
                <button type="button" className="btn btn-primary" onClick={() => void check(s.id, s.api_key, s.extra)}>
                  Проверить
                </button>
                <button type="button" className="btn btn-ghost" onClick={() => update((c) => ({ ...c, web_search: c.web_search.filter((_, j) => j !== i) }))}>
                  Убрать
                </button>
              </div>
              <CheckStatus check={checks[s.id]} />
            </div>
          );
        })}
      </section>
      {available.length > 0 && (
        <section className="section">
          <h2 className="section-title">Добавить поисковик</h2>
          <div className="row">
            <select className="select grow" value={addId} onChange={(e) => setAddId(e.target.value)}>
              <option value="">выберите…</option>
              {available.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
            <button
              type="button"
              className="btn"
              disabled={!addId}
              onClick={() => {
                const p = presets.find((x) => x.id === addId);
                if (p) update((c) => addSearch(c, p));
                setAddId('');
              }}
            >
              Добавить
            </button>
          </div>
        </section>
      )}
    </>
  );
}
