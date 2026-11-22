'use client';

import { useCallback, useEffect, useState } from 'react';

import AppShell from '@/components/AppShell';
import ProvidersTab from '@/components/settings/ProvidersTab';
import { AudioTab, PermissionsTab, SearchTab, VoiceTab, WindowsTab } from '@/components/settings/tabs';
import { call, errorText } from '@/lib/bridge';
import { useTauriEvent } from '@/lib/store';
import { isDirty } from '@/lib/settings';
import { SETTINGS_TAB_EVENT, type Config } from '@/lib/types';

const TABS = [
  { id: 'providers', name: 'Провайдеры и модели' },
  { id: 'voice', name: 'Голос и серверы' },
  { id: 'audio', name: 'Аудио' },
  { id: 'permissions', name: 'Разрешения' },
  { id: 'windows', name: 'Окна' },
  { id: 'search', name: 'Поиск' },
] as const;
type TabId = (typeof TABS)[number]['id'];

const isTab = (s: string): s is TabId => TABS.some((t) => t.id === s);

export default function SettingsPage() {
  const [tab, setTab] = useState<TabId>('providers');
  const [saved, setSaved] = useState<Config | null>(null);
  const [draft, setDraft] = useState<Config | null>(null);
  const [status, setStatus] = useState<{ ok: boolean; text: string } | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    const fromHash = () => {
      const h = window.location.hash.replace('#', '');
      if (isTab(h)) setTab(h);
    };
    fromHash();
    window.addEventListener('hashchange', fromHash);
    call<Config>('config_get')
      .then((c) => {
        setSaved(c);
        setDraft(c);
      })
      .catch((e) => setStatus({ ok: false, text: `Не удалось прочитать config.json: ${errorText(e)}` }));
    return () => window.removeEventListener('hashchange', fromHash);
  }, []);

  useTauriEvent<string>(SETTINGS_TAB_EVENT, (t) => {
    if (isTab(t)) setTab(t);
  });

  const update = useCallback((f: (c: Config) => Config) => {
    setDraft((d) => (d ? f(d) : d));
    setStatus(null);
  }, []);

  const select = (id: TabId) => {
    setTab(id);
    history.replaceState(null, '', `#${id}`);
  };

  const save = async () => {
    if (!draft || !saved) return;
    setSaving(true);
    try {
      // позиция виджета и режим могли измениться, пока открыто окно: если в черновике их не трогали — берём свежие
      const latest = await call<Config>('config_get');
      const toSave = JSON.stringify(draft.hud) === JSON.stringify(saved.hud) ? { ...draft, hud: latest.hud } : draft;
      const result = await call<Config>('config_set', { config: toSave });
      setSaved(result);
      setDraft(result);
      setStatus({ ok: true, text: 'Сохранено' });
    } catch (e) {
      setStatus({ ok: false, text: `Не сохранено: ${errorText(e)}` });
    } finally {
      setSaving(false);
    }
  };

  const dirty = isDirty(saved, draft);

  return (
    <AppShell>
      <div className="page-head">
        <h1 className="page-title">Настройки</h1>
      </div>
      <div className="tabs" role="tablist">
        {TABS.map((t) => (
          <button key={t.id} type="button" role="tab" className="tab" aria-selected={tab === t.id} onClick={() => select(t.id)}>
            {t.name}
          </button>
        ))}
      </div>
      {!draft ? (
        <p className="muted">{status?.text ?? 'Загружаю настройки…'}</p>
      ) : (
        <div role="tabpanel">
          {tab === 'providers' && (
            <ProvidersTab
              draft={draft}
              update={update}
              onSaved={(c) => {
                setSaved(c);
                setDraft(c);
              }}
            />
          )}
          {tab === 'voice' && <VoiceTab draft={draft} update={update} />}
          {tab === 'audio' && <AudioTab />}
          {tab === 'permissions' && <PermissionsTab draft={draft} update={update} />}
          {tab === 'windows' && <WindowsTab draft={draft} update={update} />}
          {tab === 'search' && <SearchTab draft={draft} update={update} />}
          <div className="savebar">
            <button type="button" className="btn btn-primary" disabled={!dirty || saving} onClick={() => void save()}>
              {saving ? 'Сохраняю…' : 'Сохранить'}
            </button>
            <button type="button" className="btn btn-ghost" disabled={!dirty || saving} onClick={() => setDraft(saved)}>
              Отменить изменения
            </button>
            {dirty && !status && <span className="muted small">Есть несохранённые изменения</span>}
            {status && <span className={status.ok ? 'ok-text small' : 'error-text small'}>{status.text}</span>}
          </div>
        </div>
      )}
    </AppShell>
  );
}
