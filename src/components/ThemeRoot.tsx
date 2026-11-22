'use client';

import { useEffect, useSyncExternalStore } from 'react';

import { call } from '@/lib/bridge';
import { useTauriEvent } from '@/lib/store';
import { applyAppearance, livePalette, onThemeChange, type Palette } from '@/lib/theme';
import { CONFIG_CHANGED_EVENT, type AppearanceConfig, type Config } from '@/lib/types';

/**
 * Тема окна: при открытии берёт `appearance` из config.json, дальше — по событию `CONFIG_CHANGED_EVENT`
 * (сохранение в настройках или правка файла), поэтому все окна перекрашиваются сразу, без перезапуска.
 */
export default function ThemeRoot() {
  useEffect(() => {
    // последняя тема из кэша — без мелькания циана, пока не пришёл config.json
    try {
      const cached = localStorage.getItem(CACHE_KEY);
      if (cached) applyAppearance(JSON.parse(cached) as AppearanceConfig);
    } catch {
      /* нет хранилища — не страшно */
    }
    void call<Config>('config_get')
      .then((c) => apply(c.appearance))
      .catch(() => {});
  }, []);
  useTauriEvent<Config>(CONFIG_CHANGED_EVENT, (c) => apply(c.appearance));
  return null;
}

const CACHE_KEY = 'jarvis-appearance';
/** Атрибут на `<html>`: тему окна задаёт экран настроек (черновик), а не сохранённый конфиг. */
export const PREVIEW_ATTR = 'data-theme-preview';

/** Экран настроек: показывать черновик темы, пока он открыт. */
export function useThemePreview(ap: AppearanceConfig | undefined) {
  const key = ap ? JSON.stringify(ap) : '';
  useEffect(() => () => document.documentElement.removeAttribute(PREVIEW_ATTR), []);
  useEffect(() => {
    if (!key) return; // черновик ещё не загружен — тема из кэша/конфига остаётся
    document.documentElement.setAttribute(PREVIEW_ATTR, '');
    applyAppearance(JSON.parse(key) as AppearanceConfig);
  }, [key]);
}

function apply(ap: AppearanceConfig | undefined) {
  // окно настроек само показывает черновик темы (живой предпросмотр) — не перебивать его
  if (!document.documentElement.hasAttribute(PREVIEW_ATTR)) applyAppearance(ap);
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(ap ?? null));
  } catch {
    /* приватный режим */
  }
}

/** Палитра темы для компонентов, которые красят сами (оверлей, канвас). */
export function useLivePalette(): Palette {
  return useSyncExternalStore(onThemeChange, livePalette, livePalette);
}
