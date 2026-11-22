import { describe, expect, it } from 'vitest';

import { defaultConfig } from './mock';
import {
  addAnotherKey,
  addProvider,
  applyRanking,
  interleaveKeys,
  presetFor,
  providerTitle,
  blockingDisabled,
  chainItemState,
  deviceValue,
  levelPercent,
  presetBadges,
  rolesUsing,
  setAudio,
  setProviderEnabled,
  shortName,
} from './settings';
import type { Config, ProviderPreset } from './types';

const zen: ProviderPreset = {
  id: 'opencode-zen',
  name: 'OpenCode Zen',
  kind: 'openai_compat',
  base_url: 'https://opencode.ai/inference/openai/v1',
  key_url: 'https://opencode.ai/console',
  docs_url: '',
  needs_key: true,
  local: false,
  free_tier: false,
  tagline: 'Подборка сильных моделей',
  note: 'GET /models публичный…',
};

function twoProviders(): Config {
  return addProvider(defaultConfig(), zen, 'zen_key');
}

describe('провайдеры: включение', () => {
  it('добавление второго провайдера не трогает первого', () => {
    const c = twoProviders();
    expect(c.providers.map((p) => [p.id, p.enabled])).toEqual([
      ['groq', true],
      ['opencode-zen', true],
    ]);
  });

  it('выключатель меняет только своего провайдера (по id)', () => {
    const c = setProviderEnabled(twoProviders(), 'opencode-zen', false);
    expect(c.providers.find((p) => p.id === 'groq')?.enabled).toBe(true);
    expect(c.providers.find((p) => p.id === 'opencode-zen')?.enabled).toBe(false);
  });

  it('выключенный groq блокирует «Мозг», и это видно', () => {
    const c = setProviderEnabled(twoProviders(), 'groq', false);
    expect(chainItemState(c, 'groq/openai/gpt-oss-120b')).toBe('disabled');
    expect(chainItemState(c, 'nope/x')).toBe('missing');
    expect(blockingDisabled(c, 'brain')).toEqual(['groq']);
    expect(rolesUsing(c, 'groq')).toEqual(['brain']);
    // есть запасная модель у включённого провайдера — роль работает
    const withFallback = { ...c, roles: { ...c.roles, brain: [...c.roles.brain, 'opencode-zen/glm-5.3'] } };
    expect(blockingDisabled(withFallback, 'brain')).toEqual([]);
  });
});

describe('несколько ключей одного провайдера', () => {
  const groq: ProviderPreset = { ...zen, id: 'groq', name: 'Groq', base_url: 'https://api.groq.com/openai/v1' };
  it('groq-2 — та же карточка «Groq · ключ 2»', () => {
    let c = defaultConfig();
    c = addAnotherKey(c, c.providers[0], groq);
    expect(c.providers.map((p) => [p.id, p.base_url, p.api_key, p.enabled])).toEqual([
      ['groq', 'https://api.groq.com/openai/v1', '', true],
      ['groq-2', 'https://api.groq.com/openai/v1', '', true],
    ]);
    expect(presetFor([zen, groq], c.providers[1])?.id).toBe('groq');
    expect(presetFor([zen, groq], { id: 'my', kind: 'openai_compat', base_url: 'https://api.groq.com/openai/v1/' })?.id).toBe('groq');
    expect(providerTitle(c.providers[0], groq, c.providers)).toBe('Groq · ключ 1');
    expect(providerTitle(c.providers[1], groq, c.providers)).toBe('Groq · ключ 2');
    expect(providerTitle(c.providers[0], groq, [c.providers[0]])).toBe('Groq');
    expect(addAnotherKey(c, c.providers[1], groq).providers[2].id).toBe('groq-3');
  });

  it('«лучшее автоматически» чередует ключи', () => {
    const c = addAnotherKey(defaultConfig(), defaultConfig().providers[0], groq);
    expect(interleaveKeys(c.providers, ['groq/a', 'groq/b'])).toEqual(['groq/a', 'groq-2/a', 'groq/b', 'groq-2/b']);
    const roles = applyRanking(c.roles, [{ role: 'brain', top: [{ model: 'groq/a', reason: '' }] }], c.providers);
    expect(roles.brain.slice(0, 2)).toEqual(['groq/a', 'groq-2/a']);
  });
});

describe('плитки и подписи', () => {
  it('метки', () => {
    expect(presetBadges({ local: true, free_tier: true, needs_key: false }).map((b) => b.text)).toEqual(['локально', 'бесплатно', 'без ключа']);
    expect(presetBadges({ local: false, free_tier: true, needs_key: true }).map((b) => b.text)).toEqual(['есть бесплатно', 'нужен ключ']);
    expect(shortName('Ollama (локально)')).toBe('Ollama');
  });
});

describe('аудио', () => {
  it('уровень микрофона в процентах по dBFS', () => {
    expect(levelPercent(0)).toBe(0);
    expect(levelPercent(Number.NaN)).toBe(0);
    expect(levelPercent(1)).toBe(100);
    expect(levelPercent(0.001)).toBeCloseTo(0, 5);
    expect(levelPercent(0.0316)).toBeCloseTo(50, 0);
  });

  it('устройства и громкость', () => {
    const c = setAudio(defaultConfig(), { input_device: 'USB', volume: 0.8 });
    expect(c.audio).toEqual({ input_device: 'USB', output_device: null, volume: 0.8, listen_on_start: true });
    expect(deviceValue(null)).toBe('');
    expect(deviceValue(' USB ')).toBe('USB');
  });
});
