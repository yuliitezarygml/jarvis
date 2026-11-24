import { describe, expect, it } from 'vitest';

import { defaultConfig } from './mock';
import {
  FACT_CATEGORIES,
  PERSONA_MODES,
  addBannedWords,
  addressPlaceholder,
  categoryLabel,
  filterFacts,
  parseMoodPayload,
  personaTitle,
  removeBannedWord,
} from './persona';
import { PERSONA_MOOD_EVENT, type Fact } from './types';

const fact = (id: string, category: string): Fact => ({ id, text: id, confidence: 1, source: '', updated: '', category, important: false });

describe('persona', () => {
  it('modes cover the config enum, only uncensored warns', () => {
    expect(PERSONA_MODES.map((m) => m.v)).toEqual(['butler', 'buddy', 'uncensored']);
    expect(PERSONA_MODES.filter((m) => m.warn).map((m) => m.v)).toEqual(['uncensored']);
    expect(personaTitle('buddy')).toBe('Свой парень');
    expect(defaultConfig().persona.mode).toBe('butler');
    expect(addressPlaceholder('butler')).toContain('сэр');
    expect(addressPlaceholder('uncensored')).toContain('без обращения');
  });

  it('banned words: split, trim, lowercase, dedupe, remove', () => {
    let w = addBannedWords([], ' Блин,  ёлки  палки ;блин\n');
    expect(w).toEqual(['блин', 'ёлки палки']);
    w = addBannedWords(w, 'БЛИН, жесть');
    expect(w).toEqual(['блин', 'ёлки палки', 'жесть']);
    expect(addBannedWords(w, ' , ;')).toEqual(w);
    expect(removeBannedWord(w, 'Ёлки Палки')).toEqual(['блин', 'жесть']);
  });

  it('fact categories and the «Характер» filter', () => {
    expect(FACT_CATEGORIES).toContain('style');
    expect(categoryLabel('style')).toBe('Характер');
    expect(categoryLabel('')).toBe('Общее');
    expect(categoryLabel(undefined)).toBe('Общее');
    expect(categoryLabel('своя')).toBe('своя');
    const facts = [fact('a', 'style'), fact('b', 'preference'), fact('c', 'style')];
    expect(filterFacts(facts, 'style').map((f) => f.id)).toEqual(['a', 'c']);
    expect(filterFacts(facts, 'all')).toBe(facts);
  });

  it('mood event payload is validated', () => {
    expect(PERSONA_MOOD_EVENT).toBe('persona:mood');
    expect(parseMoodPayload({ mood: 'offended', reason: ' дважды послали ' })).toEqual({ mood: 'offended', reason: 'дважды послали' });
    expect(parseMoodPayload({ mood: 'calm', reason: '' })).toEqual({ mood: 'calm', reason: undefined });
    expect(parseMoodPayload({ mood: 'angry' })).toBeNull();
    expect(parseMoodPayload('calm')).toBeNull();
    expect(parseMoodPayload(null)).toBeNull();
  });
});
