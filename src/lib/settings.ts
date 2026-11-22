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

/** Есть ли несохранённые изменения. */
export function isDirty(saved: Config | null, draft: Config | null): boolean {
  if (!saved || !draft) return false;
  return JSON.stringify(saved) !== JSON.stringify(draft);
}
