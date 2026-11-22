'use client';

import { useCallback, useEffect, useState } from 'react';

import { call, errorText } from '@/lib/bridge';
import type { AssistantStatus, MicStatus } from '@/lib/types';

/** Как часто обновлять сводку (модель, микрофон), пока HUD открыт. */
const STATUS_POLL_MS = 5000;

/**
 * Нижняя панель HUD: текстовая команда («Сказать Джарвису…» — то же, что голосом), кнопка микрофона
 * и постоянная плашка, если языковая модель недоступна (например, нет ключа LLM).
 */
export default function CommandBar() {
  const [text, setText] = useState('');
  const [status, setStatus] = useState<AssistantStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [micBusy, setMicBusy] = useState(false);

  const refresh = useCallback(() => {
    call<AssistantStatus>('assistant_status')
      .then(setStatus)
      .catch(() => setStatus(null));
  }, []);

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, STATUS_POLL_MS);
    return () => clearInterval(t);
  }, [refresh]);

  const submit = async () => {
    const t = text.trim();
    if (!t) return;
    try {
      await call('agent_submit', { text: t });
      setText('');
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  };

  const toggleMic = async () => {
    if (!status) return;
    setMicBusy(true);
    try {
      const mic = await call<MicStatus>('mic_set', { enabled: !status.mic.enabled });
      setStatus({ ...status, mic });
    } catch (e) {
      setError(errorText(e));
    } finally {
      setMicBusy(false);
    }
  };

  const mic = status?.mic;
  const micLabel = !mic ? 'Микрофон' : mic.enabled ? (mic.listening ? 'Слушаю' : 'Микрофон: ошибка') : 'Микрофон выкл.';
  const micTitle = mic?.error ?? (mic?.enabled ? 'Выключить микрофон' : 'Включить микрофон');

  return (
    <div className="command-bar">
      {status && !status.llm_ready && status.llm_problem && (
        <p className="command-warn" role="alert">
          {status.llm_problem}
          <button type="button" className="btn btn-small" onClick={() => void call('open_settings', { tab: 'providers' })}>
            Открыть настройки
          </button>
        </p>
      )}
      <form
        className="command-form"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <button
          type="button"
          className="btn btn-small command-mic"
          data-on={mic?.enabled && mic?.listening ? 'true' : 'false'}
          data-error={mic?.enabled && !mic?.listening ? 'true' : 'false'}
          aria-pressed={mic?.enabled ?? false}
          disabled={!status || micBusy}
          title={micTitle}
          onClick={() => void toggleMic()}
        >
          <span className="state-dot" aria-hidden="true" />
          {micBusy ? '…' : micLabel}
        </button>
        <input
          className="input command-input"
          value={text}
          placeholder="Сказать Джарвису…"
          aria-label="Команда Джарвису текстом"
          spellCheck={false}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => e.stopPropagation()}
        />
        <button type="submit" className="btn btn-primary btn-small" disabled={!text.trim()}>
          Отправить
        </button>
      </form>
      {error && <p className="command-error small">{error}</p>}
      {mic?.enabled && mic.error && !mic.listening && <p className="command-error small">{mic.error}</p>}
    </div>
  );
}
