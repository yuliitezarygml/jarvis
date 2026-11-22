'use client';

import { useJarvis, useNow } from '@/lib/store';

const HOLD_MS = 7000;

/** Субтитры: текущая реплика крупно, предыдущая — тусклее. Финальная реплика гаснет через 7 с. */
export default function Subtitles() {
  const current = useJarvis((s) => s.subtitle);
  const history = useJarvis((s) => s.subtitleHistory);
  const now = useNow(500);
  const prev = history.length > 0 && current && history[history.length - 1].at === current.at ? history[history.length - 2] : history[history.length - 1];
  const stale = current !== null && current.final && now - current.at > HOLD_MS;
  return (
    <div className="subtitles" aria-live="polite">
      {prev && !stale && (
        <p className="sub-line sub-prev" data-speaker={prev.speaker}>
          <span className="sub-who">{prev.speaker === 'user' ? 'Вы' : 'Джарвис'}</span>
          {prev.text}
        </p>
      )}
      {current && !stale ? (
        <p className="sub-line sub-current" data-speaker={current.speaker} data-final={current.final}>
          <span className="sub-who">{current.speaker === 'user' ? 'Вы' : 'Джарвис'}</span>
          {current.text}
        </p>
      ) : (
        <p className="sub-line sub-hint">Скажите «Джарвис, …»</p>
      )}
    </div>
  );
}
