'use client';

import { useState } from 'react';

import { inTauri } from '@/lib/bridge';
import { MOOD_ICON, MOOD_LABEL, parseMoodPayload } from '@/lib/persona';
import { useTauriEvent } from '@/lib/store';
import { PERSONA_MOOD_EVENT, type PersonaMood, type PersonaMoodPayload } from '@/lib/types';

/**
 * Последнее настроение из события `persona:mood`. `null` до первого события и всегда в браузере без Tauri
 * (демо-режим значок не показывает).
 */
export function usePersonaMood(): PersonaMoodPayload | null {
  const [mood, setMood] = useState<PersonaMoodPayload | null>(null);
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
