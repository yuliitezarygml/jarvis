'use client';

import { useJarvis, useNow } from '@/lib/store';

const SHOW_MS = 8000;

/** Последняя ошибка бэкенда: показывается 8 с. */
export default function ErrorToast() {
  const err = useJarvis((s) => s.lastError);
  const now = useNow(1000);
  if (!err || (now && now - err.at > SHOW_MS)) return null;
  return (
    <div className="toast toast-error" role="alert">
      {err.message}
    </div>
  );
}
