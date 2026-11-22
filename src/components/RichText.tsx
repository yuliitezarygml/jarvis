import { Fragment, type ReactNode } from 'react';

/** Кусок реплики: обычный текст, жирный или код. */
export type Span = { kind: 'text' | 'bold' | 'code'; text: string };

/**
 * Простейший Markdown из ответов модели → куски для субтитров: `**жирный**`, `__жирный__`, `` `код` ``.
 * Заголовки (`#`), маркеры списков и одиночные `*`/`_` выделения убираются — субтитры одной строкой.
 */
export function parseInline(src: string): Span[] {
  const text = src
    .replace(/^\s{0,3}#{1,6}\s+/gm, '')
    .replace(/^\s*[-*•]\s+/gm, '')
    .replace(/^\s*\d+[.)]\s+/gm, (m) => m.trim() + ' ');
  const out: Span[] = [];
  const re = /\*\*(.+?)\*\*|__(.+?)__|`([^`]+)`/g;
  let last = 0;
  for (const m of text.matchAll(re)) {
    const at = m.index ?? 0;
    if (at > last) out.push({ kind: 'text', text: text.slice(last, at) });
    if (m[3] !== undefined) out.push({ kind: 'code', text: m[3] });
    else out.push({ kind: 'bold', text: m[1] ?? m[2] ?? '' });
    last = at + m[0].length;
  }
  if (last < text.length) out.push({ kind: 'text', text: text.slice(last) });
  // Незакрытые `**` (текст ещё печатается потоком) и одиночные `*` — просто убрать.
  return out
    .map((s) => (s.kind === 'text' ? { ...s, text: s.text.replace(/\*\*|(?<!\w)\*(?=\S)|(?<=\S)\*(?!\w)/g, '') } : s))
    .filter((s) => s.text.length > 0);
}

/** Реплика с жирным и кодом вместо звёздочек и обратных кавычек. */
export default function RichText({ text }: { text: string }): ReactNode {
  return parseInline(text).map((s, i) => (
    <Fragment key={i}>
      {s.kind === 'bold' ? <strong>{s.text}</strong> : s.kind === 'code' ? <code className="rt-code">{s.text}</code> : s.text}
    </Fragment>
  ));
}
