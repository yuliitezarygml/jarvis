'use client';

import { useEffect, useState } from 'react';

import { call, inTauri } from '@/lib/bridge';
import { MOOD_ICON, MOOD_LABEL, parseMoodPayload } from '@/lib/persona';
import { useTauriEvent } from '@/lib/store';
import { PERSONA_MOOD_EVENT, type PersonaMood, type PersonaMoodPayload } from '@/lib/types';

/**
 * Настроение Джарвиса: при открытии окна — текущее (команда `persona_mood`, чтобы значок не пропадал после
 * перезагрузки окна), дальше — из события `persona:mood`. `null` в браузере без Tauri (демо значок не показывает).
 */
export function usePersonaMood(): PersonaMoodPayload | null {
  const [mood, setMood] = useState<PersonaMoodPayload | null>(null);
  useEffect(() => {
    if (!inTauri()) return;
    let alive = true;
    call<unknown>('persona_mood')
      .then((p) => {
        const m = parseMoodPayload(p);
        // событие могло прийти раньше ответа — не затираем его
        if (alive && m) setMood((cur) => cur ?? m);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);
  useTauriEvent<unknown>(PERSONA_MOOD_EVENT, (p) => {
    if (!inTauri()) return;
    const m = parseMoodPayload(p);
    if (m) setMood(m);
  });
  return mood;
}

/** Значок настроения Джарвиса: спокоен / раздражён / обижен; причина — во всплывающей подсказке. */
export default function MoodBadge({
  mood,
  reason,
  compact = false,
  className,
}: {
  mood: PersonaMood;
  reason?: string | null;
  /** Только значок, без подписи (виджет в углу). */
  compact?: boolean;
  className?: string;
}) {
  const label = MOOD_LABEL[mood];
  const tip = reason ? `Настроение: ${label} — ${reason}` : `Настроение: ${label}`;
  return (
    <span className={['mood-badge', className].filter(Boolean).join(' ')} data-mood={mood} data-compact={compact ? '' : undefined} title={tip} role="status" aria-label={tip}>
      <span className="mood-icon" aria-hidden="true">
        {MOOD_ICON[mood]}
      </span>
      {!compact && <span className="mood-label">{label}</span>}
    </span>
  );
}
