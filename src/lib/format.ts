// Форматирование чисел для HUD по-русски: десятичная запятая, неразрывный пробел перед единицами.

const NBSP = ' ';

/** Число с запятой: 7.84 → «7,8». */
export function fmtNumber(v: number, digits = 1): string {
  if (!Number.isFinite(v)) return '—';
  return v.toFixed(digits).replace('.', ',');
}

/** 12.6 → «13 %». */
export function fmtPercent(v: number | null | undefined): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return '—';
  return `${Math.round(Math.min(100, Math.max(0, v)))}${NBSP}%`;
}

/** Мегабайты: < 1024 → «512 МБ», иначе «7,8 ГБ». */
export function fmtMb(mb: number): string {
  if (!Number.isFinite(mb) || mb < 0) return '—';
  if (mb < 1024) return `${Math.round(mb)}${NBSP}МБ`;
  return `${fmtNumber(mb / 1024)}${NBSP}ГБ`;
}

/** «7,8 / 16,0 ГБ». */
export function fmtRam(usedMb: number, totalMb: number): string {
  if (!Number.isFinite(usedMb) || !Number.isFinite(totalMb) || totalMb <= 0) return '—';
  return `${fmtNumber(usedMb / 1024)} / ${fmtNumber(totalMb / 1024)}${NBSP}ГБ`;
}

export function ramPercent(usedMb: number, totalMb: number): number {
  return totalMb > 0 ? Math.min(100, Math.max(0, (usedMb / totalMb) * 100)) : 0;
}

/** Скорость сети, кбит/с → «850 кбит/с» / «12,4 Мбит/с». */
export function fmtNet(kbps: number | null | undefined): string {
  if (kbps === null || kbps === undefined || !Number.isFinite(kbps)) return '—';
  if (kbps < 1000) return `${Math.round(kbps)}${NBSP}кбит/с`;
  return `${fmtNumber(kbps / 1000)}${NBSP}Мбит/с`;
}

/** Задержка: < 1000 мс → «850 мс», иначе «1,2 с». */
export function fmtLatency(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return '—';
  if (ms < 1000) return `${Math.round(ms)}${NBSP}мс`;
  return `${fmtNumber(ms / 1000)}${NBSP}с`;
}

/** Контекст модели: 131072 → «128k». */
export function fmtCtx(ctx: number): string {
  if (!Number.isFinite(ctx) || ctx <= 0) return '—';
  if (ctx >= 1_000_000) return `${fmtNumber(ctx / 1_048_576)}M`;
  if (ctx >= 1000) return `${Math.round(ctx / 1024)}k`;
  return String(ctx);
}

/** Обратный отсчёт: 19400 → «20 с». */
export function fmtCountdown(ms: number): string {
  return `${Math.max(0, Math.ceil(ms / 1000))}${NBSP}с`;
}

/** Время «09:05:03». */
export function fmtClock(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

export type Level = 'ok' | 'warn' | 'crit';

/** Уровень нагрузки для подсветки: ≥90 — критично, ≥70 — внимание. */
export function loadLevel(pct: number | null | undefined): Level {
  if (pct === null || pct === undefined || !Number.isFinite(pct)) return 'ok';
  if (pct >= 90) return 'crit';
  if (pct >= 70) return 'warn';
  return 'ok';
}

/** Точки SVG-спарклайна в прямоугольнике w×h; значения 0..max. */
export function sparkPoints(values: number[], w: number, h: number, max = 100): string {
  if (values.length === 0) return '';
  const n = values.length;
  const step = n > 1 ? w / (n - 1) : 0;
  return values
    .map((v, i) => {
      const y = h - (Math.min(max, Math.max(0, v)) / max) * h;
      return `${(i * step).toFixed(1)},${y.toFixed(1)}`;
    })
    .join(' ');
}
