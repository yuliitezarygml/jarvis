'use client';

import { useEffect, useRef, useState } from 'react';

import { call, errorText } from '@/lib/bridge';
import { fmtCountdown } from '@/lib/format';
import { TIER_LABEL } from '@/lib/glow';
import { confirmRemainingMs } from '@/lib/reducer';
import { useJarvis, useNow } from '@/lib/store';

/**
 * Запрос подтверждения опасного действия (T3): «Да» / «Нет» и обратный отсчёт.
 * По умолчанию в фокусе «Нет» — случайный Enter ничего опасного не запустит.
 * Таймаут обрабатывает бэкенд (оболочка сама отвечает «нет», если ответа не было).
 */
export default function ConfirmDialog({ compact = false }: { compact?: boolean }) {
  const confirm = useJarvis((s) => s.confirm);
  const now = useNow(250);
  const noRef = useRef<HTMLButtonElement>(null);
  const [sending, setSending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (confirm) {
      setSending(null);
      setError(null);
      noRef.current?.focus();
    }
  }, [confirm]);

  if (!confirm) return null;
  const remaining = confirmRemainingMs(confirm, now || Date.now());
  const frac = confirm.timeoutMs > 0 ? remaining / confirm.timeoutMs : 0;
  const expired = remaining <= 0;

  const answer = async (approved: boolean) => {
    setSending(confirm.id);
    try {
      await call('confirm_respond', { id: confirm.id, approved });
    } catch (e) {
      setError(errorText(e));
      setSending(null);
    }
  };

  const busy = sending === confirm.id || expired;
  return (
    <div
      className={compact ? 'confirm confirm-compact' : 'confirm'}
      role="alertdialog"
      aria-labelledby={compact ? undefined : 'confirm-title'}
      aria-label={compact ? 'Подтвердите действие' : undefined}
      aria-describedby="confirm-summary"
    >
      <div className="confirm-head">
        <span className="tier-badge" data-tier={confirm.tier}>
          {TIER_LABEL[confirm.tier]}
        </span>
        <span className="confirm-timer" aria-label={`Осталось ${fmtCountdown(remaining)}`}>
          {expired ? 'время вышло' : fmtCountdown(remaining)}
        </span>
      </div>
      {!compact && (
        <h2 id="confirm-title" className="confirm-title">
          Подтвердите действие
        </h2>
      )}
      <p id="confirm-summary" className="confirm-summary">
        {confirm.summary}
      </p>
      <div className="confirm-bar" aria-hidden="true">
        <span style={{ transform: `scaleX(${frac})` }} />
      </div>
      <div className="confirm-actions">
        <button type="button" className="btn btn-danger" disabled={busy} onClick={() => void answer(true)}>
          Да, выполнить
        </button>
        <button type="button" ref={noRef} className="btn" disabled={busy} onClick={() => void answer(false)}>
          Нет
        </button>
      </div>
      {!compact && <p className="muted small">Можно ответить голосом: «да, сэр» или «нет».</p>}
      {error && <p className="error-text small">{error}</p>}
    </div>
  );
}
