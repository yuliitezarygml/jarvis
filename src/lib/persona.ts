// Характер Джарвиса в интерфейсе: подписи режимов, категорий памяти, настроения и чистые помощники
// (вкладка «Характер», экран «Память», значок настроения).

import type { Fact, PersonaMode, PersonaMood, PersonaMoodPayload, TeaseLevel } from './types';

export const PERSONA_MODES: { v: PersonaMode; title: string; desc: string; warn?: string }[] = [
  {
    v: 'butler',
    title: 'Дворецкий',
    desc: 'Вежливо, на «вы», «сэр», лёгкая ирония. Без мата. Для презентаций и при гостях.',
  },
  {
    v: 'buddy',
    title: 'Свой парень',
    desc: 'На «ты», по-простому, дружеский стёб. Без мата.',
  },
  {
    v: 'uncensored',
    title: 'Без цензуры',
    desc: 'На «ты», как друг. Матерится в ответ, только когда материтесь вы; если его послать — может послать в ответ.',
    warn: 'Для личного использования, не для презентаций.',
  },
];

export const TEASE_LEVELS: { v: TeaseLevel; title: string }[] = [
  { v: 'rare', title: 'Редко' },
  { v: 'sometimes', title: 'Иногда' },
  { v: 'often', title: 'Часто' },
];

/** Голосовые команды переключения режима. */
export const PERSONA_VOICE_COMMANDS: { phrase: string; mode: PersonaMode }[] = [
  { phrase: '«Джарвис, будь культурным»', mode: 'butler' },
  { phrase: '«Джарвис, давай на ты»', mode: 'buddy' },
  { phrase: '«Джарвис, можешь материться»', mode: 'uncensored' },
];

export function personaTitle(mode: PersonaMode): string {
  return PERSONA_MODES.find((m) => m.v === mode)?.title ?? mode;
}

/** Подсказка для поля «Как обращаться» — что будет при пустом значении. */
export function addressPlaceholder(mode: PersonaMode): string {
  return mode === 'butler' ? 'по режиму: «сэр»' : 'по режиму: без обращения';
}

// ───────────── запретные слова ─────────────

const norm = (w: string) => w.trim().replace(/\s+/g, ' ').toLowerCase();

/** Добавить слова из поля ввода (через запятую или точку с запятой): без пустых и повторов, в нижнем регистре. */
export function addBannedWords(list: string[], input: string): string[] {
  const out = [...list];
  const seen = new Set(list.map(norm));
  for (const raw of input.split(/[,;\n]/)) {
    const w = norm(raw);
    if (!w || seen.has(w)) continue;
    seen.add(w);
    out.push(w);
  }
  return out;
}

export function removeBannedWord(list: string[], word: string): string[] {
  const w = norm(word);
  return list.filter((x) => norm(x) !== w);
}

// ───────────── память ─────────────

export const FACT_CATEGORY_LABEL: Record<string, string> = {
  preference: 'Предпочтение',
  personal: 'Личное',
  alias: 'Название',
  habit: 'Привычка',
  contact: 'Контакт',
  general: 'Общее',
  style: 'Характер',
};

/** Категории для выбора в редакторе факта (порядок показа). */
export const FACT_CATEGORIES = ['general', 'preference', 'personal', 'alias', 'habit', 'contact', 'style'] as const;

export function categoryLabel(c: string | undefined | null): string {
  const k = (c ?? '').trim();
  if (!k) return FACT_CATEGORY_LABEL.general;
  return FACT_CATEGORY_LABEL[k] ?? k;
}

export type FactFilter = 'all' | 'style';

export function filterFacts(facts: Fact[], filter: FactFilter): Fact[] {
  return filter === 'style' ? facts.filter((f) => f.category === 'style') : facts;
}

// ───────────── настроение ─────────────

export const MOOD_LABEL: Record<PersonaMood, string> = {
  calm: 'спокоен',
  irritated: 'раздражён',
  offended: 'обижен',
};

export const MOOD_ICON: Record<PersonaMood, string> = {
  calm: '◉',
  irritated: '⚡',
  offended: '❄',
};

/** Проверить payload события `persona:mood`; мусор → null. */
export function parseMoodPayload(p: unknown): PersonaMoodPayload | null {
  if (!p || typeof p !== 'object') return null;
  const { mood, reason } = p as { mood?: unknown; reason?: unknown };
  if (mood !== 'calm' && mood !== 'irritated' && mood !== 'offended') return null;
  const r = typeof reason === 'string' && reason.trim() ? reason.trim() : undefined;
  return { mood, reason: r };
}
