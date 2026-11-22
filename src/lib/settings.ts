// Чистые операции над черновиком конфига в экране настроек.

import type { Config, ProviderPreset, RoleName, RoleRanking, Roles, SearchPreset } from './types';

export const ROLE_NAMES: RoleName[] = ['brain', 'fast', 'vision', 'reflection'];

export const ROLE_LABEL: Record<RoleName, { name: string; what: string }> = {
  brain: { name: 'Мозг', what: 'Главная модель: инструменты, русский, длинный контекст' },
  fast: { name: 'Быстрая', what: 'Короткие ответы и простые команды' },
  vision: { name: 'Зрение', what: 'Скриншоты, когда UI Automation не хватает' },
  reflection: { name: 'Рефлексия', what: 'Разбор ошибок и уроки — дёшево, большой контекст' },
};

function uniqueId(taken: string[], base: string): string {
  if (!taken.includes(base)) return base;
  for (let i = 2; ; i++) if (!taken.includes(`${base}-${i}`)) return `${base}-${i}`;
}

export function addProvider(cfg: Config, preset: ProviderPreset, apiKey: string, baseUrl?: string): Config {
  const id = uniqueId(
    cfg.providers.map((p) => p.id),
    preset.id,
  );
  return {
    ...cfg,
    providers: [...cfg.providers, { id, kind: preset.kind, base_url: (baseUrl ?? preset.base_url).trim(), api_key: apiKey.trim(), enabled: true }],
  };
}

/** Удалить провайдера и все его модели из цепочек ролей. */
export function removeProvider(cfg: Config, id: string): Config {
  const keep = (m: string) => !m.startsWith(`${id}/`);
  const roles = Object.fromEntries(ROLE_NAMES.map((r) => [r, cfg.roles[r].filter(keep)])) as unknown as Roles;
  return { ...cfg, providers: cfg.providers.filter((p) => p.id !== id), roles };
}

export function moveItem<T>(list: T[], index: number, delta: -1 | 1): T[] {
  const j = index + delta;
  if (index < 0 || index >= list.length || j < 0 || j >= list.length) return list;
  const next = list.slice();
  [next[index], next[j]] = [next[j], next[index]];
  return next;
}

export function addToChain(roles: Roles, role: RoleName, model: string): Roles {
  if (!model || roles[role].includes(model)) return roles;
  return { ...roles, [role]: [...roles[role], model] };
}

export function removeFromChain(roles: Roles, role: RoleName, model: string): Roles {
  return { ...roles, [role]: roles[role].filter((m) => m !== model) };
}

/** «Выбрать лучшее автоматически»: рекомендованные модели встают в начало цепочек, остальные — следом. */
export function applyRanking(roles: Roles, ranking: RoleRanking[]): Roles {
  const next = { ...roles };
  for (const r of ranking) {
    if (!ROLE_NAMES.includes(r.role) || r.top.length === 0) continue;
    const top = r.top.map((t) => t.model);
    next[r.role] = [...top, ...roles[r.role].filter((m) => !top.includes(m))];
  }
  return next;
}

export function addSearch(cfg: Config, preset: SearchPreset): Config {
  if (cfg.web_search.some((s) => s.id === preset.id)) return cfg;
  return { ...cfg, web_search: [...cfg.web_search, { id: preset.id, api_key: '', extra: '', enabled: true }] };
}

/** Включить/выключить провайдера по id (а не по индексу — список может поменяться, пока открыт экран). */
export function setProviderEnabled(cfg: Config, id: string, enabled: boolean): Config {
  return { ...cfg, providers: cfg.providers.map((p) => (p.id === id ? { ...p, enabled } : p)) };
}

/** Id провайдера из ссылки «provider/model». */
export function providerOf(modelRef: string): string {
  const i = modelRef.indexOf('/');
  return i > 0 ? modelRef.slice(0, i) : '';
}

export type ChainItemState = 'ok' | 'disabled' | 'missing';

/** Модель цепочки пропускается, если её провайдер выключен или не подключён. */
export function chainItemState(cfg: Config, modelRef: string): ChainItemState {
  const p = cfg.providers.find((x) => x.id === providerOf(modelRef));
  if (!p) return 'missing';
  return p.enabled ? 'ok' : 'disabled';
}

/**
 * Выключенные провайдеры, из-за которых у роли не остаётся ни одной модели (пусто — роль в порядке
 * или проблема не в выключателе).
 */
export function blockingDisabled(cfg: Config, role: RoleName): string[] {
  const chain = cfg.roles[role];
  if (chain.length === 0 || chain.some((m) => chainItemState(cfg, m) === 'ok')) return [];
  return [...new Set(chain.filter((m) => chainItemState(cfg, m) === 'disabled').map(providerOf))];
}

/** Роли, которые используют модели провайдера. */
export function rolesUsing(cfg: Config, providerId: string): RoleName[] {
  return ROLE_NAMES.filter((r) => cfg.roles[r].some((m) => providerOf(m) === providerId));
}

export interface Badge {
  tone: 'free' | 'key' | 'local';
  text: string;
}

/** Метки на плитке провайдера. */
export function presetBadges(p: Pick<ProviderPreset, 'local' | 'free_tier' | 'needs_key'>): Badge[] {
  const out: Badge[] = [];
  if (p.local) out.push({ tone: 'local', text: 'локально' });
  if (p.free_tier) out.push({ tone: 'free', text: p.local ? 'бесплатно' : 'есть бесплатно' });
  out.push(p.needs_key ? { tone: 'key', text: 'нужен ключ' } : { tone: 'free', text: 'без ключа' });
  return out;
}

/** «Ollama (локально)» → «Ollama». */
export function shortName(name: string): string {
  return name.split(' (')[0].trim();
}

/** RMS 0…1 → ширина индикатора 0…100 % по шкале −60…0 dBFS. */
export function levelPercent(rms: number): number {
  if (!(rms > 0)) return 0;
  const db = 20 * Math.log10(rms);
  return Math.max(0, Math.min(100, ((db + 60) / 60) * 100));
}

/** Значение для `<select>` устройства: '' — «по умолчанию Windows». */
export function deviceValue(name: string | null | undefined): string {
  return name?.trim() ?? '';
}

export function setAudio(cfg: Config, patch: Partial<Config['audio']>): Config {
  return { ...cfg, audio: { ...cfg.audio, ...patch } };
}

/** Есть ли несохранённые изменения. */
export function isDirty(saved: Config | null, draft: Config | null): boolean {
  if (!saved || !draft) return false;
  return JSON.stringify(saved) !== JSON.stringify(draft);
}
