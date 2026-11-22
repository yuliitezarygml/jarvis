import { describe, expect, it } from 'vitest';

import { parseInline } from '@/components/RichText';

describe('parseInline', () => {
  it('жирный вместо звёздочек', () => {
    expect(parseInline('Больше всего **Code** (~2,4 ГБ) и **Chrome**')).toEqual([
      { kind: 'text', text: 'Больше всего ' },
      { kind: 'bold', text: 'Code' },
      { kind: 'text', text: ' (~2,4 ГБ) и ' },
      { kind: 'bold', text: 'Chrome' },
    ]);
  });

  it('код, заголовки и маркеры списков', () => {
    expect(parseInline('# Итог\n- запусти `git commit`')).toEqual([
      { kind: 'text', text: 'Итог\nзапусти ' },
      { kind: 'code', text: 'git commit' },
    ]);
  });

  it('незакрытые звёздочки во время потока не видны', () => {
    const s = parseInline('Сейчас **Co');
    expect(s.map((x) => x.text).join('')).toBe('Сейчас Co');
  });

  it('обычный текст и умножение не трогает', () => {
    expect(parseInline('2 * 3 = 6')).toEqual([{ kind: 'text', text: '2 * 3 = 6' }]);
  });
});
